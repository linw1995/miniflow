use crate::{DeepSeekExecutionError, RuntimeInitializationSnafu};
use snafu::ResultExt;
use std::{
    future::{Future, poll_fn},
    pin::pin,
    sync::{Arc, LazyLock},
};
use tokio::runtime::{Handle, Runtime};

static RUNTIME: LazyLock<Result<Runtime, Arc<std::io::Error>>> = LazyLock::new(|| {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .thread_name("mfn-deepseek")
        .enable_all()
        .build()
        .map_err(Arc::new)
});

fn context() -> Result<Handle, DeepSeekExecutionError> {
    match Handle::try_current() {
        Ok(handle) => Ok(handle),
        Err(_) => RUNTIME
            .as_ref()
            .map(|runtime| runtime.handle().clone())
            .map_err(Arc::clone)
            .context(RuntimeInitializationSnafu),
    }
}

pub fn bind<F: Future>(
    future: F,
) -> Result<impl Future<Output = F::Output>, DeepSeekExecutionError> {
    let handle = context()?;
    Ok(async move {
        let mut future = pin!(future);
        poll_fn(|context| {
            let _guard = handle.enter();
            future.as_mut().poll(context)
        })
        .await
    })
}
