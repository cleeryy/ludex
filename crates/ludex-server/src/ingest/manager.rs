//! Download engine: HTTP Range resume, retries, `.part` files, persistence.
//!
//! This is a port of the standalone `gv-downloader` engine adapted to the Ludex
//! server: jobs live in memory behind a mutex, are persisted to
//! `{data_dir}/ingest-jobs.json`, and at most [`Timings::max_active`] downloads
//! run at once (the rest stay queued and are picked up by a dispatcher task).

use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex, OnceLock};
use std::time::{Duration, Instant};

use chrono::Utc;
use futures_util::StreamExt;
use reqwest::header::{
    ACCEPT, ACCEPT_ENCODING, CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE, RANGE,
};
use reqwest::StatusCode as HttpStatus;
use serde::Serialize;
use tokio::fs;
use tokio::io::{AsyncWriteExt, BufWriter};
use tokio::sync::{Mutex as AsyncMutex, Notify, OwnedSemaphorePermit, Semaphore};
use tokio::task::JoinHandle;
use tracing::{info, warn};
use url::Url;

use super::types::{
    is_private_ip, parse_http_url, sanitize_title, IngestJob, IngestStatus, Timings, JOBS_FILE,
    PARTS_FILE, PART_SUFFIX, USER_AGENT,
};
use super::IngestContext;
use crate::audit::actions;
use crate::events::{Actor, Event};

type JobMap = HashMap<String, IngestJob>;
type PartMap = HashMap<String, String>;
type CancelMap = HashMap<String, Arc<AtomicBool>>;
type TaskMap = HashMap<String, JoinHandle<()>>;
type Registry = StdMutex<HashMap<PathBuf, Arc<Inner>>>;

/// Shared ingest state: one instance per `data_dir`, created by [`shared_state`]
/// so `router()` and `resume_pending()` always talk to the same manager.
pub(crate) struct Inner {
    pub(crate) ctx: IngestContext,
    client: reqwest::Client,
    jobs: StdMutex<JobMap>,
    /// job id -> `.part` file name, persisted so restarts resume the right file.
    parts: StdMutex<PartMap>,
    cancels: StdMutex<CancelMap>,
    /// `.part` names currently owned by a running task (collision guard).
    in_use_parts: StdMutex<HashSet<String>>,
    /// Final library names reserved by a finishing download (overwrite guard).
    final_names: StdMutex<HashSet<String>>,
    /// Jobs with a spawned task (queued jobs are not active).
    active: StdMutex<HashSet<String>>,
    tasks: StdMutex<TaskMap>,
    persist_lock: AsyncMutex<()>,
    semaphore: Arc<Semaphore>,
    queue_notify: Notify,
    cancel_notify: Notify,
    dispatcher_started: AtomicBool,
    timings: Timings,
}

impl Inner {
    fn new(ctx: IngestContext, timings: Timings) -> Self {
        if let Err(error) = std::fs::create_dir_all(&ctx.data_dir) {
            warn!(path = %ctx.data_dir.display(), %error, "could not create the ingest data dir");
        }
        if let Err(error) = std::fs::create_dir_all(&ctx.games_dir) {
            warn!(path = %ctx.games_dir.display(), %error, "could not create the games dir");
        }
        let jobs = load_jobs(&ctx.data_dir);
        let parts = load_parts(&ctx.data_dir);
        Self {
            client: build_client(&timings),
            jobs: StdMutex::new(jobs),
            parts: StdMutex::new(parts),
            cancels: StdMutex::new(HashMap::new()),
            in_use_parts: StdMutex::new(HashSet::new()),
            final_names: StdMutex::new(HashSet::new()),
            active: StdMutex::new(HashSet::new()),
            tasks: StdMutex::new(HashMap::new()),
            persist_lock: AsyncMutex::new(()),
            semaphore: Arc::new(Semaphore::new(timings.max_active.max(1))),
            queue_notify: Notify::new(),
            cancel_notify: Notify::new(),
            dispatcher_started: AtomicBool::new(false),
            ctx,
            timings,
        }
    }

    fn job_count(&self) -> usize {
        lock_unpoisoned(&self.jobs).len()
    }

    pub(crate) fn jobs_sorted(&self) -> Vec<IngestJob> {
        let mut jobs: Vec<IngestJob> = lock_unpoisoned(&self.jobs).values().cloned().collect();
        jobs.sort_by(|left, right| {
            right
                .created_at
                .cmp(&left.created_at)
                .then_with(|| right.id.cmp(&left.id))
        });
        jobs
    }

    pub(crate) fn get_job(&self, id: &str) -> Option<IngestJob> {
        lock_unpoisoned(&self.jobs).get(id).cloned()
    }

