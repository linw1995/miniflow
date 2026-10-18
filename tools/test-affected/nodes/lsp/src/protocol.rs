use crate::{error::*, model::ServerConfig};
use lsp_types::{
    self as lsp,
    notification::{Notification, Progress},
    request::{self, Request},
};
use mf_runtime::StreamCancellation;
use serde_json::{Value, json};
use snafu::{ResultExt, ensure};
use std::{
    io::{Read, Write},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    time::{Duration, Instant},
};

pub fn check(cancellation: &StreamCancellation, deadline: Instant) -> Result<(), Error> {
    if let Some(source) = cancellation.failure() {
        return Err(source).context(CancelledSnafu);
    }
    ensure!(
        Instant::now() < deadline,
        LimitSnafu {
            message: "deadline exceeded"
        }
    );
    Ok(())
}

pub struct Session {
    child: Child,
    closed: bool,
    input: ChildStdin,
    output: ChildStdout,
    buffer: Vec<u8>,
    sequence: u64,
    max_requests: usize,
    max_bytes: usize,
    deadline: Instant,
    cancellation: StreamCancellation,
    options: Value,
    root_uri: String,
    pub status: Option<Value>,
    pub progress: usize,
}

impl Session {
    pub fn start(
        config: &ServerConfig,
        root: &std::path::Path,
        options: Value,
        deadline: Instant,
        cancellation: StreamCancellation,
    ) -> Result<Self, Error> {
        check(&cancellation, deadline)?;
        let mut command = Command::new(&config.command);
        command
            .args(&config.args)
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let mut child = command.spawn().context(IoSnafu {
            action: "start language server",
        })?;
        let input = child.stdin.take().expect("piped stdin");
        let output = child.stdout.take().expect("piped stdout");
        let root_uri = match url::Url::from_directory_path(root) {
            Ok(uri) => uri.to_string(),
            Err(()) => {
                let _ = child.kill();
                child.wait().context(CleanupSnafu)?;
                return InvalidSnafu {
                    message: "snapshot root must be absolute",
                }
                .fail();
            }
        };
        let session = Self {
            child,
            closed: false,
            input,
            output,
            buffer: Vec::new(),
            sequence: 0,
            max_requests: config.max_requests,
            max_bytes: config.max_bytes,
            deadline,
            cancellation,
            options,
            root_uri,
            status: None,
            progress: 0,
        };
        session.nonblocking().context(IoSnafu {
            action: "configure language-server pipes",
        })?;
        Ok(session)
    }

    #[cfg(unix)]
    fn nonblocking(&self) -> std::io::Result<()> {
        use std::os::fd::AsRawFd;
        for fd in [self.input.as_raw_fd(), self.output.as_raw_fd()] {
            let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
            if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
            {
                return Err(std::io::Error::last_os_error());
            }
        }
        Ok(())
    }

