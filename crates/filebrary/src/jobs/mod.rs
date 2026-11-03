//! Background jobs: a persistent queue with progress, cancel and retry.
//!
//! # Model
//!
//! A job is a row in the `jobs` table with a `kind` (for example
//! `convert.video`), JSON `params`, a `status` and a `progress` between 0 and
//! 1. Jobs move through:
//!
//! ```text
//! queued ──▶ running ──▶ succeeded
//!   │           ├──────▶ failed ────┐
//!   └───────────┴──────▶ cancelled ─┴─▶ (retry) queued
//! ```
//!
//! A fixed pool of workers (`jobs.workers`) claims queued jobs oldest first.
//! Jobs that were running when the server stopped are queued again at start.
//! Finished jobs are purged after `jobs.retain_days`.
//!
//! # Adding a job kind
//!
//! Implement [`JobHandler`] and register it with [`JobQueue::register`]
//! (built-in kinds are registered in [`JobQueue::new`]). Other subsystems
//! (indexing, thumbnails) can enqueue system jobs with
//! [`JobQueue::submit_system`].
//!
//! # Security
//!
//! Jobs store the *user-visible* paths the user asked for. Every run loads the
//! user again and re-checks their permissions, so revoking access also stops
//! queued and retried jobs. External programs only ever see private temporary
//! copies with names chosen by the server (see [`process`]).

pub mod api;
pub mod archive;
pub mod config;
pub mod convert;
pub mod custom;
pub mod process;
pub mod stage;

use std::{
    collections::{BTreeMap, HashMap},
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock, RwLock},
    time::{Duration, Instant},
};

use async_trait::async_trait;
use serde::Serialize;
use serde_json::Value;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

pub use config::{CustomJobConfig, CustomOutput, JobsConfig};

use crate::{
    auth::{User, load_user},
    db::now,
    error::{AppError, AppResult},
    state::AppState,
};

/// Why a job did not succeed.
#[derive(Debug, thiserror::Error)]
pub enum JobError {
    /// The job was cancelled.
    #[error("cancelled")]
    Cancelled,
    /// The job failed; the message is shown to the user.
    #[error("{0}")]
    Failed(String),
}

impl JobError {
    /// Shorthand for [`JobError::Failed`].
    pub fn failed(msg: impl Into<String>) -> Self {
        Self::Failed(msg.into())
    }
}

impl From<AppError> for JobError {
    fn from(e: AppError) -> Self {
        match e {
            AppError::Internal(inner) => {
                tracing::error!(error = ?inner, "internal error in job");
                Self::Failed("internal error".into())
            }
            AppError::NotFound => Self::Failed("file or folder not found".into()),
            other => Self::Failed(other.to_string()),
        }
    }
}

impl From<crate::storage::StorageError> for JobError {
    fn from(e: crate::storage::StorageError) -> Self {
        AppError::from(e).into()
    }
}

impl From<std::io::Error> for JobError {
    fn from(e: std::io::Error) -> Self {
        AppError::from(e).into()
    }
}

/// Result alias for job handlers.
pub type JobResult<T> = Result<T, JobError>;

/// A validated job, ready to be queued.
#[derive(Debug, Clone)]
pub struct Prepared {
    /// Short description shown in the job list, e.g. `Convert movie.mkv to MP4`.
    pub title: String,
    /// Normalised parameters, stored and passed to [`JobHandler::run`].
    pub params: Value,
}

/// One kind of job.
#[async_trait]
pub trait JobHandler: Send + Sync + 'static {
    /// Whether this kind can run on this server (for example, ffmpeg is
    /// installed). Unavailable kinds are listed but cannot be submitted.
    fn available(&self, _state: &AppState) -> Result<(), String> {
        Ok(())
    }

    /// Whether only administrators may submit this kind.
    fn admin_only(&self) -> bool {
        false
    }

    /// A short human description for the kinds list.
    fn describe(&self) -> Value {
        Value::Null
    }

    /// Validates `params` for `user` at submit time (including permission
    /// checks), returning what will be stored.
    ///
    /// # Errors
    ///
    /// A [`AppError::BadRequest`], `Forbidden` or `NotFound` that is sent back
    /// to the client.
    async fn prepare(&self, state: &AppState, user: &User, params: Value) -> AppResult<Prepared>;

    /// Runs the job, returning a JSON result (for example the path of the
    /// file it created). Must check [`JobContext::cancel`] regularly.
    async fn run(&self, ctx: &JobContext, params: &Value) -> JobResult<Value>;
}

