use snafu::{ResultExt, Snafu};
use std::{
    any::Any,
    cell::{Cell, RefCell},
    collections::VecDeque,
    io,
    marker::PhantomData,
    panic::{AssertUnwindSafe, catch_unwind, resume_unwind},
    rc::Rc,
    sync::{Arc, Condvar, Mutex, Weak, mpsc},
    thread::{self, JoinHandle},
};

thread_local! {
    static CURRENT_WORKER: Cell<usize> = const { Cell::new(0) };
    static RESERVED_RESOURCES: RefCell<Vec<WorkerResource>> = const { RefCell::new(Vec::new()) };
}

/// Identity of a serialized executor that a worker job may enter.
#[derive(Clone, Debug, Default)]
pub struct WorkerResource(Arc<()>);

impl WorkerResource {
    /// Reserves the resource before waiting for its executor lock. Reentry is rejected.
    /// The reservation belongs to this thread and must outlive the executor lock guard.
    pub fn reserve(&self) -> Option<WorkerReservation> {
        RESERVED_RESOURCES.with(|reserved| {
            let mut reserved = reserved.borrow_mut();
            if reserved.iter().any(|other| Arc::ptr_eq(&self.0, &other.0)) {
                return None;
            }
            reserved.push(self.clone());
            Some(WorkerReservation {
                resource: self.clone(),
                thread: PhantomData,
            })
        })
    }
}

/// A thread-bound reservation; releasing out of acquisition order is supported.
pub struct WorkerReservation {
    resource: WorkerResource,
    thread: PhantomData<Rc<()>>,
}

impl Drop for WorkerReservation {
    fn drop(&mut self) {
        RESERVED_RESOURCES.with(|reserved| {
            reserved
                .borrow_mut()
                .retain(|other| !Arc::ptr_eq(&self.resource.0, &other.0));
        });
    }
}

fn has_reservations() -> bool {
    RESERVED_RESOURCES.with(|reserved| !reserved.borrow().is_empty())
}

#[derive(Debug, Snafu)]
#[snafu(display("could not start workflow worker {worker}: {source}"))]
pub struct WorkerPoolError {
    pub worker: usize,
    pub source: io::Error,
}

/// Reusable bounded threads with one waiting queue slot per worker.
///
/// The synchronous handler owns result delivery and task panic handling. Dropping the pool closes
/// submissions and joins its threads. A runtime worker can execute queued nested jobs on its current
/// thread while waiting for children. Zero workers accept no jobs.
pub struct WorkerPool<Job> {
    state: Arc<WorkerState<Job>>,
    handler: Arc<WorkerHandler<Job>>,
    threads: Vec<JoinHandle<()>>,
}

type WorkerHandler<Job> = dyn Fn(Job) + Send + Sync + 'static;

struct WorkerQueue<Job> {
    jobs: VecDeque<Job>,
    closed: bool,
}

struct WorkerState<Job> {
    queue: Mutex<WorkerQueue<Job>>,
    available: Condvar,
    space: Condvar,
    capacity: usize,
}

/// A cloneable submission handle that does not keep its worker pool alive.
pub struct WorkerHandle<Job> {
    state: Weak<WorkerState<Job>>,
    handler: Weak<WorkerHandler<Job>>,
    worker_count: usize,
}

impl<Job> Clone for WorkerHandle<Job> {
    fn clone(&self) -> Self {
        Self {
            state: self.state.clone(),
            handler: self.handler.clone(),
            worker_count: self.worker_count,
        }
    }
}

impl<Job: Send + 'static> std::fmt::Debug for WorkerHandle<Job> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkerHandle")
            .field("worker_count", &self.worker_count)
            .field("active", &self.is_active())
            .finish()
    }
}