    pub(crate) async fn insert_job(&self, job: IngestJob) {
        lock_unpoisoned(&self.jobs).insert(job.id.clone(), job);
        self.persist().await;
    }

    /// Apply `update` to a job and refresh `updatedAt`. Returns the new snapshot.
    pub(crate) fn update_job<F>(&self, id: &str, update: F) -> Option<IngestJob>
    where
        F: FnOnce(&mut IngestJob),
    {
        let mut jobs = lock_unpoisoned(&self.jobs);
        let job = jobs.get_mut(id)?;
        update(job);
        job.updated_at = Utc::now();
        Some(job.clone())
    }

    pub(crate) async fn persist(&self) {
        let _guard = self.persist_lock.lock().await;
        let jobs = self.jobs_sorted();
        let parts = lock_unpoisoned(&self.parts).clone();
        write_json_atomic(&self.ctx.data_dir.join(JOBS_FILE), &jobs).await;
        write_json_atomic(&self.ctx.data_dir.join(PARTS_FILE), &parts).await;
    }

    pub(crate) fn notify_queue(&self) {
        self.queue_notify.notify_one();
    }

    pub(crate) fn cancel_flag(&self, id: &str) -> Arc<AtomicBool> {
        let mut cancels = lock_unpoisoned(&self.cancels);
        cancels
            .entry(id.to_string())
            .or_insert_with(|| Arc::new(AtomicBool::new(false)))
            .clone()
    }

    pub(crate) fn reset_cancel(&self, id: &str) {
        self.cancel_flag(id).store(false, Ordering::SeqCst);
    }

    /// Wake retry sleeps (used when a job is cancelled).
    pub(crate) fn wake_cancelled(&self) {
        self.cancel_notify.notify_waiters();
        self.cancel_notify.notify_one();
    }

    pub(crate) fn take_task(&self, id: &str) -> Option<JoinHandle<()>> {
        lock_unpoisoned(&self.tasks).remove(id)
    }

    fn remove_task(&self, id: &str) {
        lock_unpoisoned(&self.tasks).remove(id);
    }

    pub(crate) fn part_name(&self, id: &str) -> Option<String> {
        lock_unpoisoned(&self.parts).get(id).cloned()
    }

    pub(crate) fn is_part_in_use(&self, name: &str) -> bool {
        lock_unpoisoned(&self.in_use_parts).contains(name)
    }

    pub(crate) fn active_count(&self) -> u64 {
        let jobs = lock_unpoisoned(&self.jobs);
        jobs.values()
            .filter(|job| {
                matches!(
                    job.status,
                    IngestStatus::Downloading | IngestStatus::Verifying
                )
            })
            .count() as u64
    }

    /// Reserve the `.part` file for `id`, reusing the persisted mapping when it
    /// is free, otherwise suffixing with ` (2)`, ` (3)`, ... on collision.
    fn claim_part(&self, id: &str, title: &str) -> String {
        if let Some(name) = self.part_name(id) {
            let mut in_use = lock_unpoisoned(&self.in_use_parts);
            if !in_use.contains(&name) {
                in_use.insert(name.clone());
                return name;
            }
        }
        let mut in_use = lock_unpoisoned(&self.in_use_parts);
        let mut index = 1u32;
        loop {
            let candidate = if index == 1 {
                format!(".{title}{PART_SUFFIX}")
            } else {
                format!(".{title} ({index}){PART_SUFFIX}")
            };
            if !in_use.contains(&candidate) {
                in_use.insert(candidate.clone());
                drop(in_use);
                lock_unpoisoned(&self.parts).insert(id.to_string(), candidate.clone());
                return candidate;
            }
            index = index.saturating_add(1);
        }
    }

    /// Drop the in-use reservation but keep the id -> file mapping so a retry
    /// resumes the same partial file.
    fn release_part(&self, name: &str) {
        lock_unpoisoned(&self.in_use_parts).remove(name);
    }

    /// Pick a free library file name and reserve it until the rename happens.
    /// Two downloads finishing at the same time with the same title get
    /// `Title.zip` and `Title (2).zip` instead of overwriting each other.
    async fn reserve_final_name(&self, base: &str, extension: &str) -> String {
        let mut index = 1u32;
        loop {
            let candidate = if index == 1 {
                format!("{base}.{extension}")
            } else {
                format!("{base} ({index}).{extension}")
            };
            if !lock_unpoisoned(&self.final_names).contains(&candidate)
                && !fs::try_exists(self.ctx.games_dir.join(&candidate))
                    .await
                    .unwrap_or(false)
            {
                let mut reserved = lock_unpoisoned(&self.final_names);
                if reserved.insert(candidate.clone()) {
                    return candidate;
                }
            }
            index = index.saturating_add(1);
            if index > 10_000 {
                return candidate;
            }
        }
    }

