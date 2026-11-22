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
    pub fragments: Option<Vec<Vec<u8>>>,
    pub fragment_delay: Duration,
    pub release_after_prefix: Option<Arc<AtomicBool>>,
}

impl Reply {
    pub fn json(body: Value) -> Self {
        Self {
            status: 200,
            body: body.to_string(),
            headers: vec![],
            header_delay: Duration::ZERO,
            body_delay: Duration::ZERO,
            fragments: None,
            fragment_delay: Duration::ZERO,
            release_after_prefix: None,
        }
    }

    pub fn stream_text(text: &str) -> Self {
        let mut reply = Self::text(text);
        let chunk = json!({"id":"mock-response","object":"chat.completion.chunk","created":0,"model":"example-model","choices":[{"index":0,"delta":{"content":text},"finish_reason":null}]});
        let finish = json!({"id":"mock-response","object":"chat.completion.chunk","created":0,"model":"example-model","choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":2,"completion_tokens":1,"total_tokens":3}});
        reply.body = format!("data: {chunk}\n\ndata: {finish}\n\ndata: [DONE]\n\n");
        reply
            .headers
            .push(("Content-Type".into(), "text/event-stream".into()));
        reply
    }

    pub fn text(text: &str) -> Self {
        Self::json(json!({
            "id": "mock-response", "object": "chat.completion", "created": 0, "model": "example-model",
            "choices": [{"index": 0, "message": {"role": "assistant", "content": text}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 2, "completion_tokens": 1, "total_tokens": 3}
        }))
    }

    pub fn models(models: Value) -> Self {
        Self::json(json!({
            "object": "list",
            "data": models
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
    model_requests: Arc<Mutex<Vec<Request>>>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl Server {
    pub fn start(replies: Vec<Reply>) -> Self {
        Self::spawn(replies, None)
    }

    pub fn start_with_models(replies: Vec<Reply>, model_replies: Vec<Reply>) -> Self {
        Self::spawn(replies, Some(model_replies))
    }

    pub fn start_models(model_replies: Vec<Reply>) -> Self {
        Self::spawn(Vec::new(), Some(model_replies))
    }

    fn spawn(replies: Vec<Reply>, model_replies: Option<Vec<Reply>>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let base_url = format!("http://{}/prefix/v1", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured = requests.clone();
        let model_requests = Arc::new(Mutex::new(Vec::new()));
        let captured_models = model_requests.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let shutdown = stop.clone();
        let worker = thread::spawn(move || {
            let mut index = 0;
            let mut model_index = 0;
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
                            .unwrap_or(0);
                        if bytes.len() >= offset + 4 + length {
                            let body = if length == 0 {
                                Value::Null
                            } else {
                                serde_json::from_slice(&bytes[offset + 4..offset + 4 + length])
                                    .unwrap()
                            };
                            break Some(Request { headers, body });
                        }
                    }
                };
                let Some(request) = request else { continue };
                let is_models_request = request
                    .headers
                    .lines()
                    .next()
                    .and_then(|line| {
                        let mut parts = line.split_whitespace();
                        let method = parts.next()?;
                        let path = parts.next()?;
                        Some(method == "GET" && path.ends_with("/models"))
                    })
                    .unwrap_or(false);
                let reply = if is_models_request {
                    captured_models.lock().unwrap().push(request);
                    match &model_replies {
                        Some(list) => {
                            let reply = list.get(model_index).cloned().unwrap_or_else(|| {
                                let mut reply = Reply::json(
                                    json!({"error": {"message": "unexpected models request"}}),
                                );
                                reply.status = 500;
                                reply
                            });
                            model_index += 1;
                            reply
                        }
                        None => {
                            let mut reply = Reply::json(
                                json!({"error": {"message": "models endpoint not configured"}}),
                            );
                            reply.status = 404;
                            reply
                        }
                    }
                } else {
                    captured.lock().unwrap().push(request);
                    let reply = replies.get(index).cloned().unwrap_or_else(|| {
                        let mut reply =
                            Reply::json(json!({"error": {"message": "unexpected request"}}));
                        reply.status = 500;
                        reply
                    });
                    index += 1;
                    reply
                };
                thread::sleep(reply.header_delay);
                let content_type = if reply
                    .headers
                    .iter()
                    .any(|(name, _)| name.eq_ignore_ascii_case("content-type"))
                {
                    ""
                } else {
                    "Content-Type: application/json\r\n"
                };
                let mut headers = format!(
                    "HTTP/1.1 {} Mock\r\n{content_type}Content-Length: {}\r\nConnection: close\r\n",
                    reply.status,
                    reply.body.len()
                );
                for (name, value) in reply.headers {
                    headers.push_str(&format!("{name}: {value}\r\n"));
                }
                headers.push_str("\r\n");
                let _ = socket.write_all(headers.as_bytes());
                thread::sleep(reply.body_delay);
                if let Some(fragments) = reply.fragments {
                    for (index, fragment) in fragments.into_iter().enumerate() {
                        if index == 1
                            && let Some(release) = &reply.release_after_prefix
                        {
                            while !release.load(Ordering::Acquire)
                                && !shutdown.load(Ordering::Relaxed)
                            {
                                thread::sleep(Duration::from_millis(1));
                            }
                        }
                        if socket.write_all(&fragment).is_err() {
                            break;
                        }
                        thread::sleep(reply.fragment_delay);
                    }
                } else {
                    let _ = socket.write_all(reply.body.as_bytes());
                }
            }
        });
        Self {
            base_url,
            requests,
            model_requests,
            stop,
            worker: Some(worker),
        }
    }

    pub fn requests(&self) -> Vec<Request> {
        self.requests.lock().unwrap().clone()
    }

    pub fn model_requests(&self) -> Vec<Request> {
        self.model_requests.lock().unwrap().clone()
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
