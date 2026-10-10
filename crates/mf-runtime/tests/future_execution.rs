#![cfg(feature = "async")]

use mf_runtime::{ExecutionContext, FutureExecutionError, StreamCancellation, StreamError};
use std::{
    error::Error,
    future::{pending, poll_fn},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    task::Poll,
    thread,
    time::{Duration, Instant},
};

struct DropProbe(Arc<AtomicBool>);

impl Drop for DropProbe {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

async fn waiting(dropped: Arc<AtomicBool>) {
    let _probe = DropProbe(dropped);
    pending().await
}

#[test]
fn drives_values_and_preserves_operation_errors() {
    let context = ExecutionContext::default();
    assert_eq!(context.execute_future(async { 42 }, None).unwrap(), 42);
    let result = context
        .execute_future(
            async { Err::<(), _>(std::io::Error::other("provider")) },
            None,
        )
        .unwrap();
    assert_eq!(result.unwrap_err().to_string(), "provider");
}

#[test]
fn a_deadline_wakes_and_drops_a_pending_operation() {
    let context = ExecutionContext::default();
    let dropped = Arc::new(AtomicBool::new(false));
    let result = context.execute_future(
        waiting(dropped.clone()),
        Some(Instant::now() + Duration::from_millis(25)),
    );
    assert!(matches!(
        result,
        Err(FutureExecutionError::DeadlineExceeded)
    ));
    assert!(dropped.load(Ordering::SeqCst));
}

#[test]
fn cancellation_wakes_a_pending_operation_and_preserves_the_source() {
    let mut context = ExecutionContext::default();
    let cancellation = StreamCancellation::default();
    context.set_cancellation(cancellation.clone());
    let dropped = Arc::new(AtomicBool::new(false));
    let (started, receiver) = std::sync::mpsc::channel();
    let worker_dropped = dropped.clone();
    let worker = thread::spawn(move || {
        let mut notified = false;
        context.execute_future(
            async {
                let _probe = DropProbe(worker_dropped);
                poll_fn(|_| {
                    if !notified {
                        started.send(()).unwrap();
                        notified = true;
                    }
                    Poll::<()>::Pending
                })
                .await;
            },
            None,
        )
    });
    receiver.recv_timeout(Duration::from_secs(5)).unwrap();
    cancellation.cancel(StreamError::Execution {
        message: "stopped".into(),
    });
    let error = worker.join().unwrap().unwrap_err();
    assert!(matches!(error, FutureExecutionError::Cancelled { .. }));
    assert!(error.source().unwrap().is::<StreamError>());
    assert!(dropped.load(Ordering::SeqCst));
}

#[test]
fn expired_or_cancelled_contexts_do_not_poll_business_futures() {
    let mut context = ExecutionContext::default();
    let result = context.execute_future(async { panic!("must not poll") }, Some(Instant::now()));
    assert!(matches!(
        result,
        Err(FutureExecutionError::DeadlineExceeded)
    ));
    let cancellation = StreamCancellation::default();
    cancellation.cancel(StreamError::Execution {
        message: "stopped".into(),
    });
    context.set_cancellation(cancellation);
    let result = context.execute_future(async { panic!("must not poll") }, None);
    assert!(matches!(
        result,
        Err(FutureExecutionError::Cancelled { .. })
    ));
}

#[test]
fn rejects_nested_portable_entry_without_panicking() {
    let context = ExecutionContext::default();
    let error = futures::executor::block_on(async {
        context.execute_future(async { 42 }, None).unwrap_err()
    });
    assert!(matches!(error, FutureExecutionError::NestedExecutor { .. }));
    assert!(
        error
            .source()
            .unwrap()
            .is::<futures::executor::EnterError>()
    );
}

#[test]
fn forked_contexts_inherit_cancellation_and_drive_independent_values() {
    let context = ExecutionContext::default();
    let forks = [
        context.fork_domain_with_observation(None),
        context.fork_stream(None),
        context.fork_body(None),
    ];
    let workers = forks
        .into_iter()
        .enumerate()
        .map(|(index, context)| {
            thread::spawn(move || context.execute_future(async move { index }, None).unwrap())
        })
        .collect::<Vec<_>>();
    for (index, worker) in workers.into_iter().enumerate() {
        assert_eq!(worker.join().unwrap(), index);
    }
    context.cancellation().cancel(StreamError::Execution {
        message: "stopped".into(),
    });
    assert!(matches!(
        context.fork_body(None).execute_future(async { 1 }, None),
        Err(FutureExecutionError::Cancelled { .. })
    ));
}

#[cfg(feature = "tokio")]
#[test]
fn managed_backend_has_io_and_timers_and_rejects_async_host_entry() {
    let context = ExecutionContext::default();
    context
        .execute_future(
            async {
                tokio::time::sleep(Duration::from_millis(1)).await;
                let socket = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                assert!(socket.local_addr().unwrap().port() > 0);
            },
            None,
        )
        .unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let result = runtime.block_on(async { context.execute_future(async { 42 }, None) });
    assert!(matches!(result, Err(FutureExecutionError::AsyncContext)));
}

#[cfg(feature = "tokio")]
#[test]
fn concurrent_context_forks_share_one_managed_backend() {
    let context = ExecutionContext::default();
    let forks = [
        context.fork_domain_with_observation(None),
        context.fork_stream(None),
        context.fork_body(None),
    ];
    let workers = forks
        .into_iter()
        .map(|context| {
            thread::spawn(move || {
                context
                    .execute_future(async { tokio::runtime::Handle::current().id() }, None)
                    .unwrap()
            })
        })
        .collect::<Vec<_>>();
    let id = context
        .execute_future(async { tokio::runtime::Handle::current().id() }, None)
        .unwrap();
    for worker in workers {
        assert_eq!(worker.join().unwrap(), id);
    }
    let separate = ExecutionContext::default()
        .execute_future(async { tokio::runtime::Handle::current().id() }, None)
        .unwrap();
    assert_ne!(id, separate);
}

#[cfg(feature = "tokio")]
#[test]
fn managed_backend_lives_until_the_last_context_releases_it() {
    let context = ExecutionContext::default();
    let fork = context.fork_body(None);
    let dropped = Arc::new(AtomicBool::new(false));
    let worker_dropped = dropped.clone();
    let (started, receiver) = std::sync::mpsc::channel();
    context
        .execute_future(
            async move {
                tokio::spawn(async move {
                    let _probe = DropProbe(worker_dropped);
                    started.send(()).unwrap();
                    pending::<()>().await;
                });
            },
            None,
        )
        .unwrap();
    receiver.recv_timeout(Duration::from_secs(5)).unwrap();
    drop(context);
    assert!(!dropped.load(Ordering::SeqCst));
    drop(fork);
    assert!(dropped.load(Ordering::SeqCst));
}