    fn release_final_name(&self, name: &str) {
        lock_unpoisoned(&self.final_names).remove(name);
    }

    fn forget_part(&self, id: &str) {
        let removed = lock_unpoisoned(&self.parts).remove(id);
        if let Some(name) = removed {
            lock_unpoisoned(&self.in_use_parts).remove(&name);
        }
    }

    fn next_queued(&self) -> Option<String> {
        let candidates: Vec<String> = {
            let jobs = lock_unpoisoned(&self.jobs);
            let mut queued: Vec<&IngestJob> = jobs
                .values()
                .filter(|job| job.status == IngestStatus::Queued)
                .collect();
            queued.sort_by_key(|job| job.created_at);
            queued.iter().map(|job| job.id.clone()).collect()
        };
        let active = lock_unpoisoned(&self.active);
        candidates.into_iter().find(|id| !active.contains(id))
    }

    fn activate(&self, id: &str) -> bool {
        let queued = lock_unpoisoned(&self.jobs)
            .get(id)
            .is_some_and(|job| job.status == IngestStatus::Queued);
        if !queued {
            return false;
        }
        let mut active = lock_unpoisoned(&self.active);
        if active.contains(id) {
            return false;
        }
        active.insert(id.to_string());
        true
    }

    fn deactivate(&self, id: &str) {
        lock_unpoisoned(&self.active).remove(id);
        self.queue_notify.notify_one();
    }

    /// Clear the active marker without a running task (used after an abort).
    pub(crate) fn release_active(&self, id: &str) {
        self.deactivate(id);
    }
}

// ---------------------------------------------------------------------------
// Shared state registry
// ---------------------------------------------------------------------------

fn registry() -> &'static Registry {
    static REGISTRY: OnceLock<Registry> = OnceLock::new();
    REGISTRY.get_or_init(|| StdMutex::new(HashMap::new()))
}

fn lock_unpoisoned<T>(mutex: &StdMutex<T>) -> std::sync::MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

pub(crate) fn shared_state(ctx: &IngestContext) -> Arc<Inner> {
    shared_state_with(ctx, Timings::default())
}

fn shared_state_with(ctx: &IngestContext, timings: Timings) -> Arc<Inner> {
    let mut registry = lock_unpoisoned(registry());
    registry
        .entry(ctx.data_dir.clone())
        .or_insert_with(|| Arc::new(Inner::new(ctx.clone(), timings)))
        .clone()
}

#[cfg(test)]
pub(crate) fn state_for_tests(ctx: &IngestContext, timings: Timings) -> Arc<Inner> {
    shared_state_with(ctx, timings)
}

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

fn build_client(timings: &Timings) -> reqwest::Client {
    let builder = reqwest::Client::builder()
        .http1_only()
        .redirect(reqwest::redirect::Policy::limited(10))
        .connect_timeout(timings.connect_timeout)
        .tcp_keepalive(Duration::from_secs(30))
        .user_agent(USER_AGENT)
        // Archives must be stored byte for byte: never let reqwest negotiate
        // and transparently decompress the response body.
        .gzip(false);
    builder.build().unwrap_or_else(|error| {
        warn!(%error, "could not build the ingest HTTP client; falling back to defaults");
        reqwest::Client::new()
    })
}

// ---------------------------------------------------------------------------
// Boot
// ---------------------------------------------------------------------------

/// Normalize interrupted jobs to `queued`, then let the dispatcher resume them
/// from their existing `.part` files. Idempotent: jobs with a running task are
/// left untouched, so calling it again after an in-process restart is safe.
pub(crate) async fn resume_pending(state: &Arc<Inner>) {
    let active: HashSet<String> = lock_unpoisoned(&state.active).clone();
    let mut changed = false;
    {
        let mut jobs = lock_unpoisoned(&state.jobs);
        for job in jobs.values_mut() {
            if active.contains(&job.id) {
                continue;
            }
            match job.status {
                IngestStatus::Queued | IngestStatus::Downloading => {
                    job.status = IngestStatus::Queued;
                    job.error = None;
                    job.attempt = 0;
                    job.speed_bps = 0;
                    job.updated_at = Utc::now();
                    changed = true;
                }
                IngestStatus::Verifying => {
                    let already_moved = job
                        .file_name
                        .as_ref()
                        .is_some_and(|name| state.ctx.games_dir.join(name).is_file());
                    job.status = if already_moved {
                        IngestStatus::Done
                    } else {
                        IngestStatus::Queued
                    };
                    job.error = None;
                    job.speed_bps = 0;
                    job.updated_at = Utc::now();
                    changed = true;
                }
                IngestStatus::Done | IngestStatus::Error | IngestStatus::Cancelled => {}
            }
        }
    }
    if changed {
        state.persist().await;
    }
    info!(jobs = state.job_count(), "ingest job history loaded");
    ensure_dispatcher(state);
    state.notify_queue();
}

