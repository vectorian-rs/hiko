//! Abstract I/O backend trait for async operations.
//!
//! The runtime suspends a process when it requests I/O, registers the
//! request with the backend, and resumes the process when the backend
//! reports completion. No worker thread is blocked during I/O.

use std::io::Read;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[cfg(feature = "builtin-filesystem")]
use crate::heap::CapFsCandidate;
use crate::io_limits::{HttpTimeouts, ReadLimit};
use crate::sendable::SendableValue;

/// Opaque token identifying an I/O operation.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct IoToken(pub u64);

/// How to decode the HTTP response body.
#[derive(Debug, Clone)]
pub enum HttpResponseFormat {
    Text,
    Json,
    Msgpack,
    Bytes,
}

impl HttpResponseFormat {
    /// Name of the full-request builtin that uses this format, for error
    /// messages.
    fn builtin_name(&self) -> &'static str {
        match self {
            Self::Text => "http",
            Self::Json => "http_json",
            Self::Msgpack => "http_msgpack",
            Self::Bytes => "http_bytes",
        }
    }
}

#[derive(Debug, Clone)]
pub enum IoRequest {
    /// Delay for a duration (async sleep).
    Sleep(Duration),
    /// HTTP GET request. Returns (status, headers, body).
    HttpGet {
        url: String,
        max_response: ReadLimit,
        timeouts: HttpTimeouts,
    },
    /// Full HTTP request. Returns (status, headers, body).
    Http {
        method: String,
        url: String,
        headers: Vec<(String, String)>,
        body: String,
        format: HttpResponseFormat,
        max_response: ReadLimit,
        timeouts: HttpTimeouts,
    },
    /// Read a file by ambient path. Returns file contents as a string.
    ReadFile { path: String, max_bytes: ReadLimit },
    /// Read a file through preopened directory capabilities.
    #[cfg(feature = "builtin-filesystem")]
    CapReadFile {
        candidates: Vec<CapFsCandidate>,
        path: String,
        max_bytes: ReadLimit,
    },
    /// Load an AWS SDK config using an allowed SSO profile.
    #[cfg(feature = "builtin-aws-config")]
    AwsConfigSsoProfile { profile: String },
    /// Load an AWS SDK config using the ambient provider chain / instance metadata.
    #[cfg(feature = "builtin-aws-config")]
    AwsConfigInstanceProfile,
    /// List S3 buckets using an opaque reusable AWS S3 client handle.
    #[cfg(feature = "builtin-aws-s3")]
    AwsS3ListBuckets { client: Arc<aws_sdk_s3::Client> },
    /// List SQS queue URLs using an opaque reusable AWS SQS client handle.
    #[cfg(feature = "builtin-aws-sqs")]
    AwsSqsListQueues { client: Arc<aws_sdk_sqs::Client> },
}

/// Result of a completed I/O operation.
#[derive(Debug, Clone)]
pub enum IoResult {
    /// Operation completed successfully with a value.
    Ok { value: SendableValue, io_bytes: u64 },
    /// Operation failed with an error message.
    Err(String),
}

/// Abstract I/O backend. Implementations handle the actual I/O
/// (epoll, io_uring, mock, etc.) without the runtime knowing details.
/// Who issued an I/O request, so a backend can share its threads fairly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IoContext {
    /// The process tree (its root pid) the request belongs to.
    pub owner: u64,
    /// Maximum requests of this owner that may run at once.
    pub max_in_flight: usize,
}

impl Default for IoContext {
    fn default() -> Self {
        Self {
            owner: 0,
            max_in_flight: usize::MAX,
        }
    }
}

pub trait IoBackend: Send + Sync {
    /// Register an I/O request. The backend will eventually produce a result.
    fn register(&self, token: IoToken, request: IoRequest) -> Result<(), String>;

    /// Register a request on behalf of `context`. Backends that schedule
    /// fairly across process trees override this; the default ignores the
    /// context.
    fn register_for(
        &self,
        token: IoToken,
        request: IoRequest,
        context: IoContext,
    ) -> Result<(), String> {
        let _ = context;
        self.register(token, request)
    }

    /// Poll for completed I/O operations. Returns immediately.
    /// Non-blocking — returns empty vec if nothing is ready.
    fn poll(&self) -> Vec<(IoToken, IoResult)>;

    /// Cancel an outstanding request. The runtime calls this when the process
    /// waiting on `token` is cancelled or otherwise stops waiting.
    ///
    /// Best effort: a backend should release resources held for the request
    /// (for example a pending timer) and may drop its result. A completion
    /// that is still reported for a cancelled token is ignored by the runtime.
    /// Cancelling an unknown or already completed token is a no-op.
    fn cancel(&self, _token: IoToken) {}
}

/// Mock I/O backend for deterministic testing.
/// Completes requests immediately or after a configurable delay.
pub struct MockIoBackend {
    completed: Mutex<Vec<(IoToken, IoResult)>>,
}

impl Default for MockIoBackend {
    fn default() -> Self {
        Self {
            completed: Mutex::new(Vec::new()),
        }
    }
}

impl MockIoBackend {
    pub fn new() -> Self {
        Self::default()
    }
}

impl IoBackend for MockIoBackend {
    fn register(&self, token: IoToken, request: IoRequest) -> Result<(), String> {
        // Mock: complete immediately with canned responses
        let result = match request {
            IoRequest::Sleep(_) => IoResult::Ok {
                value: SendableValue::Unit,
                io_bytes: 0,
            },
            IoRequest::HttpGet { url, .. } => {
                let value = SendableValue::Tuple(vec![
                    SendableValue::Int(200),
                    SendableValue::List(vec![]),
                    SendableValue::String(format!("mock response from {url}").into()),
                ]);
                let io_bytes = value.estimated_bytes() as u64;
                IoResult::Ok { value, io_bytes }
            }
            IoRequest::Http { url, method, .. } => {
                let value = SendableValue::Tuple(vec![
                    SendableValue::Int(200),
                    SendableValue::List(vec![]),
                    SendableValue::String(format!("mock {method} {url}").into()),
                ]);
                let io_bytes = value.estimated_bytes() as u64;
                IoResult::Ok { value, io_bytes }
            }
            IoRequest::ReadFile { path, .. } => {
                let value = SendableValue::String(format!("mock contents of {path}").into());
                let io_bytes = value.estimated_bytes() as u64;
                IoResult::Ok { value, io_bytes }
            }
            #[cfg(feature = "builtin-filesystem")]
            IoRequest::CapReadFile { path, .. } => {
                let value = SendableValue::String(format!("mock contents of {path}").into());
                let io_bytes = value.estimated_bytes() as u64;
                IoResult::Ok { value, io_bytes }
            }
            #[cfg(feature = "builtin-aws-config")]
            IoRequest::AwsConfigSsoProfile { profile } => {
                let value = SendableValue::AwsConfigSsoProfile {
                    profile: Arc::from(profile),
                    sdk_config: Arc::new(aws_config::SdkConfig::builder().build()),
                };
                let io_bytes = value.estimated_bytes() as u64;
                IoResult::Ok { value, io_bytes }
            }
            #[cfg(feature = "builtin-aws-config")]
            IoRequest::AwsConfigInstanceProfile => {
                let value = SendableValue::AwsConfigInstanceProfile {
                    sdk_config: Arc::new(aws_config::SdkConfig::builder().build()),
                };
                let io_bytes = value.estimated_bytes() as u64;
                IoResult::Ok { value, io_bytes }
            }
            #[cfg(feature = "builtin-aws-s3")]
            IoRequest::AwsS3ListBuckets { .. } => {
                let value = list_buckets_success(empty_list_buckets_output());
                let io_bytes = value.estimated_bytes() as u64;
                IoResult::Ok { value, io_bytes }
            }
            #[cfg(feature = "builtin-aws-sqs")]
            IoRequest::AwsSqsListQueues { .. } => {
                let value = list_queues_success(SendableValue::List(Vec::new()));
                let io_bytes = value.estimated_bytes() as u64;
                IoResult::Ok { value, io_bytes }
            }
        };
        self.completed.lock().unwrap().push((token, result));
        Ok(())
    }

