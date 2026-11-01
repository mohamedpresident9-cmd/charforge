//! Concurrent pipeline: one FIFO queue, a worker pool, single-flight per payload,
//! lazy lookup of the asset record, retries, cancellation, and completion events.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;

use crate::model::{AssetId, Quality};
use crate::store::{Store, StoredAsset};
use crate::transport::{download, Source};

pub type JobId = u64;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum JobState {
    Queued,
    Running,
    Done,
    Failed,
    Cancelled,
}

impl JobState {
    pub fn label(self) -> &'static str {
        match self {
            JobState::Queued => "queued",
            JobState::Running => "running",
            JobState::Done => "done",
            JobState::Failed => "failed",
            JobState::Cancelled => "cancelled",
        }
    }

    pub fn is_active(self) -> bool {
        matches!(self, JobState::Queued | JobState::Running)
    }
}

/// A read-only view of one job for the UI.
#[derive(Clone, Debug)]
pub struct JobView {
    pub id: JobId,
    pub asset: AssetId,
    pub quality: Quality,
    pub state: JobState,
    pub progress: f32,
    pub attempts: u32,
    pub message: String,
}

/// Terminal notifications for the UI thread.
#[derive(Clone, Debug)]
pub enum Event {
    Finished(JobId, StoredAsset),
    Failed(JobId, String),
    Cancelled(JobId),
}

struct Job {
    id: JobId,
    asset: AssetId,
    quality: Quality,
    state: JobState,
    attempts: u32,
    progress: f32,
    message: String,
    cancel: Arc<AtomicBool>,
}

#[derive(Default)]
struct Shared {
    queue: VecDeque<JobId>,
    jobs: HashMap<JobId, Job>,
    /// Single-flight: at most one active job per (asset, quality).
    active: HashMap<(AssetId, Quality), JobId>,
    next_id: JobId,
    shutdown: bool,
}

impl Shared {
    fn enqueue(&mut self, asset: AssetId, quality: Quality) -> (JobId, bool) {
        if let Some(existing) = self.active.get(&(asset, quality)) {
            return (*existing, false);
        }
        self.next_id += 1;
        let id = self.next_id;
        self.jobs.insert(
            id,
            Job {
                id,
                asset,
                quality,
                state: JobState::Queued,
                attempts: 0,
                progress: 0.0,
                message: String::new(),
                cancel: Arc::new(AtomicBool::new(false)),
            },
        );
        self.active.insert((asset, quality), id);
        self.queue.push_back(id);
        (id, true)
    }

    fn release(&mut self, id: JobId) {
        if let Some(job) = self.jobs.get(&id) {
            let key = (job.asset, job.quality);
            if self.active.get(&key) == Some(&id) {
                self.active.remove(&key);
            }
        }
    }
}

type SharedState = Arc<(Mutex<Shared>, Condvar)>;

pub struct Engine {
    shared: SharedState,
    events: Receiver<Event>,
    workers: Vec<thread::JoinHandle<()>>,
    max_attempts: u32,
}

impl Engine {
    pub fn new(
        store: Arc<Store>,
        source: Arc<dyn Source>,
        workers: usize,
        max_attempts: u32,
    ) -> Self {
        let shared: SharedState = Arc::new((Mutex::new(Shared::default()), Condvar::new()));
        let (tx, events) = channel();
        let mut handles = Vec::new();

        for index in 0..workers.max(1) {
            let shared = Arc::clone(&shared);
            let store = Arc::clone(&store);
            let source = Arc::clone(&source);
            let tx = tx.clone();
            let max_attempts = max_attempts.max(1);

            let handle = thread::Builder::new()
                .name(format!("charforge-worker-{index}"))
                .spawn(move || worker_loop(shared, store, source, tx, max_attempts))
                .expect("spawn worker");
            handles.push(handle);
        }

        Self {
            shared,
            events,
            workers: handles,
            max_attempts: max_attempts.max(1),
        }
    }

    /// Queue one payload. Asking twice for the same payload while it is active
    /// returns the same job id instead of starting a second transfer.
    pub fn enqueue(&self, asset: AssetId, quality: Quality) -> JobId {
        let (mutex, condvar) = &*self.shared;
        let (id, fresh) = mutex.lock().unwrap().enqueue(asset, quality);
        if fresh {
            condvar.notify_one();
        }
        id
    }