// ---------------------------------------------------------------------------
// Dispatcher
// ---------------------------------------------------------------------------

/// Spawn the single dispatcher task for this state (idempotent). It picks the
/// oldest queued job, waits for a free slot, and spawns its download task.
pub(crate) fn ensure_dispatcher(state: &Arc<Inner>) {
    if state.dispatcher_started.swap(true, Ordering::SeqCst) {
        return;
    }
    let state = Arc::clone(state);
    tokio::spawn(async move {
        loop {
            let Some(id) = state.next_queued() else {
                state.queue_notify.notified().await;
                continue;
            };
            let permit = match Arc::clone(&state.semaphore).acquire_owned().await {
                Ok(permit) => permit,
                Err(_) => return,
            };
            if !state.activate(&id) {
                drop(permit);
                continue;
            }
            let run_state = Arc::clone(&state);
            let task_id = id.clone();
            let handle = tokio::spawn(async move { run_job(run_state, task_id, permit).await });
            lock_unpoisoned(&state.tasks).insert(id, handle);
        }
    });
}

// ---------------------------------------------------------------------------
// Job driver
// ---------------------------------------------------------------------------

enum Outcome {
    Done(Option<String>),
    Fatal(String),
    Cancelled,
}

enum AttemptError {
    Retry(String),
    Fatal(String),
}

struct AttemptMeta {
    content_type: Option<String>,
}

async fn run_job(state: Arc<Inner>, id: String, _permit: OwnedSemaphorePermit) {
    let cancel = state.cancel_flag(&id);
    let outcome = drive_job(&state, &id, &cancel).await;
    match outcome {
        Outcome::Done(content_type) => finish_download(&state, &id, content_type.as_deref()).await,
        Outcome::Fatal(message) => fail_job(&state, &id, message).await,
        Outcome::Cancelled => cancel_cleanup(&state, &id).await,
    }
    state.remove_task(&id);
    state.deactivate(&id);
}

async fn drive_job(state: &Arc<Inner>, id: &str, cancel: &AtomicBool) -> Outcome {
    let Some(job) = state.get_job(id) else {
        return Outcome::Cancelled;
    };
    if cancel.load(Ordering::SeqCst)
        || !matches!(job.status, IngestStatus::Queued | IngestStatus::Downloading)
    {
        return Outcome::Cancelled;
    }
    let title = sanitize_title(&job.title);
    let part_name = state.claim_part(id, &title);
    let part_path = state.ctx.games_dir.join(&part_name);
    let mut attempt = job.attempt;
    let mut backoff_step = 0u32;

    loop {
        if cancel.load(Ordering::SeqCst) {
            return Outcome::Cancelled;
        }
        let Some(current) = state.get_job(id) else {
            return Outcome::Cancelled;
        };
        if current.status == IngestStatus::Cancelled {
            return Outcome::Cancelled;
        }
        let Some(raw_url) = current.url.clone() else {
            return Outcome::Fatal("This download has no URL".to_string());
        };
        let url = match parse_http_url(&raw_url) {
            Ok(url) => url,
            Err(message) => return Outcome::Fatal(message),
        };

        attempt = attempt.saturating_add(1);
        let _ = state.update_job(id, |job| {
            job.status = IngestStatus::Downloading;
            job.attempt = attempt;
            job.error = None;
            job.speed_bps = 0;
        });
        state.persist().await;

        let bytes_before = file_len(&part_path).await;
        match attempt_once(state, id, &url, &part_path, cancel).await {
            Ok(meta) => return Outcome::Done(meta.content_type),
            Err(AttemptError::Fatal(message)) => return Outcome::Fatal(message),
            Err(AttemptError::Retry(message)) => {
                if cancel.load(Ordering::SeqCst) {
                    return Outcome::Cancelled;
                }
                // Backoff resets as soon as an attempt made progress, mirroring
                // `curl --retry-all-errors --continue-at -`.
                if file_len(&part_path).await > bytes_before {
                    backoff_step = 0;
                }
                let delay = state.timings.retry_delay(backoff_step);
                backoff_step = backoff_step.saturating_add(1);
                let _ = state.update_job(id, |job| {
                    job.error = Some(format!("{message} — retrying"));
                    job.speed_bps = 0;
                });
                state.persist().await;
                warn!(job = id, attempt, error = %message, "download attempt failed; retrying");
                if !sleep_or_cancel(state, delay, cancel).await {
                    return Outcome::Cancelled;
                }
            }
        }
    }
}