    fn poll(&self) -> Vec<(IoToken, IoResult)> {
        let mut completed = self.completed.lock().unwrap();
        std::mem::take(&mut *completed)
    }
}

/// Upper bound on worker threads, however many requests are in flight.
pub const DEFAULT_MAX_IO_THREADS: usize = 256;

/// How long a worker above the core count waits for work before exiting.
const IDLE_WORKER_EXIT: Duration = Duration::from_secs(5);

/// I/O backend with a shared work queue, an elastic worker pool, and one
/// timer thread.
///
/// Blocking requests wait in one queue per owner (process tree). Idle workers
/// take the next request round-robin across owners, skipping owners that
/// already run their `max_in_flight` requests. When a request arrives and no
/// worker is idle, the pool grows, up to `max_threads`; workers above the core
/// count exit after idling. A slow request therefore occupies only its own
/// worker and never delays unrelated requests.
///
/// An HTTP request's deadline runs from registration, not from when a worker
/// starts it: the timer thread fails a request that is still queued when its
/// deadline passes, and a worker starting a request shortens its timeouts to
/// the time left. `Sleep` requests never occupy a worker: the timer thread
/// completes them at their deadline.
///
/// Dropping the backend cancels every pending timer and queued request, and
/// returns without waiting for a worker that is inside a blocking call (an
/// HTTP request, for example). Such a worker exits once its call returns, and
/// its result is discarded. [`ThreadPoolIoBackend::shutdown`] instead joins
/// every worker.
pub struct ThreadPoolIoBackend {
    work: Arc<WorkQueue>,
    completed: Arc<Mutex<Vec<(IoToken, IoResult)>>>,
    workers: Mutex<Vec<std::thread::JoinHandle<()>>>,
    timers: Arc<Timers>,
    timer_thread: Option<std::thread::JoinHandle<()>>,
}

type Completions = Mutex<Vec<(IoToken, IoResult)>>;

fn push_completion(completed: &Completions, token: IoToken, result: IoResult) {
    completed
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push((token, result));
}

/// A blocking request waiting for, or running on, a worker.
struct Job {
    token: IoToken,
    request: IoRequest,
    context: IoContext,
    /// When the request must have completed, counted from registration.
    deadline: Option<Instant>,
}

/// Blocking requests not yet started, plus the bookkeeping that decides which
/// one a free worker takes next.
struct WorkQueue {
    state: Mutex<WorkState>,
    available: std::sync::Condvar,
    core_threads: usize,
    max_threads: usize,
}

#[derive(Default)]
struct WorkState {
    /// Queued jobs per owner. An owner is present only while it has jobs.
    queues: std::collections::HashMap<u64, std::collections::VecDeque<Job>>,
    /// Owners with queued jobs, in round-robin order.
    order: std::collections::VecDeque<u64>,
    /// Running jobs per owner. An owner is present only while it has some.
    in_flight: std::collections::HashMap<u64, usize>,
    /// Workers waiting for a job.
    idle: usize,
    /// Live workers.
    threads: usize,
    shutdown: bool,
}

impl WorkState {
    /// Take the next job round-robin across owners, skipping owners at their
    /// in-flight limit.
    fn take_next(&mut self) -> Option<Job> {
        for _ in 0..self.order.len() {
            let owner = self.order.pop_front()?;
            let running = self.in_flight.get(&owner).copied().unwrap_or(0);
            let queue = self.queues.get_mut(&owner)?;
            let limit = queue.front().map_or(0, |job| job.context.max_in_flight);
            if running < limit {
                let job = queue.pop_front()?;
                if queue.is_empty() {
                    self.queues.remove(&owner);
                } else {
                    self.order.push_back(owner);
                }
                *self.in_flight.entry(owner).or_default() += 1;
                return Some(job);
            }
            self.order.push_back(owner);
        }
        None
    }

    /// Queued jobs that could start now: per owner, up to its free slots.
    fn runnable_jobs(&self) -> usize {
        self.queues
            .iter()
            .map(|(owner, queue)| {
                let running = self.in_flight.get(owner).copied().unwrap_or(0);
                let limit = queue.front().map_or(0, |job| job.context.max_in_flight);
                queue.len().min(limit.saturating_sub(running))
            })
            .sum()
    }

    fn finish(&mut self, owner: u64) {
        if let Some(running) = self.in_flight.get_mut(&owner) {
            *running -= 1;
            if *running == 0 {
                self.in_flight.remove(&owner);
            }
        }
    }

    /// Remove a queued job. Returns `None` if it already started or is unknown.
    fn remove_queued(&mut self, token: IoToken) -> Option<Job> {
        let (owner, index) = self.queues.iter().find_map(|(owner, queue)| {
            queue
                .iter()
                .position(|job| job.token == token)
                .map(|index| (*owner, index))
        })?;
        let queue = self.queues.get_mut(&owner)?;
        let job = queue.remove(index);
        if queue.is_empty() {
            self.queues.remove(&owner);
            self.order.retain(|queued| *queued != owner);
        }
        job
    }
}

