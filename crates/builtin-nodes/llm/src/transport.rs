mod streaming;

use crate::{LlmExecutionError, NormalizeResponseSnafu, UsageMetadataSnafu};
use bytes::Bytes;
use rig_core::{
    completion::Usage,
    http_client::{
        Error, HttpClientExt, LazyBody, MultipartForm, Request, Response, Result, StreamingResponse,
    },
    wasm_compat::WasmCompatSend,
};
use rig_reqwest::ReqwestClient;
use serde::Deserialize;
use serde_json::Value;
use snafu::ResultExt;
use std::{
    future::Future,
    sync::{Arc, Mutex},
};

#[derive(Clone)]
pub struct TerminalTransport {
    inner: ReqwestClient,
    usage: Arc<Mutex<Option<Usage>>>,
}

impl TerminalTransport {
    pub fn new(inner: ReqwestClient) -> Self {
        Self {
            inner,
            usage: Arc::new(Mutex::new(None)),
        }
    }

    pub fn usage(&self) -> Option<Usage> {
        *self.usage.lock().unwrap()
    }
}

#[derive(Deserialize)]
struct WireUsage {
    prompt_tokens: Option<u64>,
    completion_tokens: Option<u64>,
    total_tokens: Option<u64>,
}

fn normalize(
    bytes: Bytes,
    usage: &Mutex<Option<Usage>>,
) -> std::result::Result<Bytes, LlmExecutionError> {
    let Ok(mut reply) = serde_json::from_slice::<Value>(&bytes) else {
        return Ok(bytes);
    };
    let Some(object) = reply.as_object_mut() else {
        return Ok(bytes);
    };
    let Some(counters) = object
        .get("usage")
        .filter(|value| !value.is_null())
        .cloned()
    else {
        return Ok(bytes);
    };
    let counters: WireUsage = serde_json::from_value(counters).context(UsageMetadataSnafu)?;
    let normalized = Usage {
        input_tokens: counters.prompt_tokens,
        output_tokens: counters.completion_tokens,
        total_tokens: counters.total_tokens,
        ..Default::default()
    };
    *usage.lock().unwrap() = Some(normalized);
    if counters.prompt_tokens.is_some()
        && counters.completion_tokens.is_some()
        && counters.total_tokens.is_some()
    {
        return Ok(bytes);
    }
    // Rig's Chat DTO requires all wire counts; carry optional counts separately instead of inventing zeroes.
    object.remove("usage");
    serde_json::to_vec(&reply)
        .context(NormalizeResponseSnafu)
        .map(Bytes::from)
}

impl HttpClientExt for TerminalTransport {
    fn send<T, U>(
        &self,
        request: Request<T>,
    ) -> impl Future<Output = Result<Response<LazyBody<U>>>> + WasmCompatSend + 'static
    where
        T: Into<Bytes> + WasmCompatSend,
        U: From<Bytes> + WasmCompatSend + 'static,
    {
        let request = request.map(Into::<Bytes>::into);
        let inner = self.inner.clone();
        let usage = self.usage.clone();
        async move {
            let response = inner.send::<Bytes, Bytes>(request).await?;
            let successful = response.status().is_success();
            let (parts, body) = response.into_parts();
            let body: LazyBody<U> = Box::pin(async move {
                let bytes = body.await?;
                let bytes = if successful {
                    normalize(bytes, &usage).map_err(Error::instance)?
                } else {
                    // Keep the original error body while retaining any valid reported counters.
                    let _ = normalize(bytes.clone(), &usage);
                    bytes
                };
                Ok(U::from(bytes))
            });
            Ok(Response::from_parts(parts, body))
        }
    }

    fn send_multipart<U>(
        &self,
        request: Request<MultipartForm>,
    ) -> impl Future<Output = Result<Response<LazyBody<U>>>> + WasmCompatSend + 'static
    where
        U: From<Bytes> + WasmCompatSend + 'static,
    {
        self.inner.send_multipart(request)
    }

    fn send_streaming<T>(
        &self,
        request: Request<T>,
    ) -> impl Future<Output = Result<StreamingResponse>> + WasmCompatSend
    where
        T: Into<Bytes> + WasmCompatSend,
    {
        let response = self.inner.send_streaming(request);
        let usage = self.usage.clone();
        async move {
            let response = response.await?;
            if !response.status().is_success() {
                return Ok(response);
            }
            Ok(response.map(|body| streaming::normalize_stream(body, usage)))
        }
    }
}
