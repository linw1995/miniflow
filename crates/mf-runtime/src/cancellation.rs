use crate::StreamError;
use std::{
    sync::{Arc, Mutex, Weak},
    task::Waker,
};

#[derive(Clone, Debug, Default)]
pub struct StreamCancellation(Arc<Mutex<CancellationState>>);

#[derive(Debug, Default)]
struct CancellationState {
    failure: Option<StreamError>,
    wakers: Vec<Waker>,
    parent: Option<StreamCancellation>,
    children: Vec<Weak<Mutex<CancellationState>>>,
    #[cfg(feature = "async")]
    waiters: Vec<Arc<Mutex<Option<Waker>>>>,
}

impl Drop for CancellationState {
    fn drop(&mut self) {
        if let Some(parent) = &self.parent {
            // Parent links are strong and child links weak: the parent stays alive
            // until its last child releases it, without retaining finished children.
            parent
                .0
                .lock()
                .unwrap()
                .children
                .retain(|child| child.strong_count() > 0);
        }
    }
}

impl StreamCancellation {
    /// Creates an invocation-local token. Parent cancellation propagates down;
    /// cancelling or dropping this child never cancels the parent or siblings.
    /// Weak child registrations are removed when the final child handle is dropped.
    pub fn child(&self) -> Self {
        let child = Self::default();
        child.0.lock().unwrap().parent = Some(self.clone());
        let failure = {
            let mut parent = self.0.lock().unwrap();
            if parent.failure.is_none() {
                parent.children.push(Arc::downgrade(&child.0));
            }
            parent.failure.clone()
        };
        if let Some(failure) = failure {
            child.cancel(failure);
        }
        child
    }

    pub fn failure(&self) -> Option<StreamError> {
        self.0.lock().unwrap().failure.clone()
    }

    pub fn cancel(&self, failure: StreamError) {
        let (wakers, children) = {
            let mut state = self.0.lock().unwrap();
            if state.failure.is_some() {
                return;
            }
            state.failure = Some(failure.clone());
            let children = std::mem::take(&mut state.children)
                .into_iter()
                .filter_map(|child| child.upgrade().map(Self))
                .collect::<Vec<_>>();
            let wakers = std::mem::take(&mut state.wakers);
            #[cfg(feature = "async")]
            {
                let mut wakers = wakers;
                for waiter in std::mem::take(&mut state.waiters) {
                    if let Some(waker) = waiter.lock().unwrap().take() {
                        wakers.push(waker);
                    }
                }
                (wakers, children)
            }
            #[cfg(not(feature = "async"))]
            (wakers, children)
        };
        for child in children {
            child.cancel(failure.clone());
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::task::Wake;
    #[cfg(feature = "async")]
    use std::{future::Future, task::Context};

    struct WakeCount(std::sync::atomic::AtomicUsize);

    impl Wake for WakeCount {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }

    #[test]
    fn child_cancellation_is_downward_only_and_preserves_typed_sources() {
        use snafu::IntoError;
        let parent = StreamCancellation::default();
        let child = parent.child();
        let sibling = parent.child();
        let grandchild = sibling.child();
        let wake = Arc::new(WakeCount(std::sync::atomic::AtomicUsize::new(0)));
        grandchild.register(Waker::from(wake.clone()));
        child.cancel(
            crate::StreamExecutionSnafu {
                message: "child only",
            }
            .build(),
        );
        assert!(parent.failure().is_none());
        assert!(sibling.failure().is_none());
        assert!(grandchild.failure().is_none());
        let failure = crate::stream_instance::OutputWriteSnafu
            .into_error(std::io::Error::other("typed parent failure"));
        parent.cancel(failure.clone());
        for token in [&parent, &sibling, &grandchild, &parent.child()] {
            match (token.failure().unwrap(), &failure) {
                (
                    StreamError::OutputWrite { source: actual },
                    StreamError::OutputWrite { source },
                ) => {
                    assert!(Arc::ptr_eq(&actual, source));
                }
                _ => panic!("parent's typed cancellation source was lost"),
            }
        }
        assert_eq!(wake.0.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(
            matches!(child.failure(), Some(StreamError::Execution { message }) if message == "child only")
        );
    }

    #[test]
    fn dead_children_are_removed_without_parent_waker_accumulation() {
        let parent = StreamCancellation::default();
        for _ in 0..1000 {
            let child = parent.child();
            let clone = child.clone();
            let grandchild = child.child();
            assert_eq!(parent.0.lock().unwrap().children.len(), 1);
            drop(grandchild);
            assert!(child.0.lock().unwrap().children.is_empty());
            drop(child);
            assert_eq!(parent.0.lock().unwrap().children.len(), 1);
            drop(clone);
            let state = parent.0.lock().unwrap();
            assert!(state.children.is_empty());
            assert!(state.wakers.is_empty());
        }
    }

    #[test]
    #[cfg(feature = "async")]
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