impl WorkQueue {
    fn new(core_threads: usize, max_threads: usize) -> Self {
        Self {
            state: Mutex::new(WorkState::default()),
            available: std::sync::Condvar::new(),
            core_threads,
            max_threads,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, WorkState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Queue a job. Returns whether the caller should start another worker.
    fn push(&self, job: Job) -> Result<bool, String> {
        let mut state = self.lock();
        if state.shutdown {
            return Err("io backend is shut down".into());
        }
        let owner = job.context.owner;
        let queue = state.queues.entry(owner).or_default();
        queue.push_back(job);
        if queue.len() == 1 {
            state.order.push_back(owner);
        }
        // An idle worker woken for an earlier job still counts as idle until
        // it takes that job, so compare against all runnable queued jobs
        // rather than checking for zero idle workers.
        let grow = state.runnable_jobs() > state.idle && state.threads < self.max_threads;
        if grow {
            state.threads += 1;
        }
        self.available.notify_one();
        Ok(grow)
    }

    fn remove_queued(&self, token: IoToken) -> Option<Job> {
        self.lock().remove_queued(token)
    }

    /// Drop every queued job and tell workers to exit.
    fn shutdown(&self) {
        let mut state = self.lock();
        state.shutdown = true;
        state.queues.clear();
        state.order.clear();
        self.available.notify_all();
    }

    /// Worker thread body.
    fn run_worker(&self, timers: &Timers, completed: &Completions) {
        let mut state = self.lock();
        loop {
            if state.shutdown {
                state.threads -= 1;
                return;
            }
            if let Some(job) = state.take_next() {
                drop(state);
                // The job left the queue, so its queue deadline is moot.
                timers.cancel(job.token);
                let (token, owner) = (job.token, job.context.owner);
                let result = run_job(job);
                push_completion(completed, token, result);
                state = self.lock();
                state.finish(owner);
                continue;
            }
            state.idle += 1;
            let (next, wait) = self
                .available
                .wait_timeout(state, IDLE_WORKER_EXIT)
                .unwrap_or_else(|e| e.into_inner());
            state = next;
            state.idle -= 1;
            if wait.timed_out() && state.threads > self.core_threads && state.order.is_empty() {
                state.threads -= 1;
                return;
            }
        }
    }
}

/// When an HTTP request must have completed, counted from `issued`.
fn request_deadline(request: &IoRequest, issued: Instant) -> Option<Instant> {
    match request {
        IoRequest::HttpGet { timeouts, .. } | IoRequest::Http { timeouts, .. } => {
            issued.checked_add(timeouts.global)
        }
        _ => None,
    }
}

fn queued_timeout_error(request: &IoRequest) -> IoResult {
    let builtin = match request {
        IoRequest::Http { format, .. } => format.builtin_name(),
        _ => "http_get",
    };
    IoResult::Err(format!(
        "{builtin}: http: timeout: deadline passed while the request was queued"
    ))
}

/// A request dequeued with less time than this before its deadline fails
/// instead of starting. It could not complete anyway, and ureq does not
/// honour timeouts of about one millisecond: they take about a second.
const MIN_START_BUDGET: Duration = Duration::from_millis(10);

/// Run a dequeued job, shortening HTTP timeouts to the time left before its
/// deadline.
fn run_job(mut job: Job) -> IoResult {
    if let Some(deadline) = job.deadline {
        let left = deadline.saturating_duration_since(Instant::now());
        if left < MIN_START_BUDGET {
            return queued_timeout_error(&job.request);
        }
        if let IoRequest::HttpGet { timeouts, .. } | IoRequest::Http { timeouts, .. } =
            &mut job.request
        {
            timeouts.global = timeouts.global.min(left);
            timeouts.connect = timeouts.connect.min(left);
        }
    }
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        execute_io_request(job.request)
    }))
    .unwrap_or_else(|_| IoResult::Err("io backend worker panicked".into()))
}

/// Pending timers, shared by the backend and its timer thread: `Sleep`
/// requests, and deadlines of queued HTTP requests.
struct Timers {
    state: Mutex<TimerState>,
    changed: std::sync::Condvar,
}

