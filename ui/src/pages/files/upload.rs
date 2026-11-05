//! Chunked, resumable uploads.
//!
//! Each file goes to an upload session on the server in [`CHUNK`]-sized pieces. The session id is kept in the
//! browser under a key made of the destination and the file's size and change time ([`resume_key`]), so an
//! upload cut off by a closed tab or a lost network carries on from the server's last byte when the same file is
//! picked again. [`CONCURRENCY`] files run at once; a failed chunk is retried with growing pauses
//! ([`retry_delay`]) up to [`MAX_RETRIES`] times, and a 409 asks the server where it stands and carries on from
//! there. A file that already exists waits for the person: Replace or Keep both.
//!
//! The queue lives for the whole page (not per folder), so uploads keep going while the person browses; the
//! panel ([`super::panel`]) shows it. [`uploading`] says whether anything is still under way (for the
//! self-update hold and the "leave the page?" question).

use super::api::{upload_cancel, upload_chunk, upload_start, upload_status};
use super::http::{is_cancelled, Abort};
use super::paths::{basename, join, parent};
use crate::api::Error;
use leptos::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;
use wasm_bindgen::JsCast;

/// Size of one chunk: large enough to be quick, small enough to retry cheaply.
pub const CHUNK: u64 = 8 * 1024 * 1024;
/// Files uploading at once.
pub const CONCURRENCY: usize = 3;
/// Retries of one chunk before the file counts as failed.
pub const MAX_RETRIES: u32 = 5;
/// The first pause before a retry, in milliseconds (doubling each time).
pub const BACKOFF_MS: u64 = 500;

/// Where an upload stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// Waiting for its turn.
    Queued,
    /// Sending.
    Uploading,
    /// Paused by the person.
    Paused,
    /// Finished.
    Done,
    /// Stopped by an error (Retry starts it again).
    Failed,
    /// A file of that name is there: Replace or Keep both.
    Exists,
    /// Cancelled by the person.
    Cancelled,
}

impl Status {
    /// Still to do (counts as under way).
    pub fn active(self) -> bool {
        matches!(self, Status::Queued | Status::Uploading)
    }

    /// Finished one way or another (Clear takes it off the list).
    pub fn finished(self) -> bool {
        matches!(self, Status::Done | Status::Failed | Status::Cancelled)
    }
}

/// One file in the queue, as the panel draws it.
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    /// Its key in the queue.
    pub key: u64,
    /// The source.
    pub source: String,
    /// Where it goes.
    pub path: String,
    /// Its path relative to the folder it was dropped in (`photos/a.jpg`).
    pub name: String,
    /// Bytes.
    pub size: u64,
    /// Bytes the server has (or is getting now).
    pub sent: u64,
    /// Where it stands.
    pub status: Status,
    /// What went wrong.
    pub error: String,
    /// Bytes per second lately (0 while unknown).
    pub rate: f64,
}

/// What the panel's head says: files under way, failed, bytes in all, bytes sent, whole percent.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Summary {
    /// Files queued or sending.
    pub active: usize,
    /// Files that failed or wait for a choice.
    pub failed: usize,
    /// Bytes of every file on the list.
    pub total: u64,
    /// Bytes sent.
    pub sent: u64,
    /// Whole percent.
    pub pct: u32,
    /// Seconds left at the current speed (`None` while unknown).
    pub eta: Option<u64>,
}

/// The panel's head for `items`.
pub fn summary(items: &[Item]) -> Summary {
    let total: u64 = items.iter().filter(|i| i.status != Status::Cancelled).map(|i| i.size).sum();
    let sent: u64 = items.iter().filter(|i| i.status != Status::Cancelled).map(|i| i.sent.min(i.size)).sum();
    let active = items.iter().filter(|i| i.status.active()).count();
    let failed = items.iter().filter(|i| matches!(i.status, Status::Failed | Status::Exists)).count();
    let pct = if total == 0 { 100 } else { ((sent as f64 / total as f64) * 100.0).floor() as u32 };
    let rate: f64 = items.iter().filter(|i| i.status == Status::Uploading).map(|i| i.rate).sum();
    let left: u64 = items.iter().filter(|i| i.status.active()).map(|i| i.size.saturating_sub(i.sent)).sum();
    let eta = (active > 0 && rate > 1.0).then(|| (left as f64 / rate).ceil() as u64);
    Summary { active, failed, total, sent, pct, eta }
}

