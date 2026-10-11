#![allow(dead_code)]

use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

#[derive(Clone)]
pub struct Reply {
    pub status: u16,
    pub body: String,
    pub headers: Vec<(String, String)>,
    pub header_delay: Duration,
    pub body_delay: Duration,
}

impl Reply {
    pub fn json(body: Value) -> Self {
        Self {
            status: 200,
            body: body.to_string(),
            headers: vec![],
            header_delay: Duration::ZERO,
            body_delay: Duration::ZERO,
        }
    }

    pub fn text(text: &str) -> Self {
        Self::json(json!({
            "id": "mock-response", "object": "chat.completion", "created": 0, "model": "example-model",
            "choices": [{"index": 0, "message": {"role": "assistant", "content": text}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 2, "completion_tokens": 1, "total_tokens": 3}
        }))
    }
}

#[derive(Clone, Debug)]
pub struct Request {
    pub headers: String,
    pub body: Value,
}

pub struct Server {
    pub base_url: String,
    requests: Arc<Mutex<Vec<Request>>>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl Server {
    pub fn start(replies: Vec<Reply>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let base_url = format!("http://{}/prefix/v1", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured = requests.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let shutdown = stop.clone();
        let worker = thread::spawn(move || {
            let mut index = 0;
            while !shutdown.load(Ordering::SeqCst) {
                let (mut socket, _) = match listener.accept() {
                    Ok(connection) => connection,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(1));
                        continue;
                    }
                    Err(error) => panic!("mock accept failed: {error}"),
                };
                // Darwin can inherit the listener's nonblocking mode on accepted sockets.
                socket.set_nonblocking(false).unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut bytes = Vec::new();
                let mut buffer = [0; 4096];
                let request = loop {
                    let count = match socket.read(&mut buffer) {
                        Ok(0) | Err(_) => break None,
                        Ok(count) => count,
                    };
                    bytes.extend_from_slice(&buffer[..count]);
                    if let Some(offset) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                        let headers = String::from_utf8(bytes[..offset].to_vec()).unwrap();
                        let length: usize = headers
                            .lines()
                            .find_map(|line| {
                                let (name, value) = line.split_once(':')?;
                                name.eq_ignore_ascii_case("content-length")
                                    .then(|| value.trim().parse().unwrap())
                            })
                            .unwrap();
                        if bytes.len() >= offset + 4 + length {
                            break Some(Request {
                                headers,
                                body: serde_json::from_slice(
                                    &bytes[offset + 4..offset + 4 + length],
                                )
                                .unwrap(),
                            });
                        }
                    }
                };
                let Some(request) = request else { continue };
                captured.lock().unwrap().push(request);
                let reply = replies.get(index).cloned().unwrap_or_else(|| {
                    let mut reply =
                        Reply::json(json!({"error": {"message": "unexpected request"}}));
                    reply.status = 500;
                    reply
                });
                index += 1;
                thread::sleep(reply.header_delay);
                let mut headers = format!(
                    "HTTP/1.1 {} Mock\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
                    reply.status,
                    reply.body.len()
                );
                for (name, value) in reply.headers {
                    headers.push_str(&format!("{name}: {value}\r\n"));
                }
                headers.push_str("\r\n");
                let _ = socket.write_all(headers.as_bytes());
                thread::sleep(reply.body_delay);
                let _ = socket.write_all(reply.body.as_bytes());
            }
        });
        Self {
            base_url,
            requests,
            stop,
            worker: Some(worker),
        }
    }

    pub fn requests(&self) -> Vec<Request> {
        self.requests.lock().unwrap().clone()
    }

    pub fn wait_for_requests(&self, count: usize) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while self.requests.lock().unwrap().len() < count {
            assert!(Instant::now() < deadline, "mock request did not arrive");
            thread::sleep(Duration::from_millis(1));
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.worker.take().unwrap().join().unwrap();
    }
}

pub fn messages(request: &Request) -> Vec<Value> {
    request.body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|message| {
            let content = match &message["content"] {
                Value::String(text) => text.clone(),
                Value::Array(parts) => parts
                    .iter()
                    .map(|part| {
                        assert_eq!(part["type"], "text");
                        part["text"].as_str().unwrap()
                    })
                    .collect::<String>(),
                _ => panic!("unsupported mock message content"),
            };
            json!({"role": message["role"], "content": content})
        })
        .collect()
}