/// An owned synchronous task for a runtime worker.
pub struct WorkerJob(Box<dyn FnOnce() + Send + 'static>);

impl WorkerJob {
    pub fn new(run: impl FnOnce() + Send + 'static) -> Self {
        Self(Box::new(run))
    }

    pub fn run(self) {
        (self.0)();
    }
}

impl<Job: Send + 'static> WorkerPool<Job> {
    pub fn new(
        worker_count: usize,
        handler: impl Fn(Job) + Send + Sync + 'static,
    ) -> Result<Self, WorkerPoolError> {
        let state = Arc::new(WorkerState {
            queue: Mutex::new(WorkerQueue {
                jobs: VecDeque::new(),
                closed: false,
            }),
            available: Condvar::new(),
            space: Condvar::new(),
            capacity: worker_count,
        });
        let handler: Arc<WorkerHandler<Job>> = Arc::new(handler);
        let mut pool = Self {
            state,
            handler: Arc::clone(&handler),
            threads: Vec::new(),
        };
        for worker in 0..worker_count {
            let state = Arc::clone(&pool.state);
            let handler = Arc::clone(&handler);
            pool.threads.push(
                thread::Builder::new()
                    .name(format!("workflow-worker-{worker}"))
                    .spawn(move || {
                        let worker_id = Arc::as_ptr(&state) as usize;
                        let previous = CURRENT_WORKER.with(|current| current.replace(worker_id));
                        loop {
                            let job = {
                                let mut queue = state.queue.lock().unwrap();
                                while queue.jobs.is_empty() && !queue.closed {
                                    queue = state.available.wait(queue).unwrap();
                                }
                                let job = queue.jobs.pop_front();
                                if job.is_some() {
                                    state.space.notify_one();
                                }
                                job
                            };
                            let Some(job) = job else { break };
                            handler(job);
                        }
                        CURRENT_WORKER.with(|current| current.set(previous));
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
        self.handle().try_submit(job)
    }

    pub fn handle(&self) -> WorkerHandle<Job> {
        WorkerHandle {
            state: Arc::downgrade(&self.state),
            handler: Arc::downgrade(&self.handler),
            worker_count: self.worker_count(),
        }
    }
}

impl<Job> Drop for WorkerPool<Job> {
    fn drop(&mut self) {
        let mut queue = self.state.queue.lock().unwrap();
        queue.closed = true;
        drop(queue);
        self.state.available.notify_all();
        self.state.space.notify_all();
        for worker in self.threads.drain(..) {
            let _ = worker.join();
        }
    }
}

impl<Job: Send + 'static> WorkerHandle<Job> {
    pub fn worker_count(&self) -> usize {
        self.worker_count
    }

    pub fn try_submit(&self, job: Job) -> Result<(), mpsc::TrySendError<Job>> {
        if self.worker_count == 0 {
            return Err(mpsc::TrySendError::Disconnected(job));
        }
        let Some(state) = self.state.upgrade() else {
            return Err(mpsc::TrySendError::Disconnected(job));
        };
        let mut queue = state.queue.lock().unwrap();
        if queue.closed {
            return Err(mpsc::TrySendError::Disconnected(job));
        }
        if queue.jobs.len() >= state.capacity {
            return Err(mpsc::TrySendError::Full(job));
        }
        queue.jobs.push_back(job);
        state.available.notify_one();
        Ok(())
    }

    /// Waits for queue space, helping queued jobs when called by a worker in this pool.
    /// With an active executor reservation, runs accepted children inline instead.
    pub fn submit(&self, job: Job) -> Result<(), mpsc::TrySendError<Job>> {
        if self.worker_count == 0 {
            return Err(mpsc::TrySendError::Disconnected(job));
        }
        let Some(state) = self.state.upgrade() else {
            return Err(mpsc::TrySendError::Disconnected(job));
        };
        let mut queue = state.queue.lock().unwrap();
        if has_reservations() && !queue.closed {
            // Queuing children behind unrelated callers waiting for this executor can deadlock.
            drop(queue);
            self.execute(job);
            return Ok(());
        }
        let mut job = Some(job);
        loop {
            if queue.closed {
                return Err(mpsc::TrySendError::Disconnected(
                    job.take().expect("unsubmitted worker job"),
                ));
            }
            if queue.jobs.len() < state.capacity {
                queue
                    .jobs
                    .push_back(job.take().expect("unsubmitted worker job"));
                state.available.notify_one();
                return Ok(());
            }

            let helper_job = if self.is_current_worker() {
                queue.jobs.pop_front()
            } else {
                None
            };
            if let Some(helper_job) = helper_job {
                state.space.notify_one();
                drop(queue);
                self.execute(helper_job);
                queue = state.queue.lock().unwrap();
            } else {
                queue = state.space.wait(queue).unwrap();
            }
        }
    }

    fn is_current_worker(&self) -> bool {
        let Some(state) = self.state.upgrade() else {
            return false;
        };
        let worker_id = Arc::as_ptr(&state) as usize;
        CURRENT_WORKER.with(|current| current.get() == worker_id)
    }

    /// Executes one queued job on the current worker unless it holds an executor reservation.
    /// Callers outside this pool do no work.
    pub fn help_one(&self) -> bool {
        if has_reservations() || !self.is_current_worker() {
            return false;
        }
        let Some(state) = self.state.upgrade() else {
            return false;
        };
        let job = {
            let mut queue = state.queue.lock().unwrap();
            let job = queue.jobs.pop_front();
            if job.is_some() {
                state.space.notify_one();
            }
            job
        };
        if let Some(job) = job {
            self.execute(job);
            true
        } else {
            false
        }
    }

    fn execute(&self, job: Job) {
        if let Some(handler) = self.handler.upgrade() {
            handler(job);
        }
    }

    pub fn is_active(&self) -> bool {
        let Some(state) = self.state.upgrade() else {
            return false;
        };
        !state.queue.lock().unwrap().closed
    }
}

impl WorkerHandle<WorkerJob> {
    /// Runs tasks in the pool and returns results in submission order.
    ///
    /// All tasks finish before a panic is resumed. An inactive pool runs tasks on the caller.
    pub fn run_parallel<T, F>(&self, tasks: Vec<F>) -> Vec<T>
    where
        T: Send + 'static,
        F: FnOnce() -> T + Send + 'static,
    {
        let count = tasks.len();
        if count == 0 {
            return Vec::new();
        }
        let (sender, receiver) = mpsc::channel();
        for (index, task) in tasks.into_iter().enumerate() {
            let sender = sender.clone();
            let job = WorkerJob::new(move || {
                let result = catch_unwind(AssertUnwindSafe(task));
                let _ = sender.send((index, result));
            });
            match self.submit(job) {
                Ok(()) => {}
                Err(mpsc::TrySendError::Disconnected(job)) => job.run(),
                Err(mpsc::TrySendError::Full(_)) => unreachable!("blocking submit returned full"),
            }
        }
        drop(sender);

        let mut results: Vec<Option<Result<T, Box<dyn Any + Send>>>> =
            std::iter::repeat_with(|| None).take(count).collect();
        let mut completed = 0;
        while completed < count {
            let (index, result) = match receiver.try_recv() {
                Ok(result) => result,
                Err(mpsc::TryRecvError::Empty) if self.help_one() => continue,
                _ => receiver
                    .recv()
                    .expect("every submitted worker task reports completion"),
            };
            results[index] = Some(result);
            completed += 1;
        }

        results
            .into_iter()
            .map(
                |result| match result.expect("every worker task completes") {
                    Ok(value) => value,
                    Err(payload) => resume_unwind(payload),
                },
            )
            .collect()
    }
}

pub type RuntimeWorkerHandle = WorkerHandle<WorkerJob>;

/// Owned by the outer execution; contexts only retain weak references so queued
/// jobs cannot keep the pool alive or join their own worker during shutdown.
pub struct ExecutionWorkers {
    pool: Mutex<Option<WorkerPool<WorkerJob>>>,
    limit: usize,
}

impl ExecutionWorkers {
    pub fn new(limit: usize) -> Self {
        Self {
            pool: Mutex::new(None),
            limit,
        }
    }

    pub fn handle(&self) -> Option<RuntimeWorkerHandle> {
        self.pool.lock().unwrap().as_ref().map(WorkerPool::handle)
    }

    pub fn initialize(&self) -> Result<RuntimeWorkerHandle, WorkerPoolError> {
        let mut pool = self.pool.lock().unwrap();
        if pool.is_none() {
            *pool = Some(WorkerPool::new(self.limit, WorkerJob::run)?);
        }
        Ok(pool.as_ref().unwrap().handle())
    }
}

impl std::fmt::Debug for ExecutionWorkers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExecutionWorkers")
            .field("limit", &self.limit)
            .field("handle", &self.handle())
            .finish()
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
    fn reservations_reject_reentry_and_release_on_unwind() {
        let first = WorkerResource::default();
        let second = WorkerResource::default();
        let first_guard = first.reserve().unwrap();
        let second_guard = second.reserve().unwrap();
        assert!(first.clone().reserve().is_none());
        drop(first_guard);
        assert!(first.reserve().is_some());
        assert!(second.reserve().is_none());
        drop(second_guard);
        let result = catch_unwind(|| {
            let _guard = second.reserve().unwrap();
            panic!("release reservation");
        });
        assert!(result.is_err());
        assert!(second.reserve().is_some());
    }

    #[test]
    fn reserved_worker_runs_children_inline_without_helping_unrelated_jobs() {
        let pool = WorkerPool::new(1, WorkerJob::run).unwrap();
        let handle = pool.handle();
        let nested = handle.clone();
        let (sent, received) = mpsc::channel();
        handle
            .submit(WorkerJob::new(move || {
                let resource = WorkerResource::default();
                let _guard = resource.reserve().unwrap();
                let queued = sent.clone();
                nested
                    .try_submit(WorkerJob::new(move || queued.send("queued").unwrap()))
                    .unwrap();
                assert!(!nested.help_one());
                let parent_thread = thread::current().id();
                let threads =
                    nested.run_parallel(vec![|| thread::current().id(), || thread::current().id()]);
                assert_eq!(threads, [parent_thread, parent_thread]);
                sent.send("parent").unwrap();
            }))
            .unwrap();
        assert_eq!(
            received.recv_timeout(Duration::from_secs(5)).unwrap(),
            "parent"
        );
        assert_eq!(
            received.recv_timeout(Duration::from_secs(5)).unwrap(),
            "queued"
        );
    }

    #[test]
    fn reserved_external_caller_runs_children_without_pool_capacity() {
        let pool = WorkerPool::new(1, WorkerJob::run).unwrap();
        let (started, received) = mpsc::channel();
        let (release, released) = mpsc::channel::<()>();
        pool.try_submit(WorkerJob::new(move || {
            started.send(()).unwrap();
            let _ = released.recv();
        }))
        .unwrap();
        received.recv_timeout(Duration::from_secs(5)).unwrap();
        let _release = release;
        let resource = WorkerResource::default();
        let guard = resource.reserve().unwrap();
        let parent_thread = thread::current().id();
        assert_eq!(
            pool.handle().run_parallel(vec![|| thread::current().id()]),
            [parent_thread]
        );
        drop(guard);
        assert!(!has_reservations());
    }

    #[test]
    fn two_reserved_workers_do_not_form_a_cross_instance_help_cycle() {
        let pool = WorkerPool::new(2, WorkerJob::run).unwrap();
        let barriers = Arc::new(std::sync::Barrier::new(2));
        let locks = [Arc::new(Mutex::new(())), Arc::new(Mutex::new(()))];
        let (sent, received) = mpsc::channel();
        for index in 0..2 {
            let barriers = barriers.clone();
            let owned = locks[index].clone();
            let other = locks[1 - index].clone();
            let handle = pool.handle();
            let sent = sent.clone();
            pool.handle()
                .submit(WorkerJob::new(move || {
                    let resource = WorkerResource::default();
                    let _reservation = resource.reserve().unwrap();
                    let _lock = owned.lock().unwrap();
                    barriers.wait();
                    let queued = sent.clone();
                    handle
                        .try_submit(WorkerJob::new(move || {
                            let _lock = other.lock().unwrap();
                            queued.send("queued").unwrap();
                        }))
                        .unwrap();
                    barriers.wait();
                    assert!(!handle.help_one());
                    assert_eq!(handle.run_parallel(vec![|| 1, || 2]), [1, 2]);
                    sent.send("parent").unwrap();
                }))
                .unwrap();
        }
        drop(sent);
        let mut events = Vec::new();
        for _ in 0..4 {
            events.push(received.recv_timeout(Duration::from_secs(5)).unwrap());
        }
        events.sort_unstable();
        assert_eq!(events, ["parent", "parent", "queued", "queued"]);
    }

    #[test]
    fn reservations_are_thread_local() {
        let resource = WorkerResource::default();
        let _guard = resource.reserve().unwrap();
        assert!(has_reservations());
        thread::spawn(move || {
            assert!(!has_reservations());
            assert!(resource.reserve().is_some());
        })
        .join()
        .unwrap();
    }

    #[test]
    fn concurrent_initialization_shares_one_execution_pool() {
        let workers = ExecutionWorkers::new(4);
        assert!(workers.handle().is_none());
        let starts = std::sync::Barrier::new(4);
        let handles = thread::scope(|scope| {
            let starts = &starts;
            let workers = &workers;
            let jobs: Vec<_> = (0..4)
                .map(|_| {
                    scope.spawn(move || {
                        starts.wait();
                        workers.initialize().unwrap()
                    })
                })
                .collect();
            jobs.into_iter()
                .map(|job| job.join().unwrap())
                .collect::<Vec<_>>()
        });
        assert!(
            handles
                .iter()
                .all(|handle| handle.state.ptr_eq(&handles[0].state))
        );
        drop(workers);
        assert!(handles.iter().all(|handle| !handle.is_active()));
    }

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

    #[test]
    fn a_waiting_worker_helps_nested_work_on_its_existing_thread() {
        let pool = WorkerPool::new(2, WorkerJob::run).unwrap();
        let (blocker_started, started) = mpsc::channel();
        let (release_blocker, blocker_released) = mpsc::channel();
        pool.try_submit(WorkerJob::new(move || {
            blocker_started.send(()).unwrap();
            let _ = blocker_released.recv();
        }))
        .unwrap();
        started.recv_timeout(Duration::from_secs(2)).unwrap();

        let (child_ran, child_thread) = mpsc::channel();
        let (completed, completion) = mpsc::channel();
        let handle = pool.handle();
        pool.try_submit(WorkerJob::new(move || {
            let parent_thread = thread::current().id();
            let result = handle.run_parallel(vec![move || {
                child_ran.send(thread::current().id()).unwrap();
                42
            }]);
            completed.send((parent_thread, result)).unwrap();
        }))
        .unwrap();

        let child_thread = match child_thread.recv_timeout(Duration::from_millis(250)) {
            Ok(thread) => thread,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                release_blocker.send(()).unwrap();
                child_thread.recv_timeout(Duration::from_secs(2)).unwrap()
            }
            Err(error) => panic!("nested job did not run: {error}"),
        };
        let _ = release_blocker.send(());
        let (parent_thread, result) = completion.recv_timeout(Duration::from_secs(2)).unwrap();

        assert_eq!(child_thread, parent_thread);
        assert_eq!(result, [42]);
        assert_eq!(pool.worker_count(), 2);
    }

    #[test]
    fn parallel_jobs_drain_before_resuming_a_panic() {
        let pool = WorkerPool::new(2, WorkerJob::run).unwrap();
        let completed = Arc::new(AtomicUsize::new(0));
        let finished = Arc::clone(&completed);
        let jobs: Vec<Box<dyn FnOnce() + Send>> = vec![
            Box::new(|| panic!("worker task panicked")),
            Box::new(move || {
                thread::sleep(Duration::from_millis(10));
                finished.fetch_add(1, Ordering::SeqCst);
            }),
        ];
        let result = catch_unwind(AssertUnwindSafe(|| pool.handle().run_parallel(jobs)));

        assert!(result.is_err());
        assert_eq!(completed.load(Ordering::SeqCst), 1);
    }
}