async fn sleep_or_cancel(state: &Arc<Inner>, delay: Duration, cancel: &AtomicBool) -> bool {
    let deadline = tokio::time::Instant::now() + delay;
    loop {
        if cancel.load(Ordering::SeqCst) {
            return false;
        }
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return true;
        }
        tokio::select! {
            () = tokio::time::sleep(remaining) => return true,
            () = state.cancel_notify.notified() => {}
        }
    }
}

// ---------------------------------------------------------------------------
// One HTTP attempt
// ---------------------------------------------------------------------------

async fn attempt_once(
    state: &Arc<Inner>,
    id: &str,
    url: &Url,
    part_path: &Path,
    cancel: &AtomicBool,
) -> Result<AttemptMeta, AttemptError> {
    ensure_target_allowed(state, url).await?;
    let existing = file_len(part_path).await;

    let mut request = state
        .client
        .get(url.as_str())
        .header(ACCEPT, "*/*")
        .header(ACCEPT_ENCODING, "identity");
    if existing > 0 {
        request = request.header(RANGE, format!("bytes={existing}-"));
    }
    let response = request
        .send()
        .await
        .map_err(|error| AttemptError::Retry(format!("connection failed: {error}")))?;
    let status = response.status();

    if status == HttpStatus::RANGE_NOT_SATISFIABLE {
        if existing == 0 {
            return Err(AttemptError::Fatal(
                "the server rejected the range request (HTTP 416)".to_string(),
            ));
        }
        // The partial file is already complete.
        let _ = state.update_job(id, |job| {
            job.downloaded_bytes = existing;
            job.total_bytes = Some(existing);
        });
        return Ok(AttemptMeta { content_type: None });
    }

    if response.url() != url {
        ensure_target_allowed(state, response.url()).await?;
    }

    if status.is_client_error() {
        if matches!(
            status,
            HttpStatus::REQUEST_TIMEOUT | HttpStatus::TOO_MANY_REQUESTS
        ) {
            return Err(AttemptError::Retry(format!("HTTP {status}")));
        }
        return Err(AttemptError::Fatal(format!(
            "HTTP {status}: the download link was rejected. It may have expired; update the URL and retry."
        )));
    }
    if status.is_server_error() {
        return Err(AttemptError::Retry(format!("HTTP {status}")));
    }
    if status != HttpStatus::OK && status != HttpStatus::PARTIAL_CONTENT {
        return Err(AttemptError::Retry(format!("unexpected HTTP {status}")));
    }

    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);

    let total: Option<u64> = if status == HttpStatus::PARTIAL_CONTENT {
        match response
            .headers()
            .get(CONTENT_RANGE)
            .and_then(|value| value.to_str().ok())
            .and_then(parse_content_range)
        {
            Some((start, range_total)) => {
                if start != existing {
                    return Err(AttemptError::Retry(format!(
                        "server resumed at an unexpected offset ({start}, expected {existing})"
                    )));
                }
                range_total
            }
            None => {
                return Err(AttemptError::Retry(
                    "server sent an invalid Content-Range header".to_string(),
                ));
            }
        }
    } else {
        response
            .headers()
            .get(CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.trim().parse().ok())
    };

    let resumed = status == HttpStatus::PARTIAL_CONTENT && existing > 0;
    let restarted = existing > 0 && !resumed;
    if restarted {
        warn!(
            job = id,
            "server ignored the Range header; restarting from zero"
        );
    }

    let _ = state.update_job(id, |job| {
        job.downloaded_bytes = if resumed { existing } else { 0 };
        if total.is_some() {
            job.total_bytes = total;
        }
        if restarted {
            job.error = Some("Server does not support resume; restarting from zero".to_string());
        }
    });

    let file = if resumed {
        fs::OpenOptions::new().append(true).open(part_path).await
    } else {
        fs::File::create(part_path).await
    }
    .map_err(|error| AttemptError::Fatal(format!("cannot open the partial file: {error}")))?;

    let buffer_size = state.timings.buffer_size.max(64 * 1024);
    let mut writer = BufWriter::with_capacity(buffer_size, file);
    let mut downloaded = if resumed { existing } else { 0 };
    let mut interval_bytes: u64 = 0;
    let mut last_report = Instant::now();
    let mut last_flush = Instant::now();
    let mut reported_once = false;
    let mut speed = 0.0f64;
    let mut stream = response.bytes_stream();

    let stream_error = loop {
        if cancel.load(Ordering::SeqCst) {
            break Some(AttemptError::Retry("cancelled".to_string()));
        }
        let next = match tokio::time::timeout(state.timings.stall_timeout, stream.next()).await {
            Ok(next) => next,
            Err(_) => {
                break Some(AttemptError::Retry(format!(
                    "no data received for {}s",
                    state.timings.stall_timeout.as_secs()
                )))
            }
        };
        let Some(chunk) = next else {
            break None;
        };
        let chunk = match chunk {
            Ok(chunk) => chunk,
            Err(error) => break Some(AttemptError::Retry(format!("read failed: {error}"))),
        };
        if let Err(error) = writer.write_all(&chunk).await {
            break Some(AttemptError::Fatal(format!("disk write failed: {error}")));
        }
        downloaded += chunk.len() as u64;
        interval_bytes += chunk.len() as u64;

        if !reported_once || last_report.elapsed() >= state.timings.progress_interval {
            let elapsed = last_report.elapsed().as_secs_f64().max(0.05);
            let instant = interval_bytes as f64 / elapsed;
            speed = if speed <= 0.0 {
                instant
            } else {
                speed * 0.7 + instant * 0.3
            };
            let reported = speed as u64;
            reported_once = true;
            last_report = Instant::now();
            interval_bytes = 0;
            let _ = state.update_job(id, |job| {
                job.downloaded_bytes = downloaded;
                job.speed_bps = reported;
            });
        }
        if last_flush.elapsed() >= state.timings.flush_interval {
            if let Err(error) = writer.flush().await {
                break Some(AttemptError::Fatal(format!("flush failed: {error}")));
            }
            last_flush = Instant::now();
        }
    };

    if let Err(error) = writer.flush().await {
        return Err(AttemptError::Fatal(format!("flush failed: {error}")));
    }
    drop(writer);
    if let Some(error) = stream_error {
        return Err(error);
    }

    let final_len = file_len(part_path).await;
    if final_len == 0 {
        return Err(AttemptError::Retry(
            "the server returned an empty body".to_string(),
        ));
    }
    if let Some(expected) = total {
        if final_len < expected {
            return Err(AttemptError::Retry(format!(
                "connection closed early ({final_len}/{expected} bytes)"
            )));
        }
    }
    let _ = state.update_job(id, |job| {
        job.downloaded_bytes = final_len;
        job.speed_bps = 0;
        if job.total_bytes.is_none() {
            job.total_bytes = Some(final_len);
        }
    });
    Ok(AttemptMeta { content_type })
}