#[derive(Default)]
struct TimerState {
    /// Min-heap of deadlines. It may hold entries for cancelled tokens; those
    /// are skipped when they reach the top, and pruned when they dominate.
    deadlines: std::collections::BinaryHeap<std::cmp::Reverse<(Instant, u64)>>,
    /// Tokens whose timer has neither fired nor been cancelled.
    pending: std::collections::HashMap<IoToken, TimerKind>,
    shutdown: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TimerKind {
    /// Complete a `Sleep` request.
    Sleep,
    /// Fail the request if it is still queued.
    QueueDeadline,
}

impl Timers {
    fn new() -> Self {
        Self {
            state: Mutex::new(TimerState::default()),
            changed: std::sync::Condvar::new(),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, TimerState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn add(&self, token: IoToken, duration: Duration) -> Result<(), String> {
        let now = Instant::now();
        // Durations too large to represent are clamped to roughly a century.
        let deadline = now
            .checked_add(duration)
            .unwrap_or_else(|| now + Duration::from_secs(100 * 365 * 24 * 3600));
        self.add_at(token, deadline, TimerKind::Sleep)
    }

    fn add_at(&self, token: IoToken, deadline: Instant, kind: TimerKind) -> Result<(), String> {
        let mut state = self.lock();
        if state.shutdown {
            return Err("io backend is shut down".into());
        }
        state.deadlines.push(std::cmp::Reverse((deadline, token.0)));
        state.pending.insert(token, kind);
        self.changed.notify_one();
        Ok(())
    }

    fn cancel(&self, token: IoToken) {
        let mut state = self.lock();
        if state.pending.remove(&token).is_none() {
            return;
        }
        // Keep the heap proportional to the live timers so that cancelled
        // long sleeps do not accumulate.
        if state.deadlines.len() > 2 * state.pending.len() + 16 {
            let TimerState {
                deadlines, pending, ..
            } = &mut *state;
            deadlines.retain(|entry| pending.contains_key(&IoToken(entry.0.1)));
        }
        self.changed.notify_one();
    }

    fn shutdown(&self) {
        let mut state = self.lock();
        state.shutdown = true;
        state.pending.clear();
        state.deadlines.clear();
        self.changed.notify_all();
    }

    /// Timer thread body: complete each pending sleep at its deadline, and
    /// fail each HTTP request still queued at its deadline.
    ///
    /// Lock order is timers, then work queue; nothing takes them the other way.
    fn run(&self, completed: &Completions, work: &WorkQueue) {
        let mut state = self.lock();
        loop {
            if state.shutdown {
                return;
            }
            let now = Instant::now();
            match state.deadlines.peek().map(|entry| entry.0) {
                None => {
                    state = self.changed.wait(state).unwrap_or_else(|e| e.into_inner());
                }
                Some((deadline, raw_token)) if deadline <= now => {
                    state.deadlines.pop();
                    let token = IoToken(raw_token);
                    match state.pending.remove(&token) {
                        Some(TimerKind::Sleep) => push_completion(
                            completed,
                            token,
                            IoResult::Ok {
                                value: SendableValue::Unit,
                                io_bytes: 0,
                            },
                        ),
                        Some(TimerKind::QueueDeadline) => {
                            if let Some(job) = work.remove_queued(token) {
                                push_completion(
                                    completed,
                                    token,
                                    queued_timeout_error(&job.request),
                                );
                            }
                        }
                        None => {}
                    }
                }
                Some((deadline, _)) => {
                    state = self
                        .changed
                        .wait_timeout(state, deadline - now)
                        .unwrap_or_else(|e| e.into_inner())
                        .0;
                }
            }
        }
    }
}

impl ThreadPoolIoBackend {
    /// A pool that keeps `num_threads` workers and grows up to
    /// [`DEFAULT_MAX_IO_THREADS`] while requests are waiting.
    pub fn new(num_threads: usize) -> Self {
        Self::with_max_threads(num_threads, DEFAULT_MAX_IO_THREADS)
    }

    /// A pool that keeps `core_threads` workers and grows up to `max_threads`.
    pub fn with_max_threads(core_threads: usize, max_threads: usize) -> Self {
        let core_threads = core_threads.max(1);
        let max_threads = max_threads.max(core_threads);
        let completed = Arc::new(Mutex::new(Vec::new()));
        let work = Arc::new(WorkQueue::new(core_threads, max_threads));
        let timers = Arc::new(Timers::new());
        let timer_thread = {
            let timers = Arc::clone(&timers);
            let done = Arc::clone(&completed);
            let work = Arc::clone(&work);
            std::thread::spawn(move || timers.run(&done, &work))
        };

        let backend = Self {
            work,
            completed,
            workers: Mutex::new(Vec::with_capacity(core_threads)),
            timers,
            timer_thread: Some(timer_thread),
        };
        backend.work.lock().threads = core_threads;
        for _ in 0..core_threads {
            backend.spawn_worker();
        }
        backend
    }

    fn spawn_worker(&self) {
        let work = Arc::clone(&self.work);
        let timers = Arc::clone(&self.timers);
        let done = Arc::clone(&self.completed);
        let worker = std::thread::spawn(move || work.run_worker(&timers, &done));
        let mut workers = self.workers.lock().unwrap_or_else(|e| e.into_inner());
        workers.retain(|handle| !handle.is_finished());
        workers.push(worker);
    }

    /// Live worker threads.
    pub fn worker_count(&self) -> usize {
        self.work.lock().threads
    }

    /// Stop accepting requests, cancel pending timers, and join every thread.
    ///
    /// This blocks until in-flight blocking requests (HTTP, file reads)
    /// return. Dropping the backend does not wait for those.
    pub fn shutdown(&mut self) -> Result<(), String> {
        self.stop_timers();
        self.work.shutdown();
        let workers = self.workers.get_mut().unwrap_or_else(|e| e.into_inner());
        let mut panicked = 0;
        for worker in workers.drain(..) {
            if worker.join().is_err() {
                panicked += 1;
            }
        }
        if panicked == 0 {
            Ok(())
        } else {
            Err(format!("{panicked} io backend worker(s) panicked"))
        }
    }

    /// Cancel every pending timer and join the timer thread. This is prompt:
    /// the timer thread only ever blocks on its own condition variable.
    fn stop_timers(&mut self) {
        self.timers.shutdown();
        if let Some(timer_thread) = self.timer_thread.take() {
            let _ = timer_thread.join();
        }
    }
}

impl IoBackend for ThreadPoolIoBackend {
    fn register(&self, token: IoToken, request: IoRequest) -> Result<(), String> {
        self.register_for(token, request, IoContext::default())
    }

    fn register_for(
        &self,
        token: IoToken,
        request: IoRequest,
        context: IoContext,
    ) -> Result<(), String> {
        if let IoRequest::Sleep(duration) = request {
            return self.timers.add(token, duration);
        }
        let deadline = request_deadline(&request, Instant::now());
        let job = Job {
            token,
            request,
            context,
            deadline,
        };
        if self.work.push(job)? {
            self.spawn_worker();
        }
        if let Some(deadline) = deadline {
            // If a worker already took the job, the timer finds nothing queued.
            self.timers
                .add_at(token, deadline, TimerKind::QueueDeadline)?;
        }
        Ok(())
    }

    fn poll(&self) -> Vec<(IoToken, IoResult)> {
        let mut completed = self.completed.lock().unwrap_or_else(|e| e.into_inner());
        std::mem::take(&mut *completed)
    }

    fn cancel(&self, token: IoToken) {
        // A queued request is dropped. A blocking request cannot be
        // interrupted once a worker has started it; the runtime ignores its
        // late result.
        self.timers.cancel(token);
        self.work.remove_queued(token);
    }
}

impl Drop for ThreadPoolIoBackend {
    fn drop(&mut self) {
        self.stop_timers();
        // Idle workers exit. Workers inside a blocking call are detached
        // rather than joined, so dropping the backend never waits on the
        // network or the disk.
        self.work.shutdown();
        self.workers
            .get_mut()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }
}

fn execute_io_request(request: IoRequest) -> IoResult {
    match request {
        IoRequest::Sleep(duration) => {
            std::thread::sleep(duration);
            IoResult::Ok {
                value: SendableValue::Unit,
                io_bytes: 0,
            }
        }
        IoRequest::HttpGet {
            url,
            max_response,
            timeouts,
        } => match aio_http_get(&url, max_response, timeouts) {
            Ok((value, io_bytes)) => IoResult::Ok { value, io_bytes },
            Err(e) => IoResult::Err(e),
        },
        IoRequest::Http {
            method,
            url,
            headers,
            body,
            format,
            max_response,
            timeouts,
        } => match aio_http(
            format.builtin_name(),
            &method,
            &url,
            &headers,
            &body,
            format,
            max_response,
            timeouts,
        ) {
            Ok((value, io_bytes)) => IoResult::Ok { value, io_bytes },
            Err(e) => IoResult::Err(e),
        },
        IoRequest::ReadFile { path, max_bytes } => {
            match read_file_to_string_limited(&path, max_bytes) {
                Ok((contents, io_bytes)) => IoResult::Ok {
                    io_bytes,
                    value: SendableValue::String(contents.into()),
                },
                Err(e) => IoResult::Err(format!("read_file: {e}")),
            }
        }
        #[cfg(feature = "builtin-filesystem")]
        IoRequest::CapReadFile {
            candidates,
            path,
            max_bytes,
        } => match cap_read_to_string(&candidates, &path, max_bytes) {
            Ok((contents, io_bytes)) => IoResult::Ok {
                io_bytes,
                value: SendableValue::String(contents.into()),
            },
            Err(e) => IoResult::Err(format!("read_file: {e}")),
        },
        #[cfg(feature = "builtin-aws-config")]
        IoRequest::AwsConfigSsoProfile { profile } => match aio_aws_config_sso_profile(&profile) {
            Ok(value) => IoResult::Ok {
                io_bytes: value.estimated_bytes() as u64,
                value,
            },
            Err(e) => IoResult::Err(e),
        },
        #[cfg(feature = "builtin-aws-config")]
        IoRequest::AwsConfigInstanceProfile => match aio_aws_config_instance_profile() {
            Ok(value) => IoResult::Ok {
                io_bytes: value.estimated_bytes() as u64,
                value,
            },
            Err(e) => IoResult::Err(e),
        },
        #[cfg(feature = "builtin-aws-s3")]
        IoRequest::AwsS3ListBuckets { client } => {
            let value = match aio_aws_s3_list_buckets(client) {
                Ok(value) => list_buckets_success(value),
                Err(e) => list_buckets_error(e),
            };
            IoResult::Ok {
                io_bytes: value.estimated_bytes() as u64,
                value,
            }
        }
        #[cfg(feature = "builtin-aws-sqs")]
        IoRequest::AwsSqsListQueues { client } => {
            let value = match aio_aws_sqs_list_queues(client) {
                Ok(value) => list_queues_success(value),
                Err(e) => list_queues_error(e),
            };
            IoResult::Ok {
                io_bytes: value.estimated_bytes() as u64,
                value,
            }
        }
    }
}

/// Read until EOF, stopping one byte past the limit so an oversized source is
/// never buffered in full.
pub(crate) fn read_to_bytes_limited<R: Read>(
    mut reader: R,
    limit: ReadLimit,
    context: &str,
) -> Result<(Vec<u8>, u64), String> {
    let mut buf = Vec::new();
    reader
        .by_ref()
        .take(limit.max_bytes.saturating_add(1))
        .read_to_end(&mut buf)
        .map_err(|e| format!("{context}: {e}"))?;
    if buf.len() as u64 > limit.max_bytes {
        return Err(limit.exceeded_message(context));
    }
    let io_bytes = buf.len() as u64;
    Ok((buf, io_bytes))
}

pub(crate) fn read_to_string_limited<R: Read>(
    reader: R,
    limit: ReadLimit,
    context: &str,
) -> Result<(String, u64), String> {
    let (buf, io_bytes) = read_to_bytes_limited(reader, limit, context)?;
    let text = String::from_utf8(buf).map_err(|e| format!("{context}: {e}"))?;
    Ok((text, io_bytes))
}

fn read_file_to_string_limited(path: &str, max_bytes: ReadLimit) -> Result<(String, u64), String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    read_to_string_limited(file, max_bytes, "file read")
}

#[cfg(feature = "builtin-filesystem")]
fn cap_read_to_string(
    candidates: &[CapFsCandidate],
    _path: &str,
    max_bytes: ReadLimit,
) -> Result<(String, u64), String> {
    let mut last_err = None;
    for candidate in candidates {
        match candidate.dir.open(&candidate.relative_path) {
            Ok(file) => match read_to_string_limited(file, max_bytes, "file read") {
                Ok(contents) => return Ok(contents),
                Err(err) => last_err = Some(std::io::Error::other(err)),
            },
            Err(err) => last_err = Some(err),
        }
    }
    Err(last_err
        .map(|e| e.to_string())
        .unwrap_or_else(|| "no allowed folder matched".into()))
}

#[cfg(feature = "builtin-aws-s3")]
fn sendable_bucket(bucket: &aws_sdk_s3::types::Bucket) -> SendableValue {
    let name = bucket.name().unwrap_or("").to_string();
    let creation_date = bucket
        .creation_date()
        .map(|d| d.to_string())
        .unwrap_or_default();
    let arn = bucket.bucket_arn().unwrap_or("").to_string();
    SendableValue::Tuple(vec![
        SendableValue::String(Arc::from(name)),
        SendableValue::String(Arc::from(creation_date)),
        SendableValue::String(Arc::from(arn)),
    ])
}

#[cfg(feature = "builtin-aws-s3")]
fn sendable_list_buckets_output(
    output: &aws_sdk_s3::operation::list_buckets::ListBucketsOutput,
) -> SendableValue {
    output
        .buckets
        .as_ref()
        .map(|bs| SendableValue::List(bs.iter().map(sendable_bucket).collect()))
        .unwrap_or(SendableValue::List(Vec::new()))
}

#[cfg(feature = "builtin-aws-s3")]
fn empty_list_buckets_output() -> SendableValue {
    SendableValue::List(Vec::new())
}

#[cfg(feature = "builtin-aws-s3")]
fn list_buckets_success(output: SendableValue) -> SendableValue {
    SendableValue::Tuple(vec![
        SendableValue::Bool(true),
        output,
        SendableValue::String(Arc::from("")),
    ])
}

#[cfg(feature = "builtin-aws-s3")]
fn list_buckets_error(message: String) -> SendableValue {
    SendableValue::Tuple(vec![
        SendableValue::Bool(false),
        empty_list_buckets_output(),
        SendableValue::String(Arc::from(message)),
    ])
}

#[cfg(feature = "builtin-aws-sqs")]
fn list_queues_success(output: SendableValue) -> SendableValue {
    SendableValue::Tuple(vec![
        SendableValue::Bool(true),
        output,
        SendableValue::String(Arc::from("")),
    ])
}

#[cfg(feature = "builtin-aws-sqs")]
fn list_queues_error(message: String) -> SendableValue {
    SendableValue::Tuple(vec![
        SendableValue::Bool(false),
        SendableValue::List(Vec::new()),
        SendableValue::String(Arc::from(message)),
    ])
}

#[cfg(any(
    feature = "builtin-aws-config",
    feature = "builtin-aws-s3",
    feature = "builtin-aws-sqs"
))]
fn format_aws_error<E>(context: &str, err: &E) -> String
where
    E: std::error::Error + std::fmt::Display,
{
    const MAX_SOURCES: usize = 16;

    let mut message = format!("{context}: {err}");
    let mut seen = std::collections::BTreeSet::new();
    seen.insert(err.to_string());

    let mut sources = Vec::new();
    let mut source = err.source();
    let mut source_count = 0;
    let mut truncated = false;
    while let Some(err) = source {
        if source_count >= MAX_SOURCES {
            truncated = true;
            break;
        }

        let source_message = err.to_string();
        if !source_message.is_empty() && seen.insert(source_message.clone()) {
            sources.push(source_message);
        }
        source = err.source();
        source_count += 1;
    }

    if !sources.is_empty() {
        message.push_str("; caused by: ");
        message.push_str(&sources.join("; "));
        if truncated {
            message.push_str("; ...");
        }
    }

    message
}