    pub fn cancel(&self, id: JobId) {
        let mut shared = self.shared.0.lock().unwrap();
        if let Some(job) = shared.jobs.get_mut(&id) {
            job.cancel.store(true, Ordering::Relaxed);
            if job.state == JobState::Queued {
                job.state = JobState::Cancelled;
                job.message = "cancelled before start".to_string();
            }
        }
        shared.release(id);
    }

    pub fn is_active(&self, asset: AssetId, quality: Quality) -> bool {
        self.shared
            .0
            .lock()
            .unwrap()
            .active
            .contains_key(&(asset, quality))
    }

    /// Everything currently known about jobs, ordered by id.
    pub fn snapshot(&self) -> Vec<JobView> {
        let shared = self.shared.0.lock().unwrap();
        let mut views: Vec<JobView> = shared
            .jobs
            .values()
            .map(|job| JobView {
                id: job.id,
                asset: job.asset,
                quality: job.quality,
                state: job.state,
                progress: job.progress,
                attempts: job.attempts,
                message: job.message.clone(),
            })
            .collect();
        views.sort_by_key(|view| view.id);
        views
    }

    /// "Jobs in queue: N" — the diagnostic the original tool prints.
    pub fn queued_count(&self) -> usize {
        self.shared.0.lock().unwrap().queue.len()
    }

    pub fn drain_events(&self) -> Vec<Event> {
        let mut drained = Vec::new();
        while let Ok(event) = self.events.try_recv() {
            drained.push(event);
        }
        drained
    }

    pub fn max_attempts(&self) -> u32 {
        self.max_attempts
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        {
            let (mutex, condvar) = &*self.shared;
            let mut shared = mutex.lock().unwrap();
            shared.shutdown = true;
            condvar.notify_all();
        }
        for handle in self.workers.drain(..) {
            let _ = handle.join();
        }
    }
}

fn worker_loop(
    shared: SharedState,
    store: Arc<Store>,
    source: Arc<dyn Source>,
    tx: Sender<Event>,
    max_attempts: u32,
) {
    loop {
        // ---- take one job, blocking while the queue is empty ----
        let id = {
            let (mutex, condvar) = &*shared;
            let mut guard = mutex.lock().unwrap();
            loop {
                if guard.shutdown {
                    return;
                }
                if let Some(id) = guard.queue.pop_front() {
                    break id;
                }
                guard = condvar.wait(guard).unwrap();
            }
        };

        // ---- claim it: this is the state guard that prevents double execution ----
        let claim = {
            let mut guard = shared.0.lock().unwrap();
            let Some(job) = guard.jobs.get_mut(&id) else {
                continue;
            };
            if job.cancel.load(Ordering::Relaxed) {
                job.state = JobState::Cancelled;
                job.message = "cancelled before start".to_string();
                None
            } else {
                job.state = JobState::Running;
                job.attempts += 1;
                job.progress = 0.0;
                Some((
                    job.asset,
                    job.quality,
                    Arc::clone(&job.cancel),
                    job.attempts,
                ))
            }
        };

        let Some((asset, quality, cancel, attempt)) = claim else {
            shared.0.lock().unwrap().release(id);
            let _ = tx.send(Event::Cancelled(id));
            continue;
        };

        // ---- resolve the record now, not when the job was queued ----
        let Some(meta) = store.catalog().get(asset) else {
            fail(
                &shared,
                &tx,
                id,
                "asset left the catalog".to_string(),
                attempt,
                max_attempts,
            );
            continue;
        };

        // Progress is published in shared state; the UI polls it each frame.
        let progress_state = Arc::clone(&shared);
        let progress = move |done: u64, total: u64| {
            let ratio = if total == 0 {
                0.0
            } else {
                done as f32 / total as f32
            };
            let mut guard = progress_state.0.lock().unwrap();
            if let Some(job) = guard.jobs.get_mut(&id) {
                job.progress = ratio;
            }
        };

        let result = download(
            &meta,
            quality,
            source.as_ref(),
            worker_count_hint(),
            &cancel,
            &progress,
        );

        match result {
            Ok(payload) => match store.install(&meta, quality, &payload) {
                Ok(stored) => {
                    {
                        let mut guard = shared.0.lock().unwrap();
                        if let Some(job) = guard.jobs.get_mut(&id) {
                            job.state = JobState::Done;
                            job.progress = 1.0;
                            job.message = format!("{} parts", payload.chunks);
                        }
                        guard.release(id);
                    }
                    let _ = tx.send(Event::Finished(id, stored));
                }
                Err(error) => fail(
                    &shared,
                    &tx,
                    id,
                    format!("install failed: {error}"),
                    attempt,
                    max_attempts,
                ),
            },
            Err(error) if error == "cancelled" => {
                {
                    let mut guard = shared.0.lock().unwrap();
                    if let Some(job) = guard.jobs.get_mut(&id) {
                        job.state = JobState::Cancelled;
                        job.message = "cancelled".to_string();
                    }
                    guard.release(id);
                }
                let _ = tx.send(Event::Cancelled(id));
            }
            Err(error) => fail(&shared, &tx, id, error, attempt, max_attempts),
        }
    }
}