async fn ensure_target_allowed(state: &Arc<Inner>, url: &Url) -> Result<(), AttemptError> {
    if !matches!(url.scheme(), "http" | "https") {
        return Err(AttemptError::Fatal(
            "Only http:// and https:// URLs are allowed".to_string(),
        ));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(AttemptError::Fatal(
            "URLs with embedded credentials are not allowed".to_string(),
        ));
    }
    let Some(host) = url.host() else {
        return Err(AttemptError::Fatal("URL has no host".to_string()));
    };
    match host {
        url::Host::Ipv4(address) => {
            if !state.ctx.allow_private && is_private_ip(IpAddr::V4(address)) {
                return Err(AttemptError::Fatal(format!(
                    "refusing to download from the private address {address}"
                )));
            }
            Ok(())
        }
        url::Host::Ipv6(address) => {
            if !state.ctx.allow_private && is_private_ip(IpAddr::V6(address)) {
                return Err(AttemptError::Fatal(format!(
                    "refusing to download from the private address {address}"
                )));
            }
            Ok(())
        }
        url::Host::Domain(domain) => {
            let port = url.port_or_known_default().unwrap_or(80);
            let addresses = tokio::net::lookup_host((domain, port))
                .await
                .map_err(|error| {
                    AttemptError::Retry(format!("could not resolve {domain}: {error}"))
                })?;
            let mut resolved = false;
            for address in addresses {
                resolved = true;
                if !state.ctx.allow_private && is_private_ip(address.ip()) {
                    return Err(AttemptError::Fatal(format!(
                        "refusing to download from the private address {} ({domain})",
                        address.ip()
                    )));
                }
            }
            if !resolved {
                return Err(AttemptError::Retry(format!("could not resolve {domain}")));
            }
            Ok(())
        }
    }
}

/// Reject an IP-literal URL pointing at a private address before the job is
/// created. Hostnames are resolved again for every attempt.
pub(crate) fn reject_private_literal(state: &Inner, url: &Url) -> Result<(), String> {
    if state.ctx.allow_private {
        return Ok(());
    }
    match url.host() {
        Some(url::Host::Ipv4(address)) if is_private_ip(IpAddr::V4(address)) => Err(format!(
            "refusing to download from the private address {address}"
        )),
        Some(url::Host::Ipv6(address)) if is_private_ip(IpAddr::V6(address)) => Err(format!(
            "refusing to download from the private address {address}"
        )),
        _ => Ok(()),
    }
}