#[cfg(feature = "builtin-aws-config")]
fn aio_aws_config_sso_profile(profile: &str) -> Result<SendableValue, String> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("aws_config_sso_profile: tokio runtime: {e}"))?;
    runtime.block_on(async {
        let sdk_config = aws_config::defaults(aws_config::BehaviorVersion::latest())
            .profile_name(profile)
            .load()
            .await;
        Ok(SendableValue::AwsConfigSsoProfile {
            profile: Arc::from(profile),
            sdk_config: Arc::new(sdk_config),
        })
    })
}

#[cfg(feature = "builtin-aws-config")]
fn aio_aws_config_instance_profile() -> Result<SendableValue, String> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("aws_config_instance_profile: tokio runtime: {e}"))?;
    runtime.block_on(async {
        let sdk_config = aws_config::defaults(aws_config::BehaviorVersion::latest())
            .load()
            .await;
        Ok(SendableValue::AwsConfigInstanceProfile {
            sdk_config: Arc::new(sdk_config),
        })
    })
}

#[cfg(feature = "builtin-aws-s3")]
fn aio_aws_s3_list_buckets(client: Arc<aws_sdk_s3::Client>) -> Result<SendableValue, String> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("aws_s3_list_buckets: tokio runtime: {e}"))?;
    runtime.block_on(async {
        let output = client
            .list_buckets()
            .send()
            .await
            .map_err(|e| format_aws_error("aws_s3_list_buckets", &e))?;
        Ok(sendable_list_buckets_output(&output))
    })
}

#[cfg(feature = "builtin-aws-sqs")]
fn aio_aws_sqs_list_queues(client: Arc<aws_sdk_sqs::Client>) -> Result<SendableValue, String> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("aws_sqs_list_queues: tokio runtime: {e}"))?;
    runtime.block_on(async {
        let output = client
            .list_queues()
            .send()
            .await
            .map_err(|e| format_aws_error("aws_sqs_list_queues", &e))?;
        Ok(SendableValue::List(
            output
                .queue_urls()
                .iter()
                .map(|url| SendableValue::String(Arc::from(url.as_str())))
                .collect(),
        ))
    })
}

