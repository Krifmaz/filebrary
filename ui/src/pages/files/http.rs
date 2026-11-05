//! Calls the kit's JSON backend can't make: a raw body (an upload chunk, a text file) with headers of their own
//! (`Upload-Offset`, `If-Match`), upload progress and a way to cancel. They go through `XMLHttpRequest`, the
//! only browser API that reports upload progress.
//!
//! Errors come back as the kit's [`ApiError`] with the server's sentence, so pages handle them like any other
//! call. The CSRF token for these calls is read once from `GET /api/me` and asked again after a refusal.

use kit_leptos::api::{ApiError, NOT_CONNECTED};
use serde_json::Value;
use std::cell::RefCell;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_name = XMLHttpRequest, extends = web_sys::EventTarget)]
    type Xhr;
    #[wasm_bindgen(constructor, js_class = "XMLHttpRequest")]
    fn new() -> Xhr;
    #[wasm_bindgen(method, js_class = "XMLHttpRequest")]
    fn open(this: &Xhr, method: &str, url: &str);
    #[wasm_bindgen(method, js_class = "XMLHttpRequest", js_name = setRequestHeader)]
    fn set_request_header(this: &Xhr, name: &str, value: &str);
    #[wasm_bindgen(method, js_class = "XMLHttpRequest")]
    fn send(this: &Xhr, body: &JsValue);
    #[wasm_bindgen(method, js_class = "XMLHttpRequest")]
    fn abort(this: &Xhr);
    #[wasm_bindgen(method, getter, js_class = "XMLHttpRequest")]
    fn status(this: &Xhr) -> u16;
    #[wasm_bindgen(method, getter, js_class = "XMLHttpRequest", js_name = responseText)]
    fn response_text(this: &Xhr) -> String;
    #[wasm_bindgen(method, getter, js_class = "XMLHttpRequest")]
    fn upload(this: &Xhr) -> web_sys::EventTarget;
}

/// A way to stop a call under way (pause or cancel an upload). Cloning shares it.
#[derive(Clone, Default)]
pub struct Abort(Rc<RefCell<AbortInner>>);

#[derive(Default)]
struct AbortInner {
    aborted: bool,
    xhr: Option<Xhr>,
}

impl Abort {
    /// Stops the call under way and any later call made with this handle.
    pub fn abort(&self) {
        let mut i = self.0.borrow_mut();
        i.aborted = true;
        if let Some(x) = i.xhr.take() {
            x.abort();
        }
    }

    /// Whether [`Abort::abort`] was called.
    pub fn is_aborted(&self) -> bool {
        self.0.borrow().aborted
    }
}

/// What a call answered.
#[derive(Clone, Debug)]
pub struct Answer {
    /// The HTTP status.
    pub status: u16,
    /// The body as text.
    pub text: String,
}

impl Answer {
    /// The body as JSON (`null` when it is not JSON).
    pub fn json(&self) -> Value {
        serde_json::from_str(&self.text).unwrap_or(Value::Null)
    }
}

/// One call.
pub struct Request {
    method: &'static str,
    url: String,
    headers: Vec<(String, String)>,
    body: JsValue,
    progress: Option<Rc<dyn Fn(f64)>>,
    abort: Option<Abort>,
}

impl Request {
    /// A call of `method` to `url` (a full `/api/…` address).
    pub fn new(method: &'static str, url: impl Into<String>) -> Self {
        Request { method, url: url.into(), headers: Vec::new(), body: JsValue::UNDEFINED, progress: None, abort: None }
    }

    /// With this header.
    pub fn header(mut self, k: &str, v: &str) -> Self {
        self.headers.push((k.into(), v.into()));
        self
    }

    /// With this body (a `Blob`, a string, …).
    pub fn body(mut self, b: JsValue) -> Self {
        self.body = b;
        self
    }

    /// Reports the bytes sent so far while the body uploads.
    pub fn progress(mut self, f: Rc<dyn Fn(f64)>) -> Self {
        self.progress = Some(f);
        self
    }

    /// Can be stopped with `a`.
    pub fn abort(mut self, a: Abort) -> Self {
        self.abort = Some(a);
        self
    }

    /// Sends it with the CSRF token; a refusal for a stale token asks for a new one and tries once more.
    pub async fn send(self) -> Result<Answer, ApiError> {
        let safe = matches!(self.method, "GET" | "HEAD");
        if safe {
            return self.send_once(None).await;
        }
        let token = csrf(false).await;
        let headers = self.headers.clone();
        let body = self.body.clone();
        match self.send_once(token.as_deref()).await {
            Err(e) if e.status == 403 && crate::api::code(&e).contains("csrf") => {
                let token = csrf(true).await;
                Request { headers, body, ..Request::new(self.method, self.url.clone()) }
                    .with_progress(self.progress.clone())
                    .with_abort(self.abort.clone())
                    .send_once(token.as_deref())
                    .await
            }
            other => other,
        }
    }

    fn with_progress(mut self, p: Option<Rc<dyn Fn(f64)>>) -> Self {
        self.progress = p;
        self
    }

    fn with_abort(mut self, a: Option<Abort>) -> Self {
        self.abort = a;
        self
    }