/// What a running job can see and do.
pub struct JobContext {
    /// Server state.
    pub state: AppState,
    /// The job's id.
    pub id: i64,
    /// The user who submitted the job (`None` for system jobs).
    pub user_id: Option<i64>,
    /// Fires when the job is cancelled.
    pub cancel: CancellationToken,
    /// A private, empty folder for this run, deleted afterwards.
    pub tmp: PathBuf,
    last_flush: Mutex<Option<Instant>>,
}

impl JobContext {
    /// Loads the submitting user with their *current* permissions.
    ///
    /// # Errors
    ///
    /// Fails if the job has no user or the user was deleted or disabled.
    pub async fn user(&self) -> JobResult<User> {
        let id = self
            .user_id
            .ok_or_else(|| JobError::failed("this job has no owner"))?;
        load_user(&self.state, id)
            .await?
            .ok_or_else(|| JobError::failed("the user who started this job no longer exists"))
    }

    /// Records in the activity log that this job wrote `path` (a
    /// user-visible path) for `user`. `action` is an
    /// [`crate::activity::action`] constant, usually `UPLOAD` for a new
    /// file or `EDIT` for a replaced one.
    pub async fn record_write(
        &self,
        user: &User,
        source: &str,
        path: &crate::storage::VPath,
        action: &str,
        kind: &str,
    ) {
        let Some(grant) = user.grant(source) else {
            return;
        };
        crate::activity::Activity::new(action)
            .user(user)
            .at(source, &grant.scope.join_path(path))
            .details(serde_json::json!({ "job": self.id, "kind": kind }))
            .record(&self.state)
            .await;
    }

    /// Fails with [`JobError::Cancelled`] if the job was cancelled.
    ///
    /// # Errors
    ///
    /// See above.
    pub fn check_cancel(&self) -> JobResult<()> {
        if self.cancel.is_cancelled() {
            Err(JobError::Cancelled)
        } else {
            Ok(())
        }
    }

    /// Reports progress (`fraction` from 0 to 1) and an optional message.
    /// Writes are throttled to about two per second.
    pub async fn progress(&self, fraction: f64, message: Option<&str>) {
        {
            let mut last = self.last_flush.lock().expect("lock");
            if last.is_some_and(|t| t.elapsed() < Duration::from_millis(500)) && fraction < 1.0 {
                return;
            }
            *last = Some(Instant::now());
        }
        let fraction = if fraction.is_finite() {
            fraction.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let r = sqlx::query(
            "UPDATE jobs SET progress = ?, message = COALESCE(?, message), updated_at = ? \
             WHERE id = ? AND status = 'running'",
        )
        .bind(fraction)
        .bind(message)
        .bind(now())
        .bind(self.id)
        .execute(&self.state.db)
        .await;
        if let Err(e) = r {
            tracing::warn!(job = self.id, error = %e, "could not save job progress");
        }
    }
}

/// Status of a job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum JobStatus {
    /// Waiting for a worker.
    Queued,
    /// Running now.
    Running,
    /// Finished successfully.
    Succeeded,
    /// Finished with an error.
    Failed,
    /// Cancelled by a user.
    Cancelled,
}

impl JobStatus {
    fn parse(s: &str) -> Self {
        match s {
            "queued" => Self::Queued,
            "running" => Self::Running,
            "succeeded" => Self::Succeeded,
            "cancelled" => Self::Cancelled,
            _ => Self::Failed,
        }
    }

    /// Whether the job has finished (in any way).
    pub fn is_finished(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Cancelled)
    }
}

