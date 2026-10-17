use crate::{error::*, model::ServerConfig};
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
    pub capabilities: Value,
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
            capabilities: Value::Null,
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

    pub fn send(&mut self, mut message: Value) -> Result<(), Error> {
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
                let lengths: Vec<usize> = header
                    .into_iter()
                    .flat_map(str::lines)
                    .filter_map(|line| line.split_once(':'))
                    .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                    .filter_map(|(_, value)| value.trim().parse().ok())
                    .collect();
                ensure!(
                    lengths.len() == 1,
                    ProtocolSnafu {
                        message: "missing or invalid Content-Length"
                    }
                );
                let length = lengths[0];
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
                "$/progress" => {
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
                    "window/workDoneProgress/create" => Value::Null,
                    "workspace/configuration" => Value::Array(
                        message["params"]["items"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .map(|item| {
                                let section = item["section"].as_str().unwrap_or("");
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
                    "workspace/workspaceFolders" => {
                        json!([{ "uri": self.root_uri, "name": "snapshot" }])
                    }
                    "client/registerCapability" => {
                        for entry in message["params"]["registrations"]
                            .as_array()
                            .into_iter()
                            .flatten()
                        {
                            if let Some(key) = provider(entry["method"].as_str().unwrap_or("")) {
                                self.capabilities[key] = json!(true);
                            }
                        }
                        Value::Null
                    }
                    "client/unregisterCapability" => {
                        for entry in message["params"]["unregisterations"]
                            .as_array()
                            .into_iter()
                            .flatten()
                        {
                            if let Some(key) = provider(entry["method"].as_str().unwrap_or("")) {
                                self.capabilities[key] = json!(false);
                            }
                        }
                        Value::Null
                    }
                    "workspace/applyEdit" => json!({"applied": false}),
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

    pub fn request(&mut self, method: &str, mut params: Value) -> Result<Value, Error> {
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
            "textDocument/documentSymbol"
                | "textDocument/references"
                | "callHierarchy/incomingCalls"
        ) {
            params["partialResultToken"] = json!(token);
        }
        self.send(json!({"id":id,"method":method,"params":params}))?;
        let mut partial = Vec::new();
        let mut partial_bytes = 0;
        loop {
            let message = self.receive()?;
            if message["method"] == "$/progress"
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
                if partial.is_empty() {
                    return Ok(result);
                }
                if let Some(values) = result.as_array() {
                    partial.extend(values.iter().cloned());
                }
                return Ok(Value::Array(partial));
            }
        }
    }

    pub fn supports(&self, name: &str) -> bool {
        self.capabilities
            .get(name)
            .is_some_and(|value| !value.is_null() && value != false)
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

fn provider(method: &str) -> Option<&'static str> {
    match method {
        "textDocument/documentSymbol" => Some("documentSymbolProvider"),
        "textDocument/references" => Some("referencesProvider"),
        "textDocument/prepareCallHierarchy" => Some("callHierarchyProvider"),
        _ => None,
    }
}