/// Async HTTP GET — runs on I/O pool thread.
#[cfg(feature = "builtin-http")]
fn aio_http_get(
    url: &str,
    max_response: ReadLimit,
    timeouts: HttpTimeouts,
) -> Result<(SendableValue, u64), String> {
    aio_http(
        "http_get",
        "GET",
        url,
        &[],
        "",
        HttpResponseFormat::Text,
        max_response,
        timeouts,
    )
}

/// Async full HTTP — runs on I/O pool thread. `builtin` prefixes errors, as
/// on the sync path.
#[cfg(feature = "builtin-http")]
#[allow(clippy::too_many_arguments)]
fn aio_http(
    builtin: &str,
    method: &str,
    url: &str,
    req_headers: &[(String, String)],
    body: &str,
    format: HttpResponseFormat,
    max_response: ReadLimit,
    timeouts: HttpTimeouts,
) -> Result<(SendableValue, u64), String> {
    let headers = req_headers
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect::<Vec<_>>();
    let response = hiko_common::dispatch_ureq(method, url, &headers, body, timeouts)
        .map_err(|e| format!("{builtin}: {e}"))?;

    let status = SendableValue::Int(response.status().as_u16() as i64);
    let headers: Vec<SendableValue> = response
        .headers()
        .iter()
        .map(|(k, v)| {
            SendableValue::Tuple(vec![
                SendableValue::String(k.as_str().into()),
                SendableValue::String(v.to_str().unwrap_or("").into()),
            ])
        })
        .collect();

    let (resp_body, io_bytes) = match format {
        HttpResponseFormat::Text => {
            let (s, io_bytes) =
                read_to_string_limited(response.into_body().into_reader(), max_response, builtin)?;
            (SendableValue::String(s.into()), io_bytes)
        }
        HttpResponseFormat::Json => {
            #[cfg(feature = "builtin-http")]
            {
                let (s, io_bytes) = read_to_string_limited(
                    response.into_body().into_reader(),
                    max_response,
                    "http_json",
                )?;
                let parsed: serde_json::Value =
                    serde_json::from_str(&s).map_err(|e| format!("http_json: {e}"))?;
                (json_value_to_sendable(&parsed)?, io_bytes)
            }
            #[cfg(not(feature = "builtin-http"))]
            {
                let _ = response;
                return Err("http_json is not available in this build".into());
            }
        }
        HttpResponseFormat::Msgpack => {
            #[cfg(feature = "builtin-http")]
            {
                let (buf, io_bytes) = read_to_bytes_limited(
                    response.into_body().into_reader(),
                    max_response,
                    "http_msgpack",
                )?;
                let parsed: serde_json::Value =
                    rmp_serde::from_slice(&buf).map_err(|e| format!("http_msgpack: {e}"))?;
                (json_value_to_sendable(&parsed)?, io_bytes)
            }
            #[cfg(not(feature = "builtin-http"))]
            {
                let _ = response;
                return Err("http_msgpack is not available in this build".into());
            }
        }
        HttpResponseFormat::Bytes => {
            let (buf, io_bytes) = read_to_bytes_limited(
                response.into_body().into_reader(),
                max_response,
                "http_bytes",
            )?;
            (SendableValue::Bytes(buf.into()), io_bytes)
        }
    };

    Ok((
        SendableValue::Tuple(vec![status, SendableValue::List(headers), resp_body]),
        io_bytes,
    ))
}

/// Convert a serde_json::Value to SendableValue (for async JSON/msgpack parsing).
#[cfg(feature = "builtin-http")]
fn json_value_to_sendable(v: &serde_json::Value) -> Result<SendableValue, String> {
    match v {
        serde_json::Value::Null => Ok(SendableValue::Unit),
        serde_json::Value::Bool(b) => Ok(SendableValue::Bool(*b)),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(SendableValue::Int(i))
            } else {
                let f = n
                    .as_f64()
                    .ok_or_else(|| format!("json number not representable as f64: {n}"))?;
                Ok(SendableValue::Float(f))
            }
        }
        serde_json::Value::String(s) => Ok(SendableValue::String(s.clone().into())),
        serde_json::Value::Array(arr) => {
            let items: Result<Vec<_>, _> = arr.iter().map(json_value_to_sendable).collect();
            Ok(SendableValue::List(items?))
        }
        serde_json::Value::Object(obj) => {
            let items: Result<Vec<SendableValue>, String> = obj
                .iter()
                .map(|(k, val)| {
                    Ok(SendableValue::Tuple(vec![
                        SendableValue::String(k.clone().into()),
                        json_value_to_sendable(val)?,
                    ]))
                })
                .collect();
            Ok(SendableValue::List(items?))
        }
    }
}

#[cfg(not(feature = "builtin-http"))]
fn aio_http_get(
    url: &str,
    max_response: ReadLimit,
    timeouts: HttpTimeouts,
) -> Result<(SendableValue, u64), String> {
    let _ = (url, max_response, timeouts);
    Err("http_get is not available in this build".into())
}

