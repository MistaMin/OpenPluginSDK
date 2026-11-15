//! Load heavy things off the audio thread.
//!
//! Impulse responses, samples and tables are too big to build inside
//! `process`. This module gives a plugin one safe pattern for them:
//!
//! 1. The main (UI) thread calls [`Loader::submit`] with a closure that builds
//!    the new value. The closure runs on a background thread owned by the
//!    [`Loader`].
//! 2. The finished value waits in a bounded lock-free queue.
//! 3. The audio thread calls [`Slot::poll`] at the top of each block. If a new
//!    value is ready it is swapped in with a few pointer-sized moves. The old
//!    value goes back to the worker thread through a second queue and is
//!    dropped there.
//!
//! On the audio thread nothing locks, allocates, frees, parks or makes a
//! system call: [`Slot::poll`], [`Slot::get`] and [`Slot::get_mut`] only do
//! atomic loads and stores and move values by value.
//!
//! # Limits and failure
//!
//! * Every queue is bounded. [`Loader::submit`] returns [`QueueFull`] instead
//!   of blocking when too many jobs are waiting. If the worker cannot hand
//!   over a result because the audio thread has not collected the earlier
//!   ones, the worker waits (it never drops or allocates on the audio side).
//! * If the audio thread cannot hand an old value back because the return
//!   queue is full, [`Slot::poll`] keeps the old value and does not swap yet.
//!   It tries again on the next call. It never drops the value itself.
//! * A job that panics is counted in [`Loader::failed`]. The current value is
//!   kept and the worker keeps running.
//!
//! # Where to drop things
//!
//! A [`Slot`] owns the live value, so dropping the `Slot` drops that value on
//! whatever thread drops it. Drop it from the main thread (for example when
//! the plugin instance is destroyed), not from `process`.
//!
//! # Running jobs on a host's thread
//!
//! A format entry can supply its own worker thread through [`Executor`] (the
//! LV2 entry hands jobs to the host's `worker` extension this way). The entry
//! wraps plugin construction in [`with_executor`]; a [`loader`] created inside
//! it runs its jobs on the executor instead of starting a thread, and falls
//! back to its own thread if the executor refuses. The plugin's code does not
//! change. Create the loader in the constructor (or `prepare`), not later.
//! The audio side then calls [`Waker::wake`] after a submit and after a swap,
//! which is a lock-free atomic update; the executor decides when to run the
//! task. `submit` still boxes the closure on the calling thread.
//!
//! # Example
//!
//! ```
//! use openplugin_core::worker::loader;
//!
//! // The audio side starts with a silent table; the loader builds new ones.
//! let (mut loader, mut slot) = loader::<Vec<f32>>(vec![0.0; 4], 4);
//! loader.submit(|| vec![1.0, 2.0, 3.0, 4.0]).unwrap();
//! loader.wait_idle(std::time::Duration::from_secs(5));
//!
//! // Audio thread, top of `process`:
//! assert!(slot.poll());
//! assert_eq!(slot.get(), &[1.0, 2.0, 3.0, 4.0]);
//! ```

use crate::spsc;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread::{Builder, JoinHandle, Thread};
use std::time::{Duration, Instant};

/// How long the worker sleeps when it has nothing to do. It also bounds how
/// long a retired value waits before it is dropped, because the audio thread
/// does not wake the worker (waking would be a system call).
const IDLE: Duration = Duration::from_millis(5);

/// A job could not be queued because the queue is full. Nothing was queued.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueueFull;

impl std::fmt::Display for QueueFull {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("worker job queue is full")
    }
}

impl std::error::Error for QueueFull {}

type Job<T> = Box<dyn FnOnce() -> T + Send + 'static>;

/// What one call of [`Task::work`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Nothing to run (old values were freed, if any were waiting).
    Idle,
    /// A job ran and its result is in the queue for the audio thread.
    Done,
    /// Work is left over: a result waits for room in the queue. Call again.
    Retry,
}