/// "3 min left", "12 s left".
pub fn eta_words(secs: u64) -> String {
    match secs {
        0..=59 => format!("{secs} s left"),
        60..=3599 => format!("{} min left", secs.div_ceil(60)),
        _ => format!("{} h {} min left", secs / 3600, (secs % 3600) / 60),
    }
}

/// The browser key that remembers "this file going to this place".
pub fn resume_key(source: &str, path: &str, size: u64, last_modified: f64) -> String {
    format!("filebrary.upload:{source}:{path}:{size}:{}", last_modified as i64)
}

/// The pause before retry number `failures` (1, 2, …): 0.5 s, 1 s, 2 s, …
pub fn retry_delay(failures: u32) -> u64 {
    BACKOFF_MS.saturating_mul(1 << failures.saturating_sub(1).min(16))
}

/// What to do after a chunk failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Next {
    /// Try again after a pause (no answer, a timeout, the server busy or down).
    Retry,
    /// The server's offset differs: ask it and carry on from there.
    Resync,
    /// Give up on this file.
    Fail,
}

/// What a failed chunk (status `status`) calls for.
pub fn after_failure(status: u16) -> Next {
    match status {
        409 => Next::Resync,
        408 | 429 => Next::Retry,
        400..=499 => Next::Fail,
        _ => Next::Retry,
    }
}

/// The path for "Keep both": `name (n).ext` next to the original.
pub fn keep_both_path(path: &str, n: u32) -> String {
    let name = basename(path);
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    join(&parent(path), &format!("{stem} ({n}){ext}"))
}

// ---------- the queue ----------

struct Job {
    item: Item,
    file: web_sys::File,
    abort: Option<Abort>,
    session: Option<String>,
    overwrite: bool,
}

#[derive(Default)]
struct Queue {
    jobs: Vec<Job>,
    running: usize,
    next_key: u64,
}

thread_local! {
    static QUEUE: RefCell<Queue> = RefCell::default();
    static ITEMS: ArcRwSignal<Vec<Item>> = ArcRwSignal::new(Vec::new());
    static DONE: ArcRwSignal<(u64, String, String)> = ArcRwSignal::new((0, String::new(), String::new()));
}

/// The queue as the panel draws it.
pub fn items() -> ArcRwSignal<Vec<Item>> {
    ITEMS.with(Clone::clone)
}

/// The last finished upload: `(count, source, path)`; listings reload when it lands in their folder.
pub fn done() -> ArcRwSignal<(u64, String, String)> {
    DONE.with(Clone::clone)
}

/// Whether any upload is still queued or sending.
pub fn uploading() -> bool {
    QUEUE.with(|q| q.borrow().jobs.iter().any(|j| j.item.status.active()))
}

fn publish() {
    let snap: Vec<Item> = QUEUE.with(|q| q.borrow().jobs.iter().map(|j| j.item.clone()).collect());
    ITEMS.with(|s| s.set(snap));
}

fn with_job<R>(key: u64, f: impl FnOnce(&mut Job) -> R) -> Option<R> {
    QUEUE.with(|q| q.borrow_mut().jobs.iter_mut().find(|j| j.item.key == key).map(f))
}

/// A file picked or dropped, with its path relative to the target folder (`photos/2026/a.jpg`).
pub struct Picked {
    /// The file.
    pub file: web_sys::File,
    /// Its relative path.
    pub relative: String,
}

