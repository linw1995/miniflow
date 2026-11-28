use crate::{StreamCancellation, callback::*};
use serde::{Serialize, de::DeserializeOwned};
use snafu::{OptionExt, ResultExt, ensure};
use std::{
    io::{self, BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    time::{Duration, Instant},
};

const IO_TICK: Duration = Duration::from_millis(50);
const TRANSFER_TIMEOUT: Duration = Duration::from_secs(5);

pub struct Channel {
    socket: BufReader<UnixStream>,
    pub last_id: u64,
}

fn transient(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::Interrupted | io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
    )
}

fn check(
    cancellation: &StreamCancellation,
    deadline: Option<Instant>,
) -> Result<(), CallbackError> {
    if let Some(error) = cancellation.failure() {
        return Err(error.into());
    }
    ensure!(
        deadline.is_none_or(|end| Instant::now() < end),
        ProtocolSnafu {
            reason: "RPC transfer timed out"
        }
    );
    Ok(())
}

fn encode_frame(frame: &impl Serialize, limit: usize) -> Result<Vec<u8>, CallbackError> {
    struct BoundedFrame(Vec<u8>, usize);
    impl Write for BoundedFrame {
        fn write(&mut self, input: &[u8]) -> io::Result<usize> {
            if input.len() >= self.1 - self.0.len() {
                return Err(io::Error::other("RPC frame exceeds limit"));
            }
            self.0.extend_from_slice(input);
            Ok(input.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut bytes = BoundedFrame(Vec::new(), limit);
    serde_json::to_writer(&mut bytes, frame).context(CodecSnafu)?;
    bytes.0.push(b'\n');
    Ok(bytes.0)
}

impl Channel {
    pub fn new(socket: UnixStream) -> Result<Self, CallbackError> {
        // Accepted sockets may inherit the listener's nonblocking flag on BSD.
        socket.set_nonblocking(false).context(TransportSnafu)?;
        socket
            .set_read_timeout(Some(IO_TICK))
            .context(TransportSnafu)?;
        socket
            .set_write_timeout(Some(IO_TICK))
            .context(TransportSnafu)?;
        Ok(Self {
            socket: BufReader::new(socket),
            last_id: 0,
        })
    }

    pub fn next_id(&self) -> Result<u64, CallbackError> {
        self.last_id.checked_add(1).context(ProtocolSnafu {
            reason: "callback ID exhausted",
        })
    }

    pub fn read_frame(
        &mut self,
        limit: usize,
        cancel: &StreamCancellation,
    ) -> Result<Option<Vec<u8>>, CallbackError> {
        let mut bytes = Vec::new();
        let mut deadline = None;
        loop {
            check(cancel, deadline)?;
            let input = match self.socket.fill_buf() {
                Ok(input) => input,
                Err(e) if transient(&e) => continue,
                Err(e) => return Err(e).context(TransportSnafu),
            };
            if input.is_empty() {
                ensure!(
                    bytes.is_empty(),
                    ProtocolSnafu {
                        reason: "incomplete RPC message"
                    }
                );
                return Ok(None);
            }
            let newline = input.iter().position(|b| *b == b'\n');
            let len = newline.unwrap_or(input.len());
            ensure!(
                bytes.len() + len < limit,
                ProtocolSnafu {
                    reason: "RPC message exceeds limit"
                }
            );
            bytes.extend_from_slice(&input[..len]);
            self.socket.consume(len + usize::from(newline.is_some()));
            if newline.is_some() {
                return Ok(Some(bytes));
            }
            deadline.get_or_insert_with(|| Instant::now() + TRANSFER_TIMEOUT);
        }
    }

    pub fn reply<T: DeserializeOwned>(
        &mut self,
        cancel: &StreamCancellation,
    ) -> Result<T, CallbackError> {
        let bytes = self
            .read_frame(MAX_CALLBACK_FRAME, cancel)?
            .context(ProtocolSnafu {
                reason: "host closed without a result",
            })?;
        crate::decode_unique_json(&bytes).context(CodecSnafu)
    }

    pub fn send(
        &mut self,
        value: &impl Serialize,
        limit: usize,
        backpressured: bool,
        cancel: &StreamCancellation,
    ) -> Result<(), CallbackError> {
        let bytes = encode_frame(value, limit)?;
        // A host consuming live records can legitimately stop reading for longer than a control transfer.
        let deadline = (!backpressured).then(|| Instant::now() + TRANSFER_TIMEOUT);
        let mut rest = bytes.as_slice();
        while !rest.is_empty() {
            check(cancel, deadline)?;
            match self.socket.get_mut().write(rest) {
                Ok(0) => {
                    return Err(io::Error::from(io::ErrorKind::WriteZero)).context(TransportSnafu);
                }
                Ok(n) => rest = &rest[n..],
                Err(e) if transient(&e) => continue,
                Err(e) => return Err(e).context(TransportSnafu),
            }
        }
        Ok(())
    }
}
