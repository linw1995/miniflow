use crate::StreamError;
use std::{
    sync::{Arc, Mutex},
    task::Waker,
};

#[derive(Clone, Debug, Default)]
pub struct StreamCancellation(Arc<Mutex<CancellationState>>);

#[derive(Debug, Default)]
struct CancellationState {
    failure: Option<StreamError>,
    wakers: Vec<Waker>,
    #[cfg(feature = "async")]
    waiters: Vec<Arc<Mutex<Option<Waker>>>>,
}

impl StreamCancellation {
    pub fn failure(&self) -> Option<StreamError> {
        self.0.lock().unwrap().failure.clone()
    }

    pub fn cancel(&self, failure: StreamError) {
        let wakers = {
            let mut state = self.0.lock().unwrap();
            if state.failure.is_some() {
                return;
            }
            state.failure = Some(failure);
            let wakers = std::mem::take(&mut state.wakers);
            #[cfg(feature = "async")]
            {
                let mut wakers = wakers;
                for waiter in std::mem::take(&mut state.waiters) {
                    if let Some(waker) = waiter.lock().unwrap().take() {
                        wakers.push(waker);
                    }
                }
                wakers
            }
            #[cfg(not(feature = "async"))]
            wakers
        };
        for waker in wakers {
            waker.wake();
        }
    }

    pub fn register(&self, waker: Waker) {
        let mut state = self.0.lock().unwrap();
        if state.failure.is_some() {
            drop(state);
            waker.wake();
        } else if !state
            .wakers
            .iter()
            .any(|existing| existing.will_wake(&waker))
        {
            state.wakers.push(waker);
        }
    }

    /// A cancellable wait whose registration is removed when the future is dropped.
    #[cfg(feature = "async")]
    pub fn cancelled(&self) -> Cancelled {
        Cancelled {
            cancellation: self.clone(),
            waiter: Arc::new(Mutex::new(None)),
            registered: false,
        }
    }
}

#[cfg(feature = "async")]
pub struct Cancelled {
    cancellation: StreamCancellation,
    waiter: Arc<Mutex<Option<Waker>>>,
    registered: bool,
}

#[cfg(feature = "async")]
impl std::future::Future for Cancelled {
    type Output = StreamError;

    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        context: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        let cancellation = self.cancellation.clone();
        let mut state = cancellation.0.lock().unwrap();
        if let Some(failure) = &state.failure {
            return std::task::Poll::Ready(failure.clone());
        }
        *self.waiter.lock().unwrap() = Some(context.waker().clone());
        if !self.registered {
            state.waiters.push(self.waiter.clone());
            self.registered = true;
        }
        std::task::Poll::Pending
    }
}

#[cfg(feature = "async")]
impl Drop for Cancelled {
    fn drop(&mut self) {
        if self.registered {
            self.cancellation
                .0
                .lock()
                .unwrap()
                .waiters
                .retain(|waiter| !Arc::ptr_eq(waiter, &self.waiter));
        }
    }
}

#[cfg(all(test, feature = "async"))]
mod tests {
    use super::*;
    use std::{
        future::Future,
        task::{Context, Wake},
    };

    struct WakeCount(std::sync::atomic::AtomicUsize);

    impl Wake for WakeCount {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }

    #[test]
    fn cancellation_waiter_is_updated_and_removed_without_accumulation() {
        let cancellation = StreamCancellation::default();
        let first = Arc::new(WakeCount(std::sync::atomic::AtomicUsize::new(0)));
        let second = Arc::new(WakeCount(std::sync::atomic::AtomicUsize::new(0)));
        let mut future = Box::pin(cancellation.cancelled());
        for _ in 0..1000 {
            assert!(
                future
                    .as_mut()
                    .poll(&mut Context::from_waker(&Waker::from(first.clone())))
                    .is_pending()
            );
        }
        assert_eq!(cancellation.0.lock().unwrap().waiters.len(), 1);
        assert!(
            future
                .as_mut()
                .poll(&mut Context::from_waker(&Waker::from(second.clone())))
                .is_pending()
        );
        cancellation.cancel(crate::StreamExecutionSnafu { message: "stopped" }.build());
        assert_eq!(first.0.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert_eq!(second.0.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(cancellation.0.lock().unwrap().waiters.is_empty());
        drop(future);

        let fresh = StreamCancellation::default();
        let mut future = Box::pin(fresh.cancelled());
        assert!(
            future
                .as_mut()
                .poll(&mut Context::from_waker(&Waker::from(first)))
                .is_pending()
        );
        drop(future);
        assert!(fresh.0.lock().unwrap().waiters.is_empty());
    }
}