/// A loader's background half, for an [`Executor`] to run on its own thread.
pub trait Task: Send {
    /// Runs at most one job, or hands over a result that was waiting for
    /// room. Not for the audio thread: it may run a long job. One thread at
    /// a time.
    fn work(&mut self) -> Step;
}

/// Wakes a task. Callable from any thread, including the audio thread.
pub trait Waker: Send + Sync {
    /// Asks the executor to call [`Task::work`] once more. Lock-free and
    /// allocation-free.
    fn wake(&self);
    /// The loader is gone: stop running the task and drop it. Waits for a
    /// running call to finish. Not for the audio thread.
    fn detach(&self);
}

/// A way to run loader tasks other than on a thread of their own.
pub trait Executor: Send + Sync {
    /// Takes over `task`. An `Err` hands the task back (no room, say); the
    /// loader then starts its own thread for it.
    fn spawn(&self, task: Box<dyn Task>) -> Result<Arc<dyn Waker>, Box<dyn Task>>;
}

thread_local! {
    static AMBIENT: RefCell<Option<Arc<dyn Executor>>> = const { RefCell::new(None) };
}

/// Runs `f` with `executor` as the one new loaders on this thread use.
pub fn with_executor<R>(executor: Arc<dyn Executor>, f: impl FnOnce() -> R) -> R {
    struct Restore(Option<Arc<dyn Executor>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            let previous = self.0.take();
            AMBIENT.with(|a| *a.borrow_mut() = previous);
        }
    }
    let _restore = Restore(AMBIENT.with(|a| a.borrow_mut().replace(executor)));
    f()
}

fn ambient() -> Option<Arc<dyn Executor>> {
    AMBIENT.with(|a| a.borrow().clone())
}

#[derive(Default)]
struct Counters {
    submitted: AtomicUsize,
    /// Jobs that finished, whether they succeeded or panicked.
    finished: AtomicUsize,
    failed: AtomicUsize,
    stop: AtomicBool,
}

/// The main-thread half: submits jobs to the background worker.
///
/// Dropping the `Loader` stops and joins the worker. Jobs not yet started are
/// dropped without running.
pub struct Loader<T: Send + 'static> {
    jobs: spsc::Producer<Job<T>>,
    counters: Arc<Counters>,
    backend: Backend,
}

enum Backend {
    Thread { thread: Thread, handle: Option<JoinHandle<()>> },
    Host(Arc<dyn Waker>),
}

/// The audio-thread half: owns the live value and picks up new ones.
pub struct Slot<T: Send + 'static> {
    current: T,
    ready: spsc::Consumer<T>,
    retired: spsc::Producer<T>,
    /// An old value the return queue had no room for yet.
    stalled: Option<T>,
    /// Set when an executor runs the task: told after an old value is retired.
    waker: Option<Arc<dyn Waker>>,
}

/// Starts a worker thread and returns both halves.
///
/// `initial` is the value used until the first job finishes. `capacity` is the
/// size of each of the three bounded queues (jobs, finished results, returned
/// old values) and must be at least 1.
///
/// # Panics
///
/// Panics if `capacity` is 0 or the operating system cannot start a thread.
pub fn loader<T: Send + 'static>(initial: T, capacity: usize) -> (Loader<T>, Slot<T>) {
    assert!(capacity > 0, "worker capacity must be non-zero");
    let (job_tx, job_rx) = spsc::channel::<Job<T>>(capacity);
    let (ready_tx, ready_rx) = spsc::channel::<T>(capacity);
    let (retired_tx, retired_rx) = spsc::channel::<T>(capacity);
    let counters = Arc::new(Counters::default());
    let core: Box<dyn Task> = Box::new(Core {
        jobs: job_rx,
        ready: ready_tx,
        retired: retired_rx,
        waiting: None,
        counters: counters.clone(),
    });
    let hosted = match ambient() {
        Some(executor) => executor.spawn(core),
        None => Err(core),
    };
    let (backend, waker) = match hosted {
        Ok(waker) => (Backend::Host(waker.clone()), Some(waker)),
        Err(core) => {
            let stop = counters.clone();
            let handle = Builder::new()
                .name("openplugin-worker".into())
                .spawn(move || run(core, stop))
                .expect("could not start the openplugin worker thread");
            let thread = handle.thread().clone();
            (Backend::Thread { thread, handle: Some(handle) }, None)
        }
    };
    (
        Loader {
            jobs: job_tx,
            counters,
            backend,
        },
        Slot {
            current: initial,
            ready: ready_rx,
            retired: retired_tx,
            stalled: None,
            waker,
        },
    )
}

