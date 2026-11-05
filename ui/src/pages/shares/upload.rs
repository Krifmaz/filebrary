//! Drop-link uploads: chunked and resumable, on the server's upload protocol for drop links
//! (`/api/public/shares/<token>/uploads/…`, see `crates/filebrary/src/shares/drop.rs`).
//!
//! A file is sent in [`CHUNK`]-sized pieces to an upload the server keeps. The upload's id is remembered in the
//! browser under a key made of the link, the file's name, size and change time ([`resume_key`]), so a file cut
//! off by a closed tab, a reload or a lost network carries on from the server's last byte when the visitor
//! chooses it again. A chunk that fails for want of a connection (or a busy server) is tried again with growing
//! pauses ([`retry_delay`]) up to [`MAX_RETRIES`] times in a row; a 409 asks the server where it stands and
//! carries on from there. Stopping keeps the upload on the server, so Try again resumes it.

use super::api::{grant, Xhr, ACCESS_HEADER};
use serde_json::{json, Value};
use std::cell::RefCell;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

/// Size of one chunk: large enough to be quick, small enough to retry cheaply (the server takes up to 64 MiB).
pub const CHUNK: u64 = 8 * 1024 * 1024;

/// Failed chunks in a row before the file counts as failed.
pub const MAX_RETRIES: u32 = 8;

/// The first pause before a retry; each further one doubles, up to [`MAX_DELAY_MS`].
const BACKOFF_MS: u64 = 1000;

/// The longest pause between two tries.
const MAX_DELAY_MS: u64 = 30_000;

/// How an upload ended.
#[derive(Clone, Debug, PartialEq)]
pub enum Sent {
    /// Stored, under this name (the server numbers a name that is taken: `a (1).txt`).
    Stored(String),
    /// Refused, with the sentence to show.
    Failed(String),
    /// Stopped by the visitor.
    Cancelled,
}

/// What an upload tells the page while it runs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Progress {
    /// Bytes the server has, plus those of the chunk on its way.
    Sent(f64),
    /// The connection dropped; trying again (bytes the server has).
    Retrying(f64),
}

/// What a failed drop upload says, from its status and answer.
pub fn upload_failure(status: u16, body: &str) -> String {
    let data: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    match status {
        0 => kit_leptos::api::NOT_CONNECTED.to_string(),
        401 => "The password is needed again. Reload the page.".into(),
        404 | 410 => "This link no longer accepts files.".into(),
        413 => "This file is larger than the link allows.".into(),
        429 => "Too many uploads at once. Wait a moment and try again.".into(),
        _ => kit_leptos::api::error_message(&data, ""),
    }
}

/// The browser key that remembers "this file going to this link".
pub fn resume_key(token: &str, name: &str, size: u64, last_modified: f64) -> String {
    format!("filebrary.drop:{token}:{name}:{size}:{}", last_modified as i64)
}

/// The pause before retry number `failures` (1, 2, …): 1 s, 2 s, 4 s, … up to 30 s.
pub fn retry_delay(failures: u32) -> u64 {
    BACKOFF_MS.saturating_mul(1 << failures.saturating_sub(1).min(16)).min(MAX_DELAY_MS)
}

/// What to do after a request failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Next {
    /// Try again after a pause (no answer, a timeout, the server busy or down).
    Retry,
    /// The server's offset differs: carry on from the one it gives.
    Resync,
    /// Give up on this file.
    Fail,
}

/// What a failed request (status `status`, 0 for no answer) calls for.
pub fn after_failure(status: u16) -> Next {
    match status {
        409 => Next::Resync,
        0 | 408 | 429 => Next::Retry,
        400..=499 => Next::Fail,
        _ => Next::Retry,
    }
}

/// The end of the chunk that starts at `offset` of a file of `size` bytes.
pub fn chunk_end(offset: u64, size: u64) -> u64 {
    offset.saturating_add(CHUNK).min(size)
}