pub(crate) fn parse_content_range(value: &str) -> Option<(u64, Option<u64>)> {
    let rest = value.trim().strip_prefix("bytes")?.trim_start();
    let (range, total) = rest.split_once('/')?;
    let (start, _end) = range.split_once('-')?;
    let start = start.trim().parse().ok()?;
    let total = total.trim();
    let total = if total == "*" {
        None
    } else {
        Some(total.parse().ok()?)
    };
    Some((start, total))
}

// ---------------------------------------------------------------------------
// Completion
// ---------------------------------------------------------------------------

async fn finish_download(state: &Arc<Inner>, id: &str, content_type: Option<&str>) {
    let Some(part_name) = state.part_name(id) else {
        fail_job(
            state,
            id,
            "internal error: the partial file is missing".to_string(),
        )
        .await;
        return;
    };
    let part_path = state.ctx.games_dir.join(&part_name);
    let size = match fs::metadata(&part_path).await {
        Ok(metadata) => metadata.len(),
        Err(error) => {
            fail_job(
                state,
                id,
                format!("cannot read the downloaded file: {error}"),
            )
            .await;
            return;
        }
    };
    if size == 0 {
        fail_job(state, id, "the downloaded file is empty".to_string()).await;
        return;
    }
    let _ = state.update_job(id, |job| {
        job.status = IngestStatus::Verifying;
        job.downloaded_bytes = size;
        job.speed_bps = 0;
    });
    state.persist().await;

    let detect_path = part_path.clone();
    let detected =
        tokio::task::spawn_blocking(move || ludex_core::archive::detect_format(&detect_path)).await;
    let extension = match detected {
        Ok(Ok(format)) => match format.extension() {
            Some(extension) => extension,
            None => {
                warn!(job = id, "downloaded file is not a recognised archive");
                fallback_extension(state, id, content_type)
            }
        },
        Ok(Err(error)) => {
            warn!(job = id, %error, "could not detect the archive format from magic bytes");
            fallback_extension(state, id, content_type)
        }
        Err(error) => {
            warn!(job = id, %error, "archive format detection task failed");
            fallback_extension(state, id, content_type)
        }
    };

    let title = state
        .get_job(id)
        .map(|job| sanitize_title(&job.title))
        .unwrap_or_else(|| "Game".to_string());
    let file_name = state.reserve_final_name(&title, extension).await;
    let final_path = state.ctx.games_dir.join(&file_name);
    let _ = state.update_job(id, |job| {
        job.file_name = Some(file_name.clone());
    });
    state.persist().await;

    if let Err(error) = fs::rename(&part_path, &final_path).await {
        state.release_final_name(&file_name);
        state.release_part(&part_name);
        fail_job(
            state,
            id,
            format!("could not move the finished file into the library: {error}"),
        )
        .await;
        return;
    }
    state.forget_part(id);
    state.release_final_name(&file_name);

    verify_best_effort(state, &final_path).await;

    let final_size = fs::metadata(&final_path)
        .await
        .map(|metadata| metadata.len())
        .unwrap_or(size);
    let _ = state.update_job(id, |job| {
        job.status = IngestStatus::Done;
        job.file_name = Some(file_name.clone());
        job.downloaded_bytes = final_size;
        if job.total_bytes.is_none() {
            job.total_bytes = Some(final_size);
        }
        job.speed_bps = 0;
        job.error = None;
    });
    state.persist().await;
    // Non-blocking: a full scan channel simply drops the request when full.
    let _ = state
        .ctx
        .scan_tx
        .try_send(crate::scanner::ScanRequest::all());
    if let Some(events) = &state.ctx.events {
        let title = state.get_job(id).map(|job| job.title).unwrap_or_default();
        events.emit(
            Event::new(actions::DOWNLOAD_COMPLETED, Actor::System)
                .target("download", id.to_string())
                .metadata(serde_json::json!({
                    "title": title,
                    "fileName": file_name,
                    "bytes": final_size,
                })),
        );
    }
    info!(job = id, file = %file_name, bytes = final_size, "ingest download finished");
}

async fn verify_best_effort(state: &Arc<Inner>, path: &Path) {
    let verify_path = path.to_path_buf();
    let handle =
        tokio::task::spawn_blocking(move || ludex_core::verify_archive(&verify_path, None));
    match tokio::time::timeout(state.timings.verify_timeout, handle).await {
        Ok(Ok(Ok(_))) => info!(file = %path.display(), "archive header verified"),
        Ok(Ok(Err(error))) => warn!(
            file = %path.display(),
            %error,
            "archive verification failed at ingest; file kept"
        ),
        Ok(Err(error)) => warn!(
            file = %path.display(),
            %error,
            "archive verification task failed; file kept"
        ),
        Err(_) => warn!(
            file = %path.display(),
            "archive verification timed out; file kept"
        ),
    }
}