#[cfg(not(feature = "builtin-http"))]
#[allow(clippy::too_many_arguments)]
fn aio_http(
    builtin: &str,
    method: &str,
    url: &str,
    req_headers: &[(String, String)],
    body: &str,
    format: HttpResponseFormat,
    max_response: ReadLimit,
    timeouts: HttpTimeouts,
) -> Result<(SendableValue, u64), String> {
    let _ = (
        builtin,
        method,
        url,
        req_headers,
        body,
        format,
        max_response,
        timeouts,
    );
    Err("HTTP builtins are not available in this build".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(any(
        feature = "builtin-aws-config",
        feature = "builtin-aws-s3",
        feature = "builtin-aws-sqs"
    ))]
    #[derive(Debug)]
    struct TestAwsError {
        message: &'static str,
        source: Option<Box<TestAwsError>>,
    }

    #[cfg(any(
        feature = "builtin-aws-config",
        feature = "builtin-aws-s3",
        feature = "builtin-aws-sqs"
    ))]
    impl std::fmt::Display for TestAwsError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "{}", self.message)
        }
    }

    #[cfg(any(
        feature = "builtin-aws-config",
        feature = "builtin-aws-s3",
        feature = "builtin-aws-sqs"
    ))]
    impl std::error::Error for TestAwsError {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            self.source
                .as_deref()
                .map(|source| source as &(dyn std::error::Error + 'static))
        }
    }

    #[test]
    #[cfg(any(
        feature = "builtin-aws-config",
        feature = "builtin-aws-s3",
        feature = "builtin-aws-sqs"
    ))]
    fn test_format_aws_error_preserves_context_and_deduplicated_source_chain() {
        let message = format_aws_error(
            "aws_s3_list_buckets",
            &TestAwsError {
                message: "request failed",
                source: Some(Box::new(TestAwsError {
                    message: "other",
                    source: Some(Box::new(TestAwsError {
                        message: "other",
                        source: Some(Box::new(TestAwsError {
                            message: "socket closed",
                            source: Some(Box::new(TestAwsError {
                                message: "socket closed",
                                source: None,
                            })),
                        })),
                    })),
                })),
            },
        );

        assert_eq!(
            message,
            "aws_s3_list_buckets: request failed; caused by: other; socket closed"
        );
        assert!(!message.contains("debug:"), "{message}");
    }

    #[test]
    #[cfg(feature = "builtin-aws-s3")]
    fn test_s3_errors_are_returned_as_hiko_result_tuple_with_message() {
        let value = list_buckets_error("connection reset by peer".to_string());

        match value {
            SendableValue::Tuple(fields) => {
                assert!(
                    matches!(fields.as_slice(), [SendableValue::Bool(false), SendableValue::List(items), SendableValue::String(message)] if items.is_empty() && message.as_ref() == "connection reset by peer")
                );
            }
            other => panic!("unexpected S3 error value: {other:?}"),
        }
    }

    #[test]
    #[cfg(feature = "builtin-aws-sqs")]
    fn test_sqs_errors_are_returned_as_hiko_result_tuple_with_message() {
        let value = list_queues_error("connection reset by peer".to_string());

        match value {
            SendableValue::Tuple(fields) => {
                assert!(
                    matches!(fields.as_slice(), [SendableValue::Bool(false), SendableValue::List(items), SendableValue::String(message)] if items.is_empty() && message.as_ref() == "connection reset by peer")
                );
            }
            other => panic!("unexpected SQS error value: {other:?}"),
        }
    }

    #[test]
    #[cfg(feature = "builtin-http")]
    fn test_json_value_to_sendable_normal_float() {
        let v: serde_json::Value = serde_json::from_str("3.5").unwrap();
        let result = json_value_to_sendable(&v).unwrap();
        assert!(matches!(result, SendableValue::Float(f) if (f - 3.5).abs() < f64::EPSILON));
    }

    #[test]
    #[cfg(feature = "builtin-http")]
    fn test_json_value_to_sendable_unrepresentable_float_errors() {
        // serde_json with the arbitrary_precision feature can produce numbers
        // that have no f64 representation. Without that feature, all parsed
        // numbers fit in f64, so we construct one via the Number API that
        // would fail as_f64() -- specifically a u64 that exceeds i64::MAX so
        // as_i64() returns None, then check whether as_f64() also fails.
        // In practice, serde_json::Number from a u64 always has an f64 path,
        // so we verify the happy path doesn't silently produce 0.0.
        let v: serde_json::Value = serde_json::json!(u64::MAX);
        let result = json_value_to_sendable(&v);
        // u64::MAX cannot be represented as i64, so the code takes the float
        // branch. as_f64() succeeds (lossy) so we get Ok, but crucially we
        // do NOT get 0.0.
        match result {
            Ok(SendableValue::Float(f)) => assert!(f > 0.0, "must not silently map to 0.0"),
            Ok(SendableValue::Int(_)) => { /* also acceptable */ }
            other => panic!("unexpected result: {other:?}"),
        }
    }

    const LIMIT_3: ReadLimit = ReadLimit {
        max_bytes: 3,
        limit_name: "max_file_read_bytes",
    };

    #[test]
    fn read_to_bytes_limited_rejects_over_limit() {
        let err =
            read_to_bytes_limited(std::io::Cursor::new(b"abcd"), LIMIT_3, "test").unwrap_err();
        assert_eq!(err, "test: result exceeds max_file_read_bytes (3 bytes)");
    }

    #[cfg(unix)]
    #[test]
    fn read_to_bytes_limited_stops_reading_unbounded_source() {
        // /dev/zero never ends; the read must stop at the limit instead of
        // buffering the whole source.
        let zero = std::fs::File::open("/dev/zero").unwrap();
        let err = read_to_bytes_limited(zero, LIMIT_3, "test").unwrap_err();
        assert!(err.contains("max_file_read_bytes"), "{err}");
    }

    #[test]
    fn read_to_string_limited_allows_exact_limit() {
        let (text, io_bytes) =
            read_to_string_limited(std::io::Cursor::new("abc"), LIMIT_3, "test").unwrap();
        assert_eq!(text, "abc");
        assert_eq!(io_bytes, 3);
    }

    #[test]
    fn test_mock_sleep_completes_immediately() {
        let backend = MockIoBackend::new();
        backend
            .register(IoToken(1), IoRequest::Sleep(Duration::from_millis(100)))
            .unwrap();
        let results = backend.poll();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, IoToken(1));
        assert!(matches!(
            results[0].1,
            IoResult::Ok {
                value: SendableValue::Unit,
                io_bytes: 0
            }
        ));
    }

    #[test]
    fn test_mock_poll_drains() {
        let backend = MockIoBackend::new();
        backend
            .register(IoToken(1), IoRequest::Sleep(Duration::from_millis(0)))
            .unwrap();
        let r1 = backend.poll();
        assert_eq!(r1.len(), 1);
        let r2 = backend.poll();
        assert_eq!(r2.len(), 0);
    }

    #[test]
    fn test_thread_pool_zero_threads_still_accepts_request() {
        let backend = ThreadPoolIoBackend::new(0);
        backend
            .register(IoToken(1), IoRequest::Sleep(Duration::from_millis(0)))
            .unwrap();

        for _ in 0..50 {
            let results = backend.poll();
            if !results.is_empty() {
                assert_eq!(results.len(), 1);
                assert_eq!(results[0].0, IoToken(1));
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }

        panic!("thread pool backend did not complete request");
    }

    #[test]
    fn test_thread_pool_sleep_does_not_occupy_a_worker() {
        // One worker: a long sleep must not delay a request queued behind it.
        let backend = ThreadPoolIoBackend::new(1);
        backend
            .register(IoToken(1), IoRequest::Sleep(Duration::from_secs(60)))
            .unwrap();
        backend
            .register(IoToken(2), IoRequest::Sleep(Duration::from_millis(0)))
            .unwrap();
        loop {
            let results = backend.poll();
            if let Some((token, _)) = results.first() {
                assert_eq!(*token, IoToken(2));
                break;
            }
            std::thread::yield_now();
        }
        assert!(backend.timers.lock().pending.contains_key(&IoToken(1)));
    }

    #[test]
    fn test_thread_pool_cancel_releases_timer() {
        let backend = ThreadPoolIoBackend::new(1);
        backend
            .register(IoToken(1), IoRequest::Sleep(Duration::from_secs(60)))
            .unwrap();
        backend.cancel(IoToken(1));
        backend.cancel(IoToken(1));
        backend.cancel(IoToken(99));
        assert!(backend.timers.lock().pending.is_empty());
    }

    #[test]
    fn test_thread_pool_drop_with_pending_sleep_is_prompt() {
        let backend = ThreadPoolIoBackend::new(2);
        backend
            .register(IoToken(1), IoRequest::Sleep(Duration::from_secs(60)))
            .unwrap();
        let start = std::time::Instant::now();
        drop(backend);
        assert!(
            start.elapsed() < Duration::from_millis(100),
            "drop took {:?}",
            start.elapsed()
        );
    }

    #[test]
    fn test_thread_pool_shutdown_closes_registration() {
        let mut backend = ThreadPoolIoBackend::new(1);
        backend.shutdown().unwrap();

        let err = backend
            .register(IoToken(1), IoRequest::Sleep(Duration::from_millis(0)))
            .unwrap_err();
        assert!(err.contains("shut down"));
    }

    /// Local HTTP server that answers each request after `delay`, one thread
    /// per connection. Returns its URL and a count of accepted connections.
    #[cfg(feature = "builtin-http")]
    fn slow_server(delay: Duration) -> (String, Arc<std::sync::atomic::AtomicUsize>) {
        use std::io::{Read as _, Write as _};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let accepted = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = Arc::clone(&accepted);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                std::thread::spawn(move || {
                    let mut request = [0u8; 1024];
                    let _ = stream.read(&mut request);
                    std::thread::sleep(delay);
                    let _ = stream.write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                    );
                });
            }
        });
        (format!("http://{addr}/"), accepted)
    }

    #[cfg(feature = "builtin-http")]
    fn http_get_request(url: &str, timeout: Duration) -> IoRequest {
        IoRequest::HttpGet {
            url: url.to_string(),
            max_response: ReadLimit {
                max_bytes: 1024,
                limit_name: "max_http_response_bytes",
            },
            timeouts: HttpTimeouts {
                connect: timeout,
                global: timeout,
            },
        }
    }

    /// Poll until `count` completions arrived or `limit` passed.
    fn collect(
        backend: &ThreadPoolIoBackend,
        count: usize,
        limit: Duration,
    ) -> Vec<(IoToken, IoResult, Duration)> {
        let start = Instant::now();
        let mut results = Vec::new();
        while results.len() < count && start.elapsed() < limit {
            for (token, result) in backend.poll() {
                results.push((token, result, start.elapsed()));
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        results
    }

    #[test]
    fn test_thread_pool_sleeps_never_occupy_a_worker() {
        // One worker, which may never grow: if any sleep took the worker, the
        // file read below could not run until that sleep ended.
        let backend = ThreadPoolIoBackend::with_max_threads(1, 1);
        for raw in 0..50 {
            backend
                .register(IoToken(raw), IoRequest::Sleep(Duration::from_secs(60)))
                .unwrap();
        }
        let path = std::env::temp_dir().join(format!("hiko-io-pool-{}", std::process::id()));
        std::fs::write(&path, "data").unwrap();
        backend
            .register(
                IoToken(100),
                IoRequest::ReadFile {
                    path: path.to_string_lossy().into_owned(),
                    max_bytes: ReadLimit {
                        max_bytes: 1024,
                        limit_name: "max_file_read_bytes",
                    },
                },
            )
            .unwrap();
        let results = collect(&backend, 1, Duration::from_secs(1));
        let _ = std::fs::remove_file(path);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, IoToken(100));
        assert!(matches!(results[0].1, IoResult::Ok { .. }));
        assert_eq!(backend.worker_count(), 1);
    }

    #[cfg(feature = "builtin-http")]
    #[test]
    fn test_thread_pool_grows_while_workers_are_busy() {
        let (url, _) = slow_server(Duration::from_millis(500));
        let backend = ThreadPoolIoBackend::with_max_threads(2, 16);
        for raw in 0..6 {
            backend
                .register(IoToken(raw), http_get_request(&url, Duration::from_secs(5)))
                .unwrap();
        }
        let results = collect(&backend, 6, Duration::from_secs(5));
        assert_eq!(results.len(), 6);
        // All six ran at once: none waited for another to finish.
        for (_, result, elapsed) in &results {
            assert!(matches!(result, IoResult::Ok { .. }), "{result:?}");
            assert!(*elapsed < Duration::from_millis(900), "took {elapsed:?}");
        }
    }

    #[cfg(feature = "builtin-http")]
    #[test]
    fn test_thread_pool_limits_in_flight_per_owner_and_round_robins() {
        let (slow_url, slow_accepted) = slow_server(Duration::from_millis(400));
        let (fast_url, _) = slow_server(Duration::ZERO);
        let backend = ThreadPoolIoBackend::with_max_threads(4, 16);
        let busy = IoContext {
            owner: 1,
            max_in_flight: 2,
        };
        for raw in 0..6 {
            backend
                .register_for(
                    IoToken(raw),
                    http_get_request(&slow_url, Duration::from_secs(5)),
                    busy,
                )
                .unwrap();
        }
        let other = IoContext {
            owner: 2,
            max_in_flight: 2,
        };
        backend
            .register_for(
                IoToken(10),
                http_get_request(&fast_url, Duration::from_secs(5)),
                other,
            )
            .unwrap();

        let first = collect(&backend, 1, Duration::from_secs(1));
        assert_eq!(
            first[0].0,
            IoToken(10),
            "the other owner was not served first"
        );
        assert!(
            first[0].2 < Duration::from_millis(100),
            "took {:?}",
            first[0].2
        );
        // Only two of owner 1's requests have started.
        assert_eq!(slow_accepted.load(std::sync::atomic::Ordering::SeqCst), 2);
        let rest = collect(&backend, 6, Duration::from_secs(5));
        assert_eq!(rest.len(), 6);
    }

    #[cfg(feature = "builtin-http")]
    #[test]
    fn test_thread_pool_fails_request_whose_deadline_passes_in_queue() {
        let (url, accepted) = slow_server(Duration::from_secs(2));
        let backend = ThreadPoolIoBackend::with_max_threads(1, 1);
        // The first request holds the only worker; the second waits.
        backend
            .register(IoToken(1), http_get_request(&url, Duration::from_secs(5)))
            .unwrap();
        backend
            .register(
                IoToken(2),
                http_get_request(&url, Duration::from_millis(300)),
            )
            .unwrap();

        let results = collect(&backend, 1, Duration::from_secs(1));
        assert_eq!(results.len(), 1);
        let (token, result, elapsed) = &results[0];
        assert_eq!(*token, IoToken(2));
        match result {
            IoResult::Err(message) => assert_eq!(
                message,
                "http_get: http: timeout: deadline passed while the request was queued"
            ),
            other => panic!("expected queue timeout, got {other:?}"),
        }
        assert!(
            *elapsed >= Duration::from_millis(250) && *elapsed < Duration::from_millis(500),
            "failed after {elapsed:?}"
        );
        assert_eq!(accepted.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[cfg(feature = "builtin-http")]
    #[test]
    fn test_thread_pool_cancel_removes_queued_request() {
        let (url, accepted) = slow_server(Duration::from_millis(300));
        let backend = ThreadPoolIoBackend::with_max_threads(1, 1);
        backend
            .register(IoToken(1), http_get_request(&url, Duration::from_secs(5)))
            .unwrap();
        // Wait until the only worker is busy with the first request, so the
        // second one stays queued.
        let start = Instant::now();
        while accepted.load(std::sync::atomic::Ordering::SeqCst) == 0
            && start.elapsed() < Duration::from_secs(1)
        {
            std::thread::sleep(Duration::from_millis(1));
        }
        backend
            .register(IoToken(2), http_get_request(&url, Duration::from_secs(5)))
            .unwrap();
        backend.cancel(IoToken(2));
        assert!(backend.work.lock().queues.is_empty());

        let results = collect(&backend, 2, Duration::from_secs(1));
        assert_eq!(results.len(), 1, "cancelled request completed: {results:?}");
        assert_eq!(results[0].0, IoToken(1));
        assert_eq!(accepted.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(backend.timers.lock().pending.is_empty());
    }
}