/// Stops an upload: the request on its way is aborted and no further one starts.
#[derive(Clone, Default)]
pub struct Stop(Rc<RefCell<StopInner>>);

#[derive(Default)]
struct StopInner {
    stopped: bool,
    xhr: Option<Xhr>,
}

impl Stop {
    /// Stops it.
    pub fn stop(&self) {
        let mut s = self.0.borrow_mut();
        s.stopped = true;
        if let Some(x) = s.xhr.take() {
            x.abort();
        }
    }

    fn stopped(&self) -> bool {
        self.0.borrow().stopped
    }
}

/// An answer: its status (0 when none came) and body.
struct Reply {
    status: u16,
    body: String,
    aborted: bool,
}

impl Reply {
    fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }

    fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or(Value::Null)
    }
}

#[derive(Default)]
struct Shared {
    done: Option<&'static str>,
    waker: Option<Waker>,
}

/// Waits for a request's load, error or abort.
struct Wait(Rc<RefCell<Shared>>);

impl Future for Wait {
    type Output = &'static str;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut s = self.0.borrow_mut();
        match s.done {
            Some(how) => Poll::Ready(how),
            None => {
                s.waker = Some(cx.waker().clone());
                Poll::Pending
            }
        }
    }
}

/// What a request carries.
enum Body<'a> {
    None,
    Json(String),
    Chunk(&'a web_sys::Blob),
}

/// Sends one request of link `token` and waits for its answer.
async fn request(
    token: &str,
    method: &str,
    url: &str,
    headers: &[(&str, String)],
    body: Body<'_>,
    progress: Option<Rc<dyn Fn(f64)>>,
    stop: &Stop,
) -> Reply {
    if stop.stopped() {
        return Reply { status: 0, body: String::new(), aborted: true };
    }
    let x = Xhr::new();
    x.open(method, url);
    x.set_request_header("X-Requested-With", crate::project());
    let g = grant(token);
    if !g.is_empty() {
        x.set_request_header(ACCESS_HEADER, &g);
    }
    for (k, v) in headers {
        x.set_request_header(k, v);
    }
    let state: Rc<RefCell<Shared>> = Rc::default();
    let mut keep: Vec<Closure<dyn FnMut(JsValue)>> = Vec::new();
    for how in ["load", "error", "abort"] {
        let s = state.clone();
        let f = Closure::<dyn FnMut(JsValue)>::new(move |_: JsValue| {
            let mut s = s.borrow_mut();
            s.done.get_or_insert(how);
            if let Some(w) = s.waker.take() {
                w.wake();
            }
        });
        match how {
            "load" => x.set_onload(f.as_ref().unchecked_ref()),
            "error" => x.set_onerror(f.as_ref().unchecked_ref()),
            _ => x.set_onabort(f.as_ref().unchecked_ref()),
        }
        keep.push(f);
    }
    if let Some(p) = progress {
        let f = Closure::<dyn FnMut(JsValue)>::new(move |e: JsValue| {
            p(js_sys::Reflect::get(&e, &"loaded".into()).ok().and_then(|v| v.as_f64()).unwrap_or(0.0));
        });
        x.upload().set_onprogress(f.as_ref().unchecked_ref());
        keep.push(f);
    }
    stop.0.borrow_mut().xhr = Some(x.clone());
    match body {
        Body::None => x.send_nothing(),
        Body::Json(s) => {
            x.set_request_header("Content-Type", "application/json");
            x.send_text(&s);
        }
        Body::Chunk(b) => {
            x.set_request_header("Content-Type", "application/octet-stream");
            x.send(b);
        }
    }
    let how = Wait(state).await;
    stop.0.borrow_mut().xhr = None;
    let reply = match how {
        "load" => Reply { status: x.status(), body: x.response_text(), aborted: false },
        "abort" => Reply { status: 0, body: String::new(), aborted: true },
        _ => Reply { status: 0, body: String::new(), aborted: false },
    };
    drop(keep);
    reply
}