/// A job as returned by the API.
#[derive(Debug, Clone, Serialize)]
pub struct Job {
    /// Id.
    pub id: i64,
    /// Kind, e.g. `convert.video`.
    pub kind: String,
    /// Human description.
    pub title: String,
    /// Owner (`None` for system jobs).
    pub user_id: Option<i64>,
    /// Current status.
    pub status: JobStatus,
    /// Progress from 0 to 1.
    pub progress: f64,
    /// Latest progress message.
    pub message: Option<String>,
    /// Error message for failed jobs.
    pub error: Option<String>,
    /// Handler result for succeeded jobs.
    pub result: Option<Value>,
    /// Parameters.
    pub params: Value,
    /// Times the job has been started.
    pub attempts: i64,
    /// Creation time (Unix seconds).
    pub created_at: i64,
    /// Start of the latest attempt.
    pub started_at: Option<i64>,
    /// End of the latest attempt.
    pub finished_at: Option<i64>,
}

type JobRow = (
    i64,
    String,
    String,
    Option<i64>,
    String,
    f64,
    Option<String>,
    Option<String>,
    Option<String>,
    String,
    i64,
    i64,
    Option<i64>,
    Option<i64>,
);

/// The columns read into a [`JobRow`], as a literal for `concat!`.
macro_rules! job_columns {
    () => {
        "id, kind, title, user_id, status, progress, message, error, result, \
         params, attempts, created_at, started_at, finished_at"
    };
}

impl From<JobRow> for Job {
    fn from(r: JobRow) -> Self {
        Job {
            id: r.0,
            kind: r.1,
            title: r.2,
            user_id: r.3,
            status: JobStatus::parse(&r.4),
            progress: r.5,
            message: r.6,
            error: r.7,
            result: r.8.and_then(|s| serde_json::from_str(&s).ok()),
            params: serde_json::from_str(&r.9).unwrap_or(Value::Null),
            attempts: r.10,
            created_at: r.11,
            started_at: r.12,
            finished_at: r.13,
        }
    }
}

/// The job queue. Cheap to clone.
#[derive(Clone)]
pub struct JobQueue(Arc<QueueInner>);

struct QueueInner {
    handlers: RwLock<BTreeMap<String, Arc<dyn JobHandler>>>,
    notify: Notify,
    running: Mutex<HashMap<i64, CancellationToken>>,
    started: OnceLock<()>,
    workers: usize,
    retain_days: u64,
}

impl JobQueue {
    /// Creates the queue with the built-in job kinds registered. Workers start
    /// with [`JobQueue::start`].
    pub fn new(cfg: &JobsConfig) -> Self {
        let q = Self(Arc::new(QueueInner {
            handlers: RwLock::new(BTreeMap::new()),
            notify: Notify::new(),
            running: Mutex::new(HashMap::new()),
            started: OnceLock::new(),
            workers: cfg.workers.max(1),
            retain_days: cfg.retain_days,
        }));
        q.register("archive.create", Arc::new(archive::CreateArchive));
        q.register("convert.document", Arc::new(convert::DocumentToPdf));
        q.register("convert.video", Arc::new(convert::Video));
        q.register("convert.image", Arc::new(convert::Image));
        for c in &cfg.custom {
            q.register(
                &format!("custom.{}", c.name),
                Arc::new(custom::CustomJob(c.clone())),
            );
        }
        q
    }

    /// Registers (or replaces) the handler for `kind`.
    pub fn register(&self, kind: &str, handler: Arc<dyn JobHandler>) {
        self.0
            .handlers
            .write()
            .expect("lock")
            .insert(kind.to_owned(), handler);
    }

    /// The handler for `kind`.
    pub fn handler(&self, kind: &str) -> Option<Arc<dyn JobHandler>> {
        self.0.handlers.read().expect("lock").get(kind).cloned()
    }

    /// All registered kinds with their handlers.
    pub fn kinds(&self) -> Vec<(String, Arc<dyn JobHandler>)> {
        self.0
            .handlers
            .read()
            .expect("lock")
            .iter()
            .map(|(k, h)| (k.clone(), h.clone()))
            .collect()
    }