/// The loader's background half: runs jobs and frees retired values.
struct Core<T: Send + 'static> {
    jobs: spsc::Consumer<Job<T>>,
    ready: spsc::Producer<T>,
    retired: spsc::Consumer<T>,
    /// A finished result that the audio thread has not made room for yet.
    waiting: Option<T>,
    counters: Arc<Counters>,
}

impl<T: Send + 'static> Task for Core<T> {
    fn work(&mut self) -> Step {
        // Old values come back here to be freed off the audio thread.
        while let Some(old) = self.retired.pop() {
            drop(old);
        }
        if let Some(result) = self.waiting.take() {
            return match self.ready.push(result) {
                Ok(()) => Step::Done,
                Err(result) => {
                    self.waiting = Some(result);
                    Step::Retry
                }
            };
        }
        let Some(job) = self.jobs.pop() else { return Step::Idle };
        match catch_unwind(AssertUnwindSafe(job)) {
            Ok(value) => {
                if let Err(value) = self.ready.push(value) {
                    self.waiting = Some(value);
                }
            }
            Err(_) => {
                self.counters.failed.fetch_add(1, Ordering::Release);
            }
        }
        self.counters.finished.fetch_add(1, Ordering::Release);
        if self.waiting.is_some() { Step::Retry } else { Step::Done }
    }
}

impl<T: Send + 'static> Drop for Core<T> {
    fn drop(&mut self) {
        while let Some(old) = self.retired.pop() {
            drop(old);
        }
    }
}

/// The worker thread: runs the task until the loader stops it.
fn run(mut task: Box<dyn Task>, counters: Arc<Counters>) {
    while !counters.stop.load(Ordering::Acquire) {
        if task.work() != Step::Done {
            std::thread::park_timeout(IDLE);
        }
    }
}

impl<T: Send + 'static> Loader<T> {
    /// Queues `job` to run on the worker thread. Call from the main thread.
    ///
    /// Returns [`QueueFull`] and queues nothing when the job queue is full.
    /// The value the job returns replaces the live value the next time the
    /// audio thread calls [`Slot::poll`]. Jobs run in order; when several
    /// results are waiting, the audio thread applies them one per poll.
    pub fn submit<F>(&mut self, job: F) -> Result<(), QueueFull>
    where
        F: FnOnce() -> T + Send + 'static,
    {
        // Count before queueing so `pending` never undercounts a queued job.
        self.counters.submitted.fetch_add(1, Ordering::AcqRel);
        match self.jobs.push(Box::new(job)) {
            Ok(()) => {
                match &self.backend {
                    Backend::Thread { thread, .. } => thread.unpark(),
                    Backend::Host(waker) => waker.wake(),
                }
                Ok(())
            }
            Err(_) => {
                self.counters.submitted.fetch_sub(1, Ordering::AcqRel);
                Err(QueueFull)
            }
        }
    }

    /// Jobs submitted and not yet finished running (queued or running).
    pub fn pending(&self) -> usize {
        let finished = self.counters.finished.load(Ordering::Acquire);
        self.counters
            .submitted
            .load(Ordering::Acquire)
            .saturating_sub(finished)
    }

    /// Jobs that panicked. Their results were discarded.
    pub fn failed(&self) -> usize {
        self.counters.failed.load(Ordering::Acquire)
    }

    /// Waits until no job is queued or running, or `timeout` passes. Returns
    /// true when idle. A finished result may still be waiting for the audio
    /// thread. For tests and shutdown, not for the audio thread.
    pub fn wait_idle(&self, timeout: Duration) -> bool {
        let start = Instant::now();
        while self.pending() > 0 {
            if start.elapsed() >= timeout {
                return false;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        true
    }
}

impl<T: Send + 'static> Drop for Loader<T> {
    fn drop(&mut self) {
        self.counters.stop.store(true, Ordering::Release);
        match &mut self.backend {
            Backend::Thread { thread, handle } => {
                thread.unpark();
                if let Some(handle) = handle.take() {
                    let _ = handle.join();
                }
            }
            Backend::Host(waker) => waker.detach(),
        }
    }
}