fn storage() -> Option<web_sys::Storage> {
    kit_leptos::rt::window().and_then(|w| w.local_storage().ok().flatten())
}

fn remembered(k: &str) -> Option<String> {
    storage().and_then(|s| s.get_item(k).ok().flatten())
}

fn remember(k: &str, v: &str) {
    if let Some(s) = storage() {
        let _ = s.set_item(k, v);
    }
}

fn forget(k: &str) {
    if let Some(s) = storage() {
        let _ = s.remove_item(k);
    }
}

fn uploads_url(token: &str, rest: &str) -> String {
    format!("/api/public/shares/{}/uploads/{rest}", crate::api::encode(token))
}

fn stored_name(v: &Value, fallback: &str) -> String {
    v.get("name").and_then(Value::as_str).map(String::from).unwrap_or_else(|| fallback.to_string())
}

fn offset_of(v: &Value) -> Option<u64> {
    v.get("offset").and_then(Value::as_u64)
}

/// Where an upload stands before its next chunk.
enum Open {
    /// Carry on at this offset of this upload.
    At(String, u64),
    /// Already stored (an empty file), under this name.
    Done(String),
}

/// Finds the upload this file had on the server (when it is still there) or starts one.
async fn open(token: &str, key: &str, name: &str, size: u64, stop: &Stop) -> Result<Open, Reply> {
    if let Some(id) = remembered(key) {
        let r = request(token, "GET", &uploads_url(token, &crate::api::encode(&id)), &[], Body::None, None, stop).await;
        if r.ok() {
            if let Some(at) = offset_of(&r.json()) {
                return Ok(Open::At(id, at));
            }
        } else if r.status == 404 {
            forget(key);
        } else {
            return Err(r);
        }
    }
    let body = json!({ "path": name, "size": size }).to_string();
    let r = request(token, "POST", &uploads_url(token, "start"), &[], Body::Json(body), None, stop).await;
    if !r.ok() {
        return Err(r);
    }
    let v = r.json();
    if v.get("done").and_then(Value::as_bool) == Some(true) {
        return Ok(Open::Done(stored_name(&v, name)));
    }
    let id = v.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
    remember(key, &id);
    Ok(Open::At(id, offset_of(&v).unwrap_or(0)))
}

