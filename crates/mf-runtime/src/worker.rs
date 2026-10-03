use snafu::{ResultExt, Snafu};
use std::{
    io,
    sync::{Arc, Mutex, mpsc},
    thread::{self, JoinHandle},
};

#[derive(Debug, Snafu)]
#[snafu(display("could not start workflow worker {worker}: {source}"))]
pub struct WorkerPoolError {
    pub worker: usize,
    pub source: io::Error,
}

/// Reusable threads with one waiting queue slot per worker.
///
/// The synchronous handler owns result delivery and task panic handling. Dropping the pool closes
/// submissions and joins its threads. Zero workers accept no jobs.
pub struct WorkerPool<Job> {
    sender: Option<mpsc::SyncSender<Job>>,
    threads: Vec<JoinHandle<()>>,
}

impl<Job: Send + 'static> WorkerPool<Job> {
    pub fn new(
        worker_count: usize,
        handler: impl Fn(Job) + Send + Sync + 'static,
    ) -> Result<Self, WorkerPoolError> {
        let (sender, receiver) = mpsc::sync_channel(worker_count);
        let receiver = Arc::new(Mutex::new(receiver));
        let handler = Arc::new(handler);
        let mut pool = Self {
            sender: Some(sender),
            threads: Vec::new(),
        };
        for worker in 0..worker_count {
            let receiver = Arc::clone(&receiver);
            let handler = Arc::clone(&handler);
            pool.threads.push(
                thread::Builder::new()
                    .name(format!("workflow-worker-{worker}"))
                    .spawn(move || {
                        loop {
                            let job = { receiver.lock().unwrap().recv() };
                            let Ok(job) = job else { break };
                            handler(job);
                        }
                    })
                    .context(WorkerPoolSnafu { worker })?,
            );
        }
        Ok(pool)
    }

    pub fn worker_count(&self) -> usize {
        self.threads.len()
    }

    pub fn try_submit(&self, job: Job) -> Result<(), mpsc::TrySendError<Job>> {
        self.sender.as_ref().expect("pool is active").try_send(job)
    }
}

impl<Job> Drop for WorkerPool<Job> {
    fn drop(&mut self) {
        self.sender.take();
        for worker in self.threads.drain(..) {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        collections::HashSet,
        sync::atomic::{AtomicUsize, Ordering},
        time::Duration,
    };

    #[test]
    fn reuses_bounded_workers_and_drains_accepted_jobs_on_drop() {
        let (started, received) = mpsc::channel();
        let (unlock, blocked) = mpsc::channel::<()>();
        let blocked = Mutex::new(blocked);
        let completed = Arc::new(AtomicUsize::new(0));
        let finished = Arc::clone(&completed);
        let pool = WorkerPool::new(2, move |job| {
            started.send((job, thread::current().id())).unwrap();
            let _ = blocked.lock().unwrap().recv();
            finished.fetch_add(1, Ordering::SeqCst);
        })
        .unwrap();
        // Unblock handlers before dropping the pool if an assertion fails.
        let release_workers = unlock;
        assert_eq!(pool.worker_count(), 2);
        pool.try_submit(1).unwrap();
        pool.try_submit(2).unwrap();
        let mut calls = vec![
            received.recv_timeout(Duration::from_secs(5)).unwrap(),
            received.recv_timeout(Duration::from_secs(5)).unwrap(),
        ];
        pool.try_submit(3).unwrap();
        pool.try_submit(4).unwrap();
        assert!(matches!(
            pool.try_submit(5),
            Err(mpsc::TrySendError::Full(5))
        ));
        assert!(matches!(
            received.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
        drop(release_workers);
        drop(pool);
        assert_eq!(completed.load(Ordering::SeqCst), 4);
        calls.extend(received);
        let workers: HashSet<_> = calls.iter().map(|(_, worker)| *worker).collect();
        let mut jobs: Vec<_> = calls.into_iter().map(|(job, _)| job).collect();
        jobs.sort_unstable();
        assert_eq!(workers.len(), 2);
        assert_eq!(jobs, [1, 2, 3, 4]);
    }

    #[test]
    fn zero_workers_return_the_unsubmitted_job() {
        let pool = WorkerPool::new(0, |_: usize| unreachable!()).unwrap();
        assert_eq!(pool.worker_count(), 0);
        assert!(matches!(
            pool.try_submit(7),
            Err(mpsc::TrySendError::Disconnected(7))
        ));
    }
}