/// Queues `files` for `dir` of `source` and starts them.
pub fn add(source: &str, dir: &str, files: Vec<Picked>) {
    if files.is_empty() {
        return;
    }
    super::panel::show();
    QUEUE.with(|q| {
        let mut q = q.borrow_mut();
        for p in files {
            q.next_key += 1;
            let key = q.next_key;
            let size = p.file.size() as u64;
            q.jobs.push(Job {
                item: Item {
                    key,
                    source: source.to_string(),
                    path: join(dir, &p.relative),
                    name: p.relative,
                    size,
                    sent: 0,
                    status: Status::Queued,
                    error: String::new(),
                    rate: 0.0,
                },
                file: p.file,
                abort: None,
                session: None,
                overwrite: false,
            });
        }
    });
    publish();
    pump();
}

/// Pauses an upload; Resume carries on where it stopped.
pub fn pause(key: u64) {
    let a = with_job(key, |j| {
        if !j.item.status.active() {
            return None;
        }
        j.item.status = Status::Paused;
        j.item.rate = 0.0;
        j.abort.take()
    })
    .flatten();
    if let Some(a) = a {
        a.abort();
    }
    publish();
}

/// Starts a paused or failed upload again.
pub fn resume(key: u64) {
    with_job(key, |j| {
        if matches!(j.item.status, Status::Paused | Status::Failed) {
            j.item.status = Status::Queued;
            j.item.error.clear();
        }
    });
    publish();
    pump();
}

/// Replaces the file that is in the way.
pub fn replace(key: u64) {
    with_job(key, |j| {
        if j.item.status == Status::Exists {
            j.overwrite = true;
            j.item.status = Status::Queued;
            j.item.error.clear();
        }
    });
    publish();
    pump();
}

/// Uploads next to the file that is in the way, as `name (n).ext`.
pub fn keep_both(key: u64) {
    let target = with_job(key, |j| (j.item.source.clone(), j.item.path.clone()));
    let Some((source, path)) = target else { return };
    kit_leptos::rt::spawn(async move {
        let mut n = 1;
        let new_path = loop {
            let p = keep_both_path(&path, n);
            match super::api::stat(&source, &p).await {
                Err(e) if e.status == 404 => break p,
                _ if n >= 99 => break p,
                _ => n += 1,
            }
        };
        with_job(key, |j| {
            if j.item.status == Status::Exists {
                j.item.path = new_path;
                j.item.status = Status::Queued;
                j.item.error.clear();
            }
        });
        publish();
        pump();
    });
}

/// Cancels an upload; the server drops what it holds.
pub fn cancel(key: u64) {
    let found = with_job(key, |j| {
        if j.item.status == Status::Done {
            return None;
        }
        j.item.status = Status::Cancelled;
        j.item.rate = 0.0;
        let id = j.session.clone().or_else(|| storage_get(&job_key(j)));
        storage_del(&job_key(j));
        Some((j.abort.take(), id))
    })
    .flatten();
    if let Some((a, id)) = found {
        if let Some(a) = a {
            a.abort();
        }
        if let Some(id) = id {
            kit_leptos::rt::spawn(async move {
                let _ = upload_cancel(&id).await;
            });
        }
    }
    publish();
}

/// Cancels every upload still to do.
pub fn cancel_all() {
    let keys: Vec<u64> = QUEUE.with(|q| q.borrow().jobs.iter().filter(|j| !j.item.status.finished()).map(|j| j.item.key).collect());
    for k in keys {
        cancel(k);
    }
}

/// Takes finished, failed and cancelled uploads off the list.
pub fn clear_finished() {
    QUEUE.with(|q| q.borrow_mut().jobs.retain(|j| !j.item.status.finished()));
    publish();
}

fn job_key(j: &Job) -> String {
    resume_key(&j.item.source, &j.item.path, j.item.size, j.file.last_modified())
}

fn storage() -> Option<web_sys::Storage> {
    kit_leptos::rt::window().and_then(|w| w.local_storage().ok().flatten())
}