/// Sends `file` into drop link `token`, telling `progress` how far it got. Stopping it with `stop` ends it as
/// [`Sent::Cancelled`] and keeps what the server has, so sending the same file again carries on.
pub async fn send_file(token: &str, file: &web_sys::File, progress: Rc<dyn Fn(Progress)>, stop: Stop) -> Sent {
    let name = file.name();
    let size = file.size() as u64;
    let key = resume_key(token, &name, size, file.last_modified());
    let mut failures = 0u32;
    // Opening (or finding) the upload, retried like a chunk when the connection is gone.
    let (id, mut offset) = loop {
        match open(token, &key, &name, size, &stop).await {
            Ok(Open::Done(stored)) => return Sent::Stored(stored),
            Ok(Open::At(id, at)) => break (id, at),
            Err(r) if r.aborted => return Sent::Cancelled,
            Err(r) => match after_failure(r.status) {
                Next::Retry if failures < MAX_RETRIES => {
                    failures += 1;
                    progress(Progress::Retrying(0.0));
                    kit_leptos::rt::sleep(retry_delay(failures)).await;
                }
                _ => return Sent::Failed(upload_failure(r.status, &r.body)),
            },
        }
    };
    let url = uploads_url(token, &crate::api::encode(&id));
    loop {
        if stop.stopped() {
            return Sent::Cancelled;
        }
        progress(Progress::Sent(offset as f64));
        let end = chunk_end(offset, size);
        let Ok(chunk) = file.slice_with_f64_and_f64(offset as f64, end as f64) else {
            return Sent::Failed("The browser could not read this file.".into());
        };
        let at = offset as f64;
        let p = progress.clone();
        let on_sent: Rc<dyn Fn(f64)> = Rc::new(move |loaded| p(Progress::Sent(at + loaded)));
        let r = request(token, "PATCH", &url, &[("Upload-Offset", offset.to_string())], Body::Chunk(&chunk), Some(on_sent), &stop).await;
        if r.aborted {
            return Sent::Cancelled;
        }
        if r.ok() {
            failures = 0;
            let v = r.json();
            if v.get("done").and_then(Value::as_bool) == Some(true) {
                forget(&key);
                return Sent::Stored(stored_name(&v, &name));
            }
            offset = offset_of(&v).unwrap_or(end);
            continue;
        }
        match after_failure(r.status) {
            Next::Resync => {
                // The server says where it stands (also while another tab sends the same chunk).
                failures += 1;
                if failures > MAX_RETRIES {
                    return Sent::Failed(upload_failure(r.status, &r.body));
                }
                if let Some(at) = offset_of(&r.json()) {
                    offset = at;
                }
                if failures > 1 {
                    kit_leptos::rt::sleep(retry_delay(failures - 1)).await;
                }
            }
            Next::Retry if failures < MAX_RETRIES => {
                failures += 1;
                progress(Progress::Retrying(offset as f64));
                kit_leptos::rt::sleep(retry_delay(failures)).await;
                // Ask where the server got to: part of the chunk may have been stored before the drop.
                let s = request(token, "GET", &url, &[], Body::None, None, &stop).await;
                if s.aborted {
                    return Sent::Cancelled;
                }
                if s.ok() {
                    if let Some(at) = offset_of(&s.json()) {
                        offset = at;
                    }
                } else if s.status == 404 {
                    forget(&key);
                    return Sent::Failed(upload_failure(404, &s.body));
                }
            }
            _ => {
                if r.status == 404 || r.status == 410 || r.status == 413 {
                    forget(&key);
                }
                return Sent::Failed(upload_failure(r.status, &r.body));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failures_read_as_sentences() {
        assert_eq!(upload_failure(413, ""), "This file is larger than the link allows.");
        assert_eq!(
            upload_failure(409, r#"{"error":"conflict","message":"a file with this name was just uploaded"}"#),
            "a file with this name was just uploaded"
        );
        assert_eq!(upload_failure(0, ""), kit_leptos::api::NOT_CONNECTED);
    }

    #[test]
    fn retries_wait_longer_each_time() {
        assert_eq!(retry_delay(1), 1000);
        assert_eq!(retry_delay(2), 2000);
        assert_eq!(retry_delay(4), 8000);
        assert_eq!(retry_delay(9), MAX_DELAY_MS);
        assert_eq!(retry_delay(99), MAX_DELAY_MS);
    }

    #[test]
    fn failures_decide_what_comes_next() {
        assert_eq!(after_failure(0), Next::Retry);
        assert_eq!(after_failure(503), Next::Retry);
        assert_eq!(after_failure(429), Next::Retry);
        assert_eq!(after_failure(409), Next::Resync);
        assert_eq!(after_failure(413), Next::Fail);
        assert_eq!(after_failure(404), Next::Fail);
    }

    #[test]
    fn chunks_cover_the_file() {
        assert_eq!(chunk_end(0, 10), 10);
        assert_eq!(chunk_end(0, 3 * CHUNK), CHUNK);
        assert_eq!(chunk_end(2 * CHUNK, 2 * CHUNK + 5), 2 * CHUNK + 5);
    }

    #[test]
    fn resume_keys_tell_files_apart() {
        let a = resume_key("tok", "a.txt", 10, 1_700_000_000_000.0);
        assert_eq!(a, "filebrary.drop:tok:a.txt:10:1700000000000");
        assert_ne!(a, resume_key("tok", "a.txt", 11, 1_700_000_000_000.0));
        assert_ne!(a, resume_key("other", "a.txt", 10, 1_700_000_000_000.0));
    }
}