    /// Starts the workers and the purge timer, once. Jobs left `running` by
    /// a previous process are queued again. Does nothing outside a Tokio
    /// runtime or when already started.
    pub fn start(&self, state: &AppState) {
        if tokio::runtime::Handle::try_current().is_err() || self.0.started.set(()).is_err() {
            return;
        }
        let state = state.clone();
        let q = self.clone();
        tokio::spawn(async move {
            if let Err(e) = sqlx::query(
                "UPDATE jobs SET status = 'queued', message = 'restarted after a server restart', \
                 updated_at = ? WHERE status = 'running'",
            )
            .bind(now())
            .execute(&state.db)
            .await
            {
                tracing::error!(error = %e, "could not requeue interrupted jobs");
            }
            for n in 0..q.0.workers {
                let (q, state) = (q.clone(), state.clone());
                tokio::spawn(async move { q.worker(n, state).await });
            }
            let mut tick = tokio::time::interval(Duration::from_secs(3600));
            loop {
                tick.tick().await;
                if let Err(e) = q.purge(&state).await {
                    tracing::warn!(error = %e, "purging old jobs failed");
                }
            }
        });
    }

    /// Validates and queues a job for `user`.
    ///
    /// # Errors
    ///
    /// `BadRequest` for an unknown or unavailable kind, `Forbidden` for an
    /// admin-only kind, and whatever [`JobHandler::prepare`] returns.
    pub async fn submit(
        &self,
        state: &AppState,
        user: &User,
        kind: &str,
        params: Value,
    ) -> AppResult<Job> {
        let h = self
            .handler(kind)
            .ok_or_else(|| AppError::bad_request(format!("unknown job kind {kind:?}")))?;
        if h.admin_only() && !user.is_admin {
            return Err(AppError::forbidden("only administrators can run this job"));
        }
        h.available(state).map_err(AppError::BadRequest)?;
        let p = h.prepare(state, user, params).await?;
        self.insert(state, Some(user.id), kind, &p).await
    }

    /// Queues a job owned by the system (no user). Parameters are not passed
    /// through [`JobHandler::prepare`]: the caller is trusted.
    ///
    /// # Errors
    ///
    /// Database failures, or an unknown kind.
    pub async fn submit_system(
        &self,
        state: &AppState,
        kind: &str,
        title: &str,
        params: Value,
    ) -> AppResult<Job> {
        if self.handler(kind).is_none() {
            return Err(AppError::bad_request(format!("unknown job kind {kind:?}")));
        }
        let p = Prepared {
            title: title.to_owned(),
            params,
        };
        self.insert(state, None, kind, &p).await
    }