fn fallback_extension(state: &Inner, id: &str, content_type: Option<&str>) -> &'static str {
    if let Some(content_type) = content_type {
        let content_type = content_type.to_ascii_lowercase();
        if content_type.contains("zip") {
            return "zip";
        }
        if content_type.contains("rar") {
            return "rar";
        }
        if content_type.contains("7z") {
            return "7z";
        }
    }
    let url = state
        .get_job(id)
        .and_then(|job| job.url)
        .unwrap_or_default();
    let url = url.to_ascii_lowercase();
    let path = url.split('?').next().unwrap_or(&url);
    if path.ends_with(".rar") {
        "rar"
    } else if path.ends_with(".7z") {
        "7z"
    } else {
        "zip"
    }
}

async fn fail_job(state: &Arc<Inner>, id: &str, message: String) {
    if let Some(name) = state.part_name(id) {
        state.release_part(&name);
    }
    let _ = state.update_job(id, |job| {
        job.status = IngestStatus::Error;
        job.error = Some(message);
        job.speed_bps = 0;
        job.file_name = None;
    });
    state.persist().await;
    warn!(job = id, "ingest download failed");
}

/// Cancel a job: delete its `.part` (and a half-finished library file, if the
/// download already moved), mark it cancelled and persist.
pub(crate) async fn cancel_cleanup(state: &Arc<Inner>, id: &str) {
    let final_name = state.get_job(id).and_then(|job| {
        if job.status == IngestStatus::Done {
            None
        } else {
            job.file_name
        }
    });
    if let Some(part_name) = state.part_name(id) {
        remove_if_exists(&state.ctx.games_dir.join(part_name)).await;
    }
    if let Some(final_name) = final_name {
        remove_if_exists(&state.ctx.games_dir.join(&final_name)).await;
        state.release_final_name(&final_name);
    }
    state.forget_part(id);
    let _ = state.update_job(id, |job| {
        if job.status != IngestStatus::Done {
            job.status = IngestStatus::Cancelled;
            job.error = Some("Cancelled by user".to_string());
            job.file_name = None;
            job.speed_bps = 0;
        }
    });
    state.persist().await;
}

// ---------------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------------

async fn file_len(path: &Path) -> u64 {
    fs::metadata(path)
        .await
        .map(|metadata| metadata.len())
        .unwrap_or(0)
}

async fn remove_if_exists(path: &Path) {
    match fs::remove_file(path).await {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => warn!(path = %path.display(), %error, "could not remove an ingest file"),
    }
}

// ---------------------------------------------------------------------------
// Persistence
// ---------------------------------------------------------------------------

async fn write_json_atomic<T: Serialize>(path: &Path, value: &T) {
    let bytes = match serde_json::to_vec_pretty(value) {
        Ok(bytes) => bytes,
        Err(error) => {
            warn!(path = %path.display(), %error, "could not serialize ingest state");
            return;
        }
    };
    let temporary = path.with_extension("json.tmp");
    if let Err(error) = fs::write(&temporary, bytes).await {
        warn!(path = %temporary.display(), %error, "could not write ingest state");
        return;
    }
    if let Err(error) = fs::rename(&temporary, path).await {
        warn!(path = %path.display(), %error, "could not replace ingest state");
    }
}

fn load_jobs(data_dir: &Path) -> JobMap {
    let path = data_dir.join(JOBS_FILE);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return HashMap::new(),
        Err(error) => {
            warn!(path = %path.display(), %error, "could not read ingest job history");
            return HashMap::new();
        }
    };
    match serde_json::from_slice::<Vec<IngestJob>>(&bytes) {
        Ok(jobs) => jobs.into_iter().map(|job| (job.id.clone(), job)).collect(),
        Err(error) => {
            warn!(path = %path.display(), %error, "could not parse ingest job history");
            HashMap::new()
        }
    }
}

fn load_parts(data_dir: &Path) -> PartMap {
    let path = data_dir.join(PARTS_FILE);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return HashMap::new(),
        Err(error) => {
            warn!(path = %path.display(), %error, "could not read the ingest part map");
            return HashMap::new();
        }
    };
    serde_json::from_slice::<PartMap>(&bytes).unwrap_or_else(|error| {
        warn!(path = %path.display(), %error, "could not parse the ingest part map");
        HashMap::new()
    })
}