fn storage_get(k: &str) -> Option<String> {
    storage().and_then(|s| s.get_item(k).ok().flatten())
}

fn storage_set(k: &str, v: &str) {
    if let Some(s) = storage() {
        let _ = s.set_item(k, v);
    }
}

fn storage_del(k: &str) {
    if let Some(s) = storage() {
        let _ = s.remove_item(k);
    }
}

fn pump() {
    loop {
        let next = QUEUE.with(|q| {
            let mut q = q.borrow_mut();
            if q.running >= CONCURRENCY {
                return None;
            }
            let j = q.jobs.iter_mut().find(|j| j.item.status == Status::Queued)?;
            j.item.status = Status::Uploading;
            let key = j.item.key;
            q.running += 1;
            Some(key)
        });
        let Some(key) = next else { break };
        publish();
        kit_leptos::rt::spawn(async move {
            run(key).await;
            QUEUE.with(|q| q.borrow_mut().running -= 1);
            publish();
            pump();
        });
    }
    guard_leaving();
}

/// Finds or makes the server's session and answers the offset to carry on from.
async fn open(key: u64, rk: &str, source: &str, path: &str, size: u64, overwrite: bool) -> Result<(String, u64), Error> {
    if let Some(saved) = storage_get(rk) {
        match upload_status(&saved).await {
            Ok(s) => return Ok((saved, s.offset)),
            Err(e) if e.status == 404 => storage_del(rk),
            Err(e) => return Err(e),
        }
    }
    let s = upload_start(source, path, size, overwrite).await?;
    storage_set(rk, &s.id);
    with_job(key, |j| j.session = Some(s.id.clone()));
    Ok((s.id, s.offset))
}

fn still_uploading(key: u64) -> bool {
    with_job(key, |j| j.item.status == Status::Uploading).unwrap_or(false)
}

fn set_progress(key: u64, sent: u64, rate: Option<f64>) {
    with_job(key, |j| {
        j.item.sent = sent;
        if let Some(r) = rate {
            j.item.rate = r;
        }
    });
    publish();
}

fn fail(key: u64, e: &Error) {
    with_job(key, |j| {
        if j.item.status != Status::Uploading {
            return;
        }
        j.item.rate = 0.0;
        if e.status == 409 && j.session.is_none() {
            j.item.status = Status::Exists;
            j.item.error = "A file with this name is already there".into();
        } else {
            j.item.status = Status::Failed;
            j.item.error = e.message.clone();
        }
    });
}