    #[cfg(not(unix))]
    fn nonblocking(&self) -> std::io::Result<()> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "LSP transport requires Unix nonblocking pipes",
        ))
    }

    fn pause(&self) -> Result<(), Error> {
        check(&self.cancellation, self.deadline)?;
        std::thread::sleep(Duration::from_millis(5));
        Ok(())
    }

    fn send(&mut self, mut message: Value) -> Result<(), Error> {
        message["jsonrpc"] = json!("2.0");
        let body = serde_json::to_vec(&message).context(JsonSnafu)?;
        ensure!(
            body.len() <= self.max_bytes,
            LimitSnafu {
                message: "outgoing message too large"
            }
        );
        let mut bytes = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
        bytes.extend(body);
        let mut offset = 0;
        while offset < bytes.len() {
            check(&self.cancellation, self.deadline)?;
            match self.input.write(&bytes[offset..]) {
                Ok(0) => {
                    return ProtocolSnafu {
                        message: "language server closed stdin",
                    }
                    .fail();
                }
                Ok(count) => offset += count,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => self.pause()?,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => {
                    return Err(e).context(IoSnafu {
                        action: "write LSP message",
                    });
                }
            }
        }
        Ok(())
    }

    fn read(&mut self) -> Result<Value, Error> {
        loop {
            check(&self.cancellation, self.deadline)?;
            if let Some(header_end) = self.buffer.windows(4).position(|s| s == b"\r\n\r\n") {
                ensure!(
                    header_end <= 8192,
                    LimitSnafu {
                        message: "LSP header too large"
                    }
                );
                let header = std::str::from_utf8(&self.buffer[..header_end]).ok();
                let lengths: Option<Vec<usize>> = header
                    .into_iter()
                    .flat_map(str::lines)
                    .filter_map(|line| line.split_once(':'))
                    .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                    .map(|(_, value)| value.trim().parse().ok())
                    .collect();
                ensure!(
                    lengths.as_ref().is_some_and(|values| values.len() == 1),
                    ProtocolSnafu {
                        message: "missing or invalid Content-Length"
                    }
                );
                let length = lengths.unwrap()[0];
                ensure!(
                    length <= self.max_bytes,
                    LimitSnafu {
                        message: "incoming message too large"
                    }
                );
                let end = header_end + 4 + length;
                if self.buffer.len() >= end {
                    let message = serde_json::from_slice(&self.buffer[header_end + 4..end])
                        .context(JsonSnafu)?;
                    self.buffer.drain(..end);
                    return Ok(message);
                }
            } else {
                ensure!(
                    self.buffer.len() <= 8192,
                    LimitSnafu {
                        message: "unterminated LSP header"
                    }
                );
            }
            let mut chunk = [0; 8192];
            match self.output.read(&mut chunk) {
                Ok(0) => {
                    return ProtocolSnafu {
                        message: "language server closed stdout",
                    }
                    .fail();
                }
                Ok(count) => self.buffer.extend_from_slice(&chunk[..count]),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => self.pause()?,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => {
                    return Err(e).context(IoSnafu {
                        action: "read LSP message",
                    });
                }
            }
        }
    }

    pub fn receive(&mut self) -> Result<Value, Error> {
        let message = self.read()?;
        if let Some(method) = message["method"].as_str() {
            match method {
                "experimental/serverStatus" => self.status = Some(message["params"].clone()),
                Progress::METHOD => {
                    use opentelemetry::trace::TraceContextExt;
                    self.progress += 1;
                    opentelemetry::Context::current()
                        .span()
                        .add_event("lsp.progress", Vec::new());
                }
                _ => {}
            }
            if message.get("id").is_some() {
                let result = match method {
                    request::WorkDoneProgressCreate::METHOD => Value::Null,
                    request::WorkspaceConfiguration::METHOD => Value::Array(
                        serde_json::from_value::<lsp::ConfigurationParams>(
                            message["params"].clone(),
                        )
                        .context(JsonSnafu)?
                        .items
                        .into_iter()
                        .map(|item| {
                            let section = item.section.as_deref().unwrap_or("");
                            if section.is_empty() || section == "rust-analyzer" {
                                return self.options.clone();
                            }
                            section
                                .split('.')
                                .fold(&self.options, |value, key| &value[key])
                                .clone()
                        })
                        .collect(),
                    ),
                    request::WorkspaceFoldersRequest::METHOD => {
                        json!([{ "uri": self.root_uri, "name": "snapshot" }])
                    }
                    request::ApplyWorkspaceEdit::METHOD => json!({"applied": false}),
                    _ => {
                        self.send(json!({"id":message["id"],"error":{"code":-32601,"message":"unsupported client request"}}))?;
                        return Ok(message);
                    }
                };
                self.send(json!({"id":message["id"],"result":result}))?;
            }
        }
        Ok(message)
    }

    pub fn notify<N: Notification>(&mut self, params: N::Params) -> Result<(), Error> {
        let params = serde_json::to_value(params).context(JsonSnafu)?;
        self.send(json!({"method": N::METHOD, "params": params}))
    }

    pub fn request<R: Request>(&mut self, params: R::Params) -> Result<R::Result, Error> {
        let method = R::METHOD;
        let mut params = serde_json::to_value(params).context(JsonSnafu)?;
        ensure!(
            (self.sequence as usize) < self.max_requests,
            LimitSnafu {
                message: "request budget exhausted"
            }
        );
        self.sequence += 1;
        let id = self.sequence;
        let token = format!("analysis-{id}");
        if matches!(
            method,
            request::DocumentSymbolRequest::METHOD
                | request::References::METHOD
                | request::CallHierarchyIncomingCalls::METHOD
        ) {
            params["partialResultToken"] = json!(token);
        }
        self.send(json!({"id":id,"method":method,"params":params}))?;
        let mut partial = Vec::new();
        let mut partial_bytes = 0;
        loop {
            let message = self.receive()?;
            if message["method"] == Progress::METHOD
                && message["params"]["token"] == token
                && let Some(values) = message["params"]["value"].as_array()
            {
                partial_bytes += serde_json::to_vec(values).context(JsonSnafu)?.len();
                ensure!(
                    partial_bytes <= self.max_bytes,
                    LimitSnafu {
                        message: "partial results too large"
                    }
                );
                partial.extend(values.iter().cloned());
            }
            if message.get("method").is_none() && message["id"] == id {
                if let Some(error) = message.get("error") {
                    return ProtocolSnafu {
                        message: format!("{method}: {error}"),
                    }
                    .fail();
                }
                ensure!(
                    message.get("result").is_some(),
                    ProtocolSnafu {
                        message: "response has no result"
                    }
                );
                let result = message["result"].clone();
                let result = if partial.is_empty() {
                    result
                } else {
                    match result {
                        Value::Null => {}
                        Value::Array(values) => partial.extend(values),
                        _ => {
                            return ProtocolSnafu {
                                message: "invalid final partial result",
                            }
                            .fail();
                        }
                    }
                    Value::Array(partial)
                };
                return serde_json::from_value(result).context(JsonSnafu);
            }
        }
    }

    pub fn finish(&mut self) -> Result<(), Error> {
        if self.closed {
            return Ok(());
        }
        // Cleanup is bounded independently of the exhausted analysis deadline.
        #[cfg(unix)]
        unsafe {
            libc::kill(-(self.child.id() as i32), libc::SIGKILL);
        }
        if self.child.try_wait().context(CleanupSnafu)?.is_none() {
            self.child.kill().context(CleanupSnafu)?;
        }
        self.child.wait().context(CleanupSnafu)?;
        self.closed = true;
        Ok(())
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.finish();
    }
}