    async fn insert(
        &self,
        state: &AppState,
        user_id: Option<i64>,
        kind: &str,
        p: &Prepared,
    ) -> AppResult<Job> {
        let t = now();
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO jobs (kind, title, user_id, params, status, created_at, updated_at) \
             VALUES (?, ?, ?, ?, 'queued', ?, ?) RETURNING id",
        )
        .bind(kind)
        .bind(&p.title)
        .bind(user_id)
        .bind(p.params.to_string())
        .bind(t)
        .bind(t)
        .fetch_one(&state.db)
        .await?;
        self.0.notify.notify_one();
        self.get(state, id).await
    }

    /// Loads one job.
    ///
    /// # Errors
    ///
    /// `NotFound` if it does not exist.
    pub async fn get(&self, state: &AppState, id: i64) -> AppResult<Job> {
        let row: JobRow = sqlx::query_as(concat!(
            "SELECT ",
            job_columns!(),
            " FROM jobs WHERE id = ?"
        ))
        .bind(id)
        .fetch_one(&state.db)
        .await?;
        Ok(row.into())
    }

    /// Lists jobs, newest first. `user_id = None` lists everyone's jobs
    /// (administrators); `status` filters by status.
    ///
    /// # Errors
    ///
    /// Database failures.
    pub async fn list(
        &self,
        state: &AppState,
        user_id: Option<i64>,
        status: Option<&str>,
        limit: i64,
    ) -> AppResult<Vec<Job>> {
        let rows: Vec<JobRow> = sqlx::query_as(concat!(
            "SELECT ",
            job_columns!(),
            " FROM jobs WHERE (?1 IS NULL OR user_id = ?1) AND (?2 IS NULL OR status = ?2) \
             ORDER BY id DESC LIMIT ?3"
        ))
        .bind(user_id)
        .bind(status)
        .bind(limit.clamp(1, 1000))
        .fetch_all(&state.db)
        .await?;
        Ok(rows.into_iter().map(Job::from).collect())
    }

    /// Cancels a queued or running job. Cancelling a finished job is a no-op.
    ///
    /// # Errors
    ///
    /// `NotFound` if the job does not exist.
    pub async fn cancel(&self, state: &AppState, id: i64) -> AppResult<Job> {
        let t = now();
        sqlx::query(
            "UPDATE jobs SET status = 'cancelled', finished_at = ?, updated_at = ? \
             WHERE id = ? AND status = 'queued'",
        )
        .bind(t)
        .bind(t)
        .bind(id)
        .execute(&state.db)
        .await?;
        sqlx::query(
            "UPDATE jobs SET cancel_requested = 1, updated_at = ? WHERE id = ? AND status = 'running'",
        )
        .bind(t)
        .bind(id)
        .execute(&state.db)
        .await?;
        if let Some(tok) = self.0.running.lock().expect("lock").get(&id) {
            tok.cancel();
        }
        self.get(state, id).await
    }

    /// Queues a failed or cancelled job again.
    ///
    /// # Errors
    ///
    /// `NotFound` if missing, `Conflict` if the job is not failed or cancelled.
    pub async fn retry(&self, state: &AppState, id: i64) -> AppResult<Job> {
        let done = sqlx::query(
            "UPDATE jobs SET status = 'queued', progress = 0, message = NULL, error = NULL, \
             result = NULL, cancel_requested = 0, started_at = NULL, finished_at = NULL, \
             updated_at = ? WHERE id = ? AND status IN ('failed', 'cancelled')",
        )
        .bind(now())
        .bind(id)
        .execute(&state.db)
        .await?
        .rows_affected();
        if done == 0 {
            self.get(state, id).await?;
            return Err(AppError::conflict(
                "only failed or cancelled jobs can be retried",
            ));
        }
        self.0.notify.notify_one();
        self.get(state, id).await
    }

    /// Deletes a finished job from the list.
    ///
    /// # Errors
    ///
    /// `NotFound` if missing, `Conflict` while it is queued or running.
    pub async fn delete(&self, state: &AppState, id: i64) -> AppResult<()> {
        let done = sqlx::query(
            "DELETE FROM jobs WHERE id = ? AND status IN ('succeeded', 'failed', 'cancelled')",
        )
        .bind(id)
        .execute(&state.db)
        .await?
        .rows_affected();
        if done == 0 {
            self.get(state, id).await?;
            return Err(AppError::conflict("cancel the job before deleting it"));
        }
        Ok(())
    }

    async fn purge(&self, state: &AppState) -> AppResult<u64> {
        let cutoff = now() - i64::try_from(self.0.retain_days.saturating_mul(86_400)).unwrap_or(0);
        Ok(sqlx::query(
            "DELETE FROM jobs WHERE status IN ('succeeded', 'failed', 'cancelled') AND finished_at < ?",
        )
        .bind(cutoff)
        .execute(&state.db)
        .await?
        .rows_affected())
    }

    async fn claim(
        &self,
        state: &AppState,
    ) -> AppResult<Option<(i64, String, Option<i64>, String)>> {
        let t = now();
        Ok(sqlx::query_as(
            "UPDATE jobs SET status = 'running', attempts = attempts + 1, started_at = ?, \
             progress = 0, updated_at = ? \
             WHERE id = (SELECT id FROM jobs WHERE status = 'queued' ORDER BY id LIMIT 1) \
             AND status = 'queued' RETURNING id, kind, user_id, params",
        )
        .bind(t)
        .bind(t)
        .fetch_optional(&state.db)
        .await?)
    }

    async fn worker(&self, n: usize, state: AppState) {
        tracing::debug!(worker = n, "job worker started");
        loop {
            let claimed = match self.claim(&state).await {
                Ok(c) => c,
                Err(e) => {
                    tracing::error!(error = %e, "claiming a job failed");
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    continue;
                }
            };
            let Some((id, kind, user_id, params)) = claimed else {
                tokio::select! {
                    () = self.0.notify.notified() => {}
                    () = tokio::time::sleep(Duration::from_secs(10)) => {}
                }
                continue;
            };
            self.run_one(&state, id, &kind, user_id, &params).await;
            // Others may be waiting for a free worker.
            self.0.notify.notify_one();
        }
    }

    async fn run_one(
        &self,
        state: &AppState,
        id: i64,
        kind: &str,
        user_id: Option<i64>,
        params: &str,
    ) {
        let cancel = CancellationToken::new();
        self.0
            .running
            .lock()
            .expect("lock")
            .insert(id, cancel.clone());
        let tmp = state.config.data_dir.join("jobs-tmp").join(id.to_string());
        let _ = tokio::fs::remove_dir_all(&tmp).await;
        let outcome = match tokio::fs::create_dir_all(&tmp).await {
            Err(e) => Err(JobError::Failed(format!("cannot create work folder: {e}"))),
            Ok(()) => match self.handler(kind) {
                None => Err(JobError::Failed(format!("unknown job kind {kind:?}"))),
                Some(h) => {
                    let ctx = JobContext {
                        state: state.clone(),
                        id,
                        user_id,
                        cancel: cancel.clone(),
                        tmp: tmp.clone(),
                        last_flush: Mutex::new(None),
                    };
                    let params: Value = serde_json::from_str(params).unwrap_or(Value::Null);
                    tracing::info!(job = id, kind, "job started");
                    // A cancel already requested (between claim and now) wins.
                    let requested: bool =
                        sqlx::query_scalar("SELECT cancel_requested FROM jobs WHERE id = ?")
                            .bind(id)
                            .fetch_one(&state.db)
                            .await
                            .unwrap_or(false);
                    if requested {
                        cancel.cancel();
                    }
                    let fut = h.run(&ctx, &params);
                    // Panics in a handler fail the job instead of the worker.
                    match futures::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(fut)).await
                    {
                        Ok(r) => r,
                        Err(_) => Err(JobError::failed("the job crashed")),
                    }
                }
            },
        };
        self.0.running.lock().expect("lock").remove(&id);
        let _ = tokio::fs::remove_dir_all(&tmp).await;

        let t = now();
        let r = match &outcome {
            Ok(result) => {
                tracing::info!(job = id, kind, "job succeeded");
                sqlx::query(
                    "UPDATE jobs SET status = 'succeeded', progress = 1, result = ?, error = NULL, \
                     finished_at = ?, updated_at = ? WHERE id = ?",
                )
                .bind(result.to_string())
                .bind(t)
                .bind(t)
                .bind(id)
                .execute(&state.db)
                .await
            }
            Err(JobError::Cancelled) => {
                tracing::info!(job = id, kind, "job cancelled");
                sqlx::query(
                    "UPDATE jobs SET status = 'cancelled', finished_at = ?, updated_at = ? WHERE id = ?",
                )
                .bind(t)
                .bind(t)
                .bind(id)
                .execute(&state.db)
                .await
            }
            Err(JobError::Failed(msg)) => {
                tracing::warn!(job = id, kind, error = %msg, "job failed");
                sqlx::query(
                    "UPDATE jobs SET status = 'failed', error = ?, finished_at = ?, updated_at = ? \
                     WHERE id = ?",
                )
                .bind(msg)
                .bind(t)
                .bind(t)
                .bind(id)
                .execute(&state.db)
                .await
            }
        };
        if let Err(e) = r {
            tracing::error!(job = id, error = %e, "could not save job outcome");
        }
    }
}