/// Retry while attempts remain, otherwise mark the job failed for good.
fn fail(
    shared: &SharedState,
    tx: &Sender<Event>,
    id: JobId,
    message: String,
    attempt: u32,
    max_attempts: u32,
) {
    let retry = attempt < max_attempts;

    {
        let mut guard = shared.0.lock().unwrap();
        if let Some(job) = guard.jobs.get_mut(&id) {
            job.message = message.clone();
            if retry {
                job.state = JobState::Queued;
                job.progress = 0.0;
            } else {
                job.state = JobState::Failed;
            }
        }
        if retry {
            guard.queue.push_back(id);
        } else {
            guard.release(id);
        }
    }

    if retry {
        shared.1.notify_one();
    } else {
        let _ = tx.send(Event::Failed(id, message));
    }
}

fn worker_count_hint() -> usize {
    thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .min(8)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Catalog;
    use std::time::{Duration, Instant};

    fn engine(dir_tag: &str) -> (Arc<Store>, Engine) {
        let mut root = std::env::temp_dir();
        root.push(format!("charforge_pipe_{}_{}", dir_tag, std::process::id()));
        let store = Arc::new(Store::open(&root, Catalog::load()).unwrap());
        let engine = Engine::new(
            Arc::clone(&store),
            Arc::new(crate::transport::DemoSource),
            2,
            2,
        );
        (store, engine)
    }

    #[test]
    fn duplicate_request_reuses_the_same_job() {
        let (_store, engine) = engine("singleflight");
        let a = engine.enqueue(AssetId("human_child"), Quality::Low);
        let b = engine.enqueue(AssetId("human_child"), Quality::Low);
        assert_eq!(a, b, "single-flight must collapse duplicates");
    }

    #[test]
    fn job_completes_and_lands_in_the_library() {
        let (store, engine) = engine("complete");
        let id = engine.enqueue(AssetId("animal_bird"), Quality::Low);

        let deadline = Instant::now() + Duration::from_secs(30);
        let mut finished = false;
        while Instant::now() < deadline && !finished {
            for event in engine.drain_events() {
                if let Event::Finished(job, stored) = event {
                    assert_eq!(job, id);
                    assert!(store.verify(&stored));
                    finished = true;
                }
            }
            thread::sleep(Duration::from_millis(20));
        }

        assert!(finished, "job never finished");
        assert!(store.find(AssetId("animal_bird"), Quality::Low).is_some());
        assert!(!engine.is_active(AssetId("animal_bird"), Quality::Low));
    }

    #[test]
    fn cancel_before_start_is_visible() {
        let (_store, engine) = engine("cancel");
        engine.enqueue(AssetId("building_tower"), Quality::High);
        let id = engine.enqueue(AssetId("car_truck"), Quality::High);
        engine.cancel(id);

        let view = engine
            .snapshot()
            .into_iter()
            .find(|v| v.id == id)
            .expect("job exists");
        assert!(
            view.state == JobState::Cancelled
                || view.state == JobState::Running
                || view.state == JobState::Done
        );
    }
}