    async fn send_once(&self, token: Option<&str>) -> Result<Answer, ApiError> {
        if self.abort.as_ref().is_some_and(Abort::is_aborted) {
            return Err(cancelled());
        }
        let xhr = Xhr::new();
        xhr.open(self.method, &self.url);
        xhr.set_request_header("X-Requested-With", crate::project());
        if let Some(t) = token {
            xhr.set_request_header("X-CSRF-Token", t);
        }
        for (k, v) in &self.headers {
            xhr.set_request_header(k, v);
        }
        let state: Rc<RefCell<Shared>> = Rc::default();
        let mut keep = Vec::new();
        for kind in ["load", "error", "abort", "timeout"] {
            let s = state.clone();
            keep.push(kit_leptos::rt::listen(&xhr, kind, move |_| {
                let mut s = s.borrow_mut();
                s.done = Some(kind);
                if let Some(w) = s.waker.take() {
                    w.wake();
                }
            }));
        }
        if let Some(p) = self.progress.clone() {
            keep.push(kit_leptos::rt::listen(&xhr.upload(), "progress", move |e| {
                let loaded = js_sys::Reflect::get(e.as_ref(), &"loaded".into()).ok().and_then(|v| v.as_f64()).unwrap_or(0.0);
                p(loaded);
            }));
        }
        if let Some(a) = &self.abort {
            a.0.borrow_mut().xhr = Some(xhr.clone());
        }
        xhr.send(&self.body);
        let how = Wait(state).await;
        drop(keep);
        if let Some(a) = &self.abort {
            a.0.borrow_mut().xhr = None;
        }
        match how {
            "load" => {
                let answer = Answer { status: xhr.status(), text: xhr.response_text() };
                if (200..300).contains(&answer.status) {
                    Ok(answer)
                } else {
                    if answer.status == 401 {
                        kit_leptos::rt::dispatch(&format!("{}:unauthorized", crate::project()));
                    }
                    Err(error_of(answer.status, &answer.text))
                }
            }
            "abort" => Err(cancelled()),
            _ => Err(ApiError::new(NOT_CONNECTED)),
        }
    }
}

impl Clone for Xhr {
    fn clone(&self) -> Self {
        JsValue::from(self).unchecked_into()
    }
}

/// A cancelled call (paused or cancelled by the person).
pub fn cancelled() -> ApiError {
    let mut e = ApiError::new(kit_leptos::api::CANCELLED);
    e.data = serde_json::json!({ "error": "cancelled" });
    e
}

/// Whether `e` is a call the person stopped.
pub fn is_cancelled(e: &ApiError) -> bool {
    e.status == 0 && crate::api::code(e) == "cancelled"
}

/// The error for a failed answer: the server's `message` (or its `error` code), else the status.
pub fn error_of(status: u16, body: &str) -> ApiError {
    let data: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let message = data
        .get("message")
        .or_else(|| data.get("detail"))
        .or_else(|| data.get("error"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| format!("The server answered {status}"));
    ApiError { status, message, data }
}

#[derive(Default)]
struct Shared {
    done: Option<&'static str>,
    waker: Option<Waker>,
}

/// Waits for the call's `load`, `error`, `abort` or `timeout`.
struct Wait(Rc<RefCell<Shared>>);

impl Future for Wait {
    type Output = &'static str;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<&'static str> {
        let mut s = self.0.borrow_mut();
        match s.done {
            Some(d) => Poll::Ready(d),
            None => {
                s.waker = Some(cx.waker().clone());
                Poll::Pending
            }
        }
    }
}

/// A promise's outcome once settled, and who waits for it.
type PromiseState = (Option<Result<JsValue, JsValue>>, Option<Waker>);

/// Waits for a JavaScript promise: its value, or what it was rejected with.
pub async fn promise(p: js_sys::Promise) -> Result<JsValue, JsValue> {
    let state: Rc<RefCell<PromiseState>> = Rc::default();
    let (ok, err) = (state.clone(), state.clone());
    let on_ok = Closure::once(move |v: JsValue| {
        let mut s = ok.borrow_mut();
        s.0 = Some(Ok(v));
        if let Some(w) = s.1.take() {
            w.wake();
        }
    });
    let on_err = Closure::once(move |v: JsValue| {
        let mut s = err.borrow_mut();
        s.0 = Some(Err(v));
        if let Some(w) = s.1.take() {
            w.wake();
        }
    });
    let _ = p.then2(&on_ok, &on_err);
    let out = PromiseWait(state).await;
    drop((on_ok, on_err));
    out
}

struct PromiseWait(Rc<RefCell<PromiseState>>);

impl Future for PromiseWait {
    type Output = Result<JsValue, JsValue>;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut s = self.0.borrow_mut();
        match s.0.take() {
            Some(r) => Poll::Ready(r),
            None => {
                s.1 = Some(cx.waker().clone());
                Poll::Pending
            }
        }
    }
}

thread_local! {
    static CSRF: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// The session's CSRF token (`fresh` asks the server again).
async fn csrf(fresh: bool) -> Option<String> {
    if !fresh {
        if let Some(t) = CSRF.with(|c| c.borrow().clone()) {
            return Some(t);
        }
    }
    let me = kit_leptos::api::get("/me").await.ok()?;
    let t = me.get("csrf_token").and_then(Value::as_str)?.to_string();
    CSRF.with(|c| *c.borrow_mut() = Some(t.clone()));
    Some(t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_carry_the_servers_sentence() {
        let e = error_of(409, r#"{"error":"conflict","message":"Offset mismatch"}"#);
        assert_eq!((e.status, e.message.as_str(), crate::api::code(&e)), (409, "Offset mismatch", "conflict"));
        assert_eq!(error_of(500, "oops").message, "The server answered 500");
        assert_eq!(error_of(404, r#"{"error":"not_found"}"#).message, "not_found");
        assert!(is_cancelled(&cancelled()));
        assert!(!is_cancelled(&ApiError::new("x")));
    }
}