impl<T: Send + 'static> Slot<T> {
    /// Takes the newest finished value if there is one. Call once at the top
    /// of each block, on the audio thread. Returns true when the live value
    /// changed. Does not lock, allocate or free.
    ///
    /// At most one value is applied per call. If the old value cannot be
    /// handed back yet (return queue full), the swap waits for a later call.
    pub fn poll(&mut self) -> bool {
        if let Some(old) = self.stalled.take() {
            if let Err(old) = self.retired.push(old) {
                self.stalled = Some(old);
                return false;
            }
            self.wake();
        }
        match self.ready.pop() {
            Some(new) => {
                let old = std::mem::replace(&mut self.current, new);
                match self.retired.push(old) {
                    Ok(()) => self.wake(),
                    Err(old) => self.stalled = Some(old),
                }
                true
            }
            None => false,
        }
    }

    /// Asks an executor-run task to free what was just retired. A no-op when
    /// the loader has its own thread (it polls on a timer).
    fn wake(&self) {
        if let Some(waker) = &self.waker {
            waker.wake();
        }
    }

    /// The live value.
    pub fn get(&self) -> &T {
        &self.current
    }

    /// The live value, mutably. Use it to carry state (a convolution history,
    /// say) alongside the loaded data.
    pub fn get_mut(&mut self) -> &mut T {
        &mut self.current
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::thread::ThreadId;

    const WAIT: Duration = Duration::from_secs(10);

    fn settle<T: Send + 'static>(loader: &Loader<T>) {
        assert!(loader.wait_idle(WAIT));
    }

    #[test]
    fn initial_value_until_a_job_finishes() {
        let (_loader, mut slot) = loader(7u32, 2);
        assert_eq!(*slot.get(), 7);
        assert!(!slot.poll());
        assert_eq!(*slot.get(), 7);
    }

    #[test]
    fn job_result_is_picked_up() {
        let (mut loader, mut slot) = loader(0u32, 2);
        loader.submit(|| 42).unwrap();
        settle(&loader);
        // The result is in the ready queue by now or very soon after.
        let start = Instant::now();
        while !slot.poll() {
            assert!(start.elapsed() < WAIT);
            std::thread::yield_now();
        }
        assert_eq!(*slot.get(), 42);
        assert!(!slot.poll());
    }

    #[test]
    fn jobs_apply_in_order_one_per_poll() {
        let (mut loader, mut slot) = loader(0u32, 4);
        for n in 1..=3 {
            loader.submit(move || n).unwrap();
        }
        settle(&loader);
        let mut seen = Vec::new();
        let start = Instant::now();
        while seen.len() < 3 {
            assert!(start.elapsed() < WAIT);
            if slot.poll() {
                seen.push(*slot.get());
            }
        }
        assert_eq!(seen, [1, 2, 3]);
    }

    #[test]
    fn full_job_queue_reports_failure() {
        let (mut loader, _slot) = loader(0u32, 1);
        let (gate_tx, gate_rx) = std::sync::mpsc::channel::<()>();
        // Occupy the worker so the queue cannot drain.
        loader
            .submit(move || {
                let _ = gate_rx.recv();
                1
            })
            .unwrap();
        // Wait until the worker has taken the first job off the queue.
        let start = Instant::now();
        loop {
            if loader.submit(|| 2).is_ok() {
                break;
            }
            assert!(start.elapsed() < WAIT);
            std::thread::yield_now();
        }
        // One running, one queued, capacity 1: the next is refused.
        assert_eq!(loader.submit(|| 3), Err(QueueFull));
        assert_eq!(loader.pending(), 2);
        gate_tx.send(()).unwrap();
    }

    #[test]
    fn panicking_job_is_counted_and_worker_survives() {
        let (mut loader, mut slot) = loader(0u32, 2);
        loader.submit(|| panic!("boom")).unwrap();
        loader.submit(|| 5).unwrap();
        settle(&loader);
        assert_eq!(loader.failed(), 1);
        let start = Instant::now();
        while !slot.poll() {
            assert!(start.elapsed() < WAIT);
        }
        assert_eq!(*slot.get(), 5);
    }

    struct Tracked {
        audio: Arc<std::sync::Mutex<Option<ThreadId>>>,
        on_audio: Arc<AtomicUsize>,
        total: Arc<AtomicUsize>,
    }

    impl Drop for Tracked {
        fn drop(&mut self) {
            self.total.fetch_add(1, Ordering::SeqCst);
            if *self.audio.lock().unwrap() == Some(std::thread::current().id()) {
                self.on_audio.fetch_add(1, Ordering::SeqCst);
            }
        }
    }

    #[test]
    fn old_values_drop_on_the_worker_not_the_audio_thread() {
        let audio = Arc::new(std::sync::Mutex::new(Some(std::thread::current().id())));
        let on_audio = Arc::new(AtomicUsize::new(0));
        let total = Arc::new(AtomicUsize::new(0));
        let make = {
            let (a, o, t) = (audio.clone(), on_audio.clone(), total.clone());
            move || Tracked {
                audio: a.clone(),
                on_audio: o.clone(),
                total: t.clone(),
            }
        };
        let (mut loader, mut slot) = loader(make(), 4);
        let swaps = 50;
        let mut done = 0;
        while done < swaps {
            let m = make.clone();
            if loader.submit(m).is_err() {
                std::thread::yield_now();
            } else {
                done += 1;
            }
            slot.poll();
        }
        settle(&loader);
        let start = Instant::now();
        while loader.pending() > 0 || slot.poll() {
            assert!(start.elapsed() < WAIT);
        }
        // Keep polling until every result has been applied.
        let start = Instant::now();
        while total.load(Ordering::SeqCst) < swaps && start.elapsed() < WAIT {
            slot.poll();
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(on_audio.load(Ordering::SeqCst), 0);
        assert!(total.load(Ordering::SeqCst) >= swaps - 4);
    }

    #[test]
    fn full_return_queue_keeps_old_value_instead_of_dropping() {
        // Capacity 1 and a worker that is busy: the second swap cannot return
        // its old value, so it waits.
        let (mut loader, mut slot) = loader(0u32, 1);
        let (gate_tx, gate_rx) = std::sync::mpsc::channel::<()>();
        loader.submit(|| 1).unwrap();
        let start = Instant::now();
        while !slot.poll() {
            assert!(start.elapsed() < WAIT);
        }
        // Park the worker inside a job so it cannot drain `retired`.
        loader
            .submit(move || {
                let _ = gate_rx.recv();
                2
            })
            .unwrap();
        // Worker may not have drained the first old value yet; either way,
        // a poll never loses or drops a value and never panics.
        for _ in 0..1000 {
            slot.poll();
        }
        gate_tx.send(()).unwrap();
        settle(&loader);
        let start = Instant::now();
        while *slot.get() != 2 {
            slot.poll();
            assert!(start.elapsed() < WAIT);
        }
    }

    #[test]
    fn drop_stops_the_worker_with_jobs_queued() {
        let (mut loader, slot) = loader(0u32, 8);
        for n in 0..8 {
            let _ = loader.submit(move || n);
        }
        drop(loader);
        drop(slot);
    }

    /// Keeps tasks for the test to run by hand, like a host's worker would.
    struct Manual {
        tasks: Tasks,
        wakes: Arc<AtomicUsize>,
        accept: bool,
    }

    struct CountWake(Arc<AtomicUsize>, Arc<AtomicBool>);

    impl Waker for CountWake {
        fn wake(&self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
        fn detach(&self) {
            self.1.store(true, Ordering::SeqCst);
        }
    }

    impl Executor for Manual {
        fn spawn(&self, task: Box<dyn Task>) -> Result<Arc<dyn Waker>, Box<dyn Task>> {
            if !self.accept {
                return Err(task);
            }
            self.tasks.lock().unwrap().push(task);
            Ok(Arc::new(CountWake(self.wakes.clone(), Arc::new(AtomicBool::new(false)))))
        }
    }

    type Tasks = Arc<std::sync::Mutex<Vec<Box<dyn Task>>>>;

    fn manual(accept: bool) -> (Arc<Manual>, Tasks, Arc<AtomicUsize>) {
        let tasks = Arc::new(std::sync::Mutex::new(Vec::new()));
        let wakes = Arc::new(AtomicUsize::new(0));
        (Arc::new(Manual { tasks: tasks.clone(), wakes: wakes.clone(), accept }), tasks, wakes)
    }

    #[test]
    fn executor_runs_the_jobs_and_is_woken() {
        let (exec, tasks, wakes) = manual(true);
        let (mut loader, mut slot) = with_executor(exec, || loader(0u32, 2));
        loader.submit(|| 9).unwrap();
        assert_eq!(wakes.load(Ordering::SeqCst), 1);
        // No thread of our own: nothing happens until the executor runs the task.
        std::thread::sleep(Duration::from_millis(20));
        assert!(!slot.poll());
        assert_eq!(tasks.lock().unwrap()[0].work(), Step::Done);
        assert!(slot.poll());
        assert_eq!(*slot.get(), 9);
        // The swap retired the old value and woke the task to free it.
        assert_eq!(wakes.load(Ordering::SeqCst), 2);
        assert_eq!(tasks.lock().unwrap()[0].work(), Step::Idle);
        assert_eq!(loader.pending(), 0);
    }

    #[test]
    fn refusing_executor_falls_back_to_a_thread() {
        let (exec, tasks, _) = manual(false);
        let (mut loader, mut slot) = with_executor(exec, || loader(0u32, 2));
        assert!(tasks.lock().unwrap().is_empty());
        loader.submit(|| 4).unwrap();
        settle(&loader);
        let start = Instant::now();
        while !slot.poll() {
            assert!(start.elapsed() < WAIT);
        }
        assert_eq!(*slot.get(), 4);
        // The executor applies only inside `with_executor`.
        assert!(ambient().is_none());
    }

    #[test]
    fn full_ready_queue_asks_for_a_retry() {
        let (exec, tasks, _) = manual(true);
        let (mut loader, mut slot) = with_executor(exec, || loader(0u32, 1));
        loader.submit(|| 1).unwrap();
        assert_eq!(tasks.lock().unwrap()[0].work(), Step::Done);
        loader.submit(|| 2).unwrap();
        // The first result is still queued, so the second has nowhere to go.
        assert_eq!(tasks.lock().unwrap()[0].work(), Step::Retry);
        assert!(slot.poll());
        assert_eq!(tasks.lock().unwrap()[0].work(), Step::Done);
        assert!(slot.poll());
        assert_eq!(*slot.get(), 2);
    }
}