async fn run(key: u64) {
    let Some((source, path, file, overwrite)) =
        with_job(key, |j| (j.item.source.clone(), j.item.path.clone(), j.file.clone(), j.overwrite))
    else {
        return;
    };
    let size = file.size() as u64;
    let rk = resume_key(&source, &path, size, file.last_modified());
    let abort = Abort::default();
    with_job(key, |j| {
        j.abort = Some(abort.clone());
        j.session = None;
    });
    let (id, mut offset) = match open(key, &rk, &source, &path, size, overwrite).await {
        Ok(x) => x,
        Err(e) => return fail(key, &e),
    };
    with_job(key, |j| j.session = Some(id.clone()));
    set_progress(key, offset, None);
    let mut failures = 0;
    while offset < size {
        if !still_uploading(key) {
            return;
        }
        let end = (offset + CHUNK).min(size);
        let Ok(blob) = file.slice_with_f64_and_f64(offset as f64, end as f64) else {
            return fail(key, &Error::new("The file could not be read"));
        };
        let started = kit_leptos::rt::now_ms();
        let base = offset;
        let progress: Rc<dyn Fn(f64)> = Rc::new(move |b: f64| {
            let secs = (kit_leptos::rt::now_ms().saturating_sub(started)) as f64 / 1000.0;
            let rate = (secs > 0.3).then(|| b / secs);
            set_progress(key, base + b as u64, rate);
        });
        match upload_chunk(&id, offset, blob.unchecked_into(), progress, abort.clone()).await {
            Ok(s) => {
                let secs = (kit_leptos::rt::now_ms().saturating_sub(started)).max(1) as f64 / 1000.0;
                set_progress(key, s.offset, Some((s.offset.saturating_sub(base)) as f64 / secs));
                offset = s.offset;
                failures = 0;
            }
            Err(e) if is_cancelled(&e) || abort.is_aborted() => return,
            Err(e) => match after_failure(e.status) {
                Next::Resync => match upload_status(&id).await {
                    Ok(s) => offset = s.offset,
                    Err(e2) => return fail(key, &e2),
                },
                Next::Fail => return fail(key, &e),
                Next::Retry => {
                    failures += 1;
                    if failures > MAX_RETRIES {
                        return fail(key, &e);
                    }
                    kit_leptos::rt::sleep(retry_delay(failures)).await;
                }
            },
        }
    }
    storage_del(&rk);
    let finished = with_job(key, |j| {
        if j.item.status != Status::Uploading {
            return false;
        }
        j.item.status = Status::Done;
        j.item.sent = j.item.size;
        j.item.rate = 0.0;
        j.abort = None;
        true
    })
    .unwrap_or(false);
    if finished {
        DONE.with(|d| d.update(|(n, s, p)| (*n, *s, *p) = (*n + 1, source.clone(), path.clone())));
    }
}

thread_local! {
    static GUARD: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Asks "Leave the page?" while uploads run (the browser's own question; it can't carry our words).
fn guard_leaving() {
    if GUARD.with(|g| g.replace(true)) {
        return;
    }
    kit_leptos::rt::on_window("beforeunload", |e| {
        if uploading() {
            e.prevent_default();
            let _ = js_sys::Reflect::set(e.as_ref(), &"returnValue".into(), &"".into());
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(size: u64, sent: u64, status: Status, rate: f64) -> Item {
        Item { key: 1, source: "s".into(), path: "/a".into(), name: "a".into(), size, sent, status, error: String::new(), rate }
    }

    #[test]
    fn the_head_adds_up() {
        let s = summary(&[item(100, 50, Status::Uploading, 10.0), item(100, 100, Status::Done, 0.0), item(50, 0, Status::Cancelled, 0.0)]);
        assert_eq!((s.active, s.failed, s.total, s.sent, s.pct), (1, 0, 200, 150, 75));
        assert_eq!(s.eta, Some(5));
        assert_eq!(summary(&[]).pct, 100);
        assert_eq!(summary(&[item(10, 0, Status::Exists, 0.0)]).failed, 1);
        assert_eq!(eta_words(5), "5 s left");
        assert_eq!(eta_words(61), "2 min left");
        assert_eq!(eta_words(3700), "1 h 1 min left");
    }

    #[test]
    fn retries_back_off_and_errors_decide() {
        assert_eq!(retry_delay(1), 500);
        assert_eq!(retry_delay(2), 1000);
        assert_eq!(retry_delay(4), 4000);
        assert_eq!(after_failure(409), Next::Resync);
        assert_eq!(after_failure(429), Next::Retry);
        assert_eq!(after_failure(403), Next::Fail);
        assert_eq!(after_failure(0), Next::Retry);
        assert_eq!(after_failure(503), Next::Retry);
    }

    #[test]
    fn keys_and_names() {
        assert_eq!(resume_key("s", "/a.txt", 10, 1234.0), "filebrary.upload:s:/a.txt:10:1234");
        assert_eq!(keep_both_path("/d/a.txt", 1), "/d/a (1).txt");
        assert_eq!(keep_both_path("/README", 2), "/README (2)");
        assert_eq!(keep_both_path("/.env", 1), "/.env (1)");
        assert!(Status::Queued.active() && !Status::Paused.active());
        assert!(Status::Failed.finished() && !Status::Exists.finished());
    }
}
