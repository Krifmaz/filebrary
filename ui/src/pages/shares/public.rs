//! `/s/<token>/<path…>`: what a visitor of a share link sees, without an account. A password form when the link
//! has one; for a drop link, a box that takes files (each with progress, Stop and Try again); for a file, its
//! name, size, View and Download; for a folder, its contents with a filter, sorting, folders to open, files to
//! view or download, and Download all as a zip.

use super::api::{self, PublicEntry, PublicShare};
use super::format::{self, Kind, Sort};
use super::icon;
use super::upload::{self, Progress, Sent, Stop};
use crate::route::{on_link, url, Route};
use kit_leptos::modal::{self, ModalOpts};
use leptos::prelude::*;
use wasm_bindgen::JsCast;

/// What the page is showing.
#[derive(Clone, Debug, PartialEq)]
enum Load {
    Loading,
    Failed(String),
    Ready(PublicShare),
}

/// The sentence for a link that can't be opened.
pub fn open_error(e: &kit_leptos::api::ApiError) -> String {
    match e.status {
        404 => "This link doesn't work any more. It may have expired or been revoked.".into(),
        410 => "This link has reached its download limit.".into(),
        429 => "Too many tries. Wait a moment and try again.".into(),
        _ => e.message.clone(),
    }
}

/// The sentence for a password that didn't open the link.
pub fn unlock_error(e: &kit_leptos::api::ApiError) -> String {
    match e.status {
        401 => "That password didn't work.".into(),
        _ => open_error(e),
    }
}

/// The visitor's page for `path` (starting with `/`) of link `token`.
#[component]
pub fn PublicSharePage(token: String, path: String) -> impl IntoView {
    let state = RwSignal::new(Load::Loading);
    let tok = StoredValue::new(token.clone());
    let p = StoredValue::new(path.clone());
    let load = move || {
        let (token, path) = (tok.get_value(), format::rel(&p.get_value()));
        kit_leptos::rt::spawn(async move {
            state.set(match api::open(&token, &path).await {
                Ok(s) => Load::Ready(s),
                Err(e) if e.status == 401 && e.data.get("needs_password").is_some() => Load::Ready(PublicShare {
                    name: String::new(),
                    is_dir: false,
                    size: None,
                    upload_only: false,
                    needs_password: true,
                    expires_at: None,
                    entries: None,
                }),
                Err(e) => Load::Failed(open_error(&e)),
            });
        })
    };
    load();
    let app = crate::app_name();
    if let Some(d) = kit_leptos::rt::document() {
        d.set_title(&format!("Shared with you · {app}"));
    }

    let body = move || {
        match state.get() {
        Load::Loading => view! { <div class="card pub-card"><div class="skeleton" style="height:42px"></div><div class="skeleton mt" style="height:160px"></div></div> }.into_any(),
        Load::Failed(msg) => view! {
            <div class="card pub-card narrow" id="share-error"><div class="pub-center">
                <span class="pub-big" inner_html=icon("link")></span>
                <h1>"This link can't be opened"</h1>
                <p class="muted">{msg}</p>
                <p class="small faint">"Ask the person who sent it for a new one."</p>
            </div></div>
        }
        .into_any(),
        Load::Ready(s) if s.needs_password => view! { <Locked token=tok.get_value() on_open=Callback::new(move |_| load())/> }.into_any(),
        Load::Ready(s) if s.upload_only => view! { <DropBox token=tok.get_value() name=s.name/> }.into_any(),
        Load::Ready(s) if !s.is_dir => view! { <OneFile token=tok.get_value() share=s/> }.into_any(),
        Load::Ready(s) => view! { <Folder token=tok.get_value() path=p.get_value() share=s/> }.into_any(),
    }
    };
    view! {
        <div class="pub-wrap" data-token=token data-path=path>
            <header class="pub-top">
                <div class="brand"><img src=url("/icon.svg") alt=""/><span>{app.clone()}</span></div>
                <span class="small faint">"Shared with you"</span>
            </header>
            {body}
            <footer class="pub-foot">{format!("Shared with {app}")}</footer>
        </div>
    }
}

/// The password form.
#[component]
fn Locked(token: String, on_open: Callback<()>) -> impl IntoView {
    let password = RwSignal::new(String::new());
    let error = RwSignal::new(None::<String>);
    let busy = RwSignal::new(false);
    let token = StoredValue::new(token);
    let submit = move |e: leptos::ev::SubmitEvent| {
        e.prevent_default();
        if busy.get_untracked() {
            return;
        }
        busy.set(true);
        error.set(None);
        kit_leptos::rt::spawn(async move {
            match api::unlock(&token.get_value(), &password.get_untracked()).await {
                Ok(()) => on_open.run(()),
                Err(e) => {
                    busy.set(false);
                    error.set(Some(unlock_error(&e)));
                }
            }
        });
    };
    view! {
        <div class="card pub-card narrow" id="share-locked"><div class="pub-center">
            <span class="pub-big" inner_html=icon("lock")></span>
            <h1>"This link is protected"</h1>
            <p class="muted">"Type the password you were given."</p>
            <form on:submit=submit>
                <label class="field"><span>"Password"</span>
                    <input class="input" type="password" name="password" autocomplete="current-password" required autofocus
                        prop:value=move || password.get() on:input=move |e| password.set(event_target_value(&e))/>
                </label>
                <div class="error-box" role="alert" class:hidden=move || error.get().is_none()>{move || error.get()}</div>
                <button class="btn primary auth-submit" type="submit" disabled=move || busy.get()>
                    {move || if busy.get() { "Opening…" } else { "Open" }}
                </button>
            </form>
        </div></div>
    }
}

/// "Works until 5 Nov 2026, 14:30", or nothing.
fn expiry_line(expires_at: &Option<String>) -> impl IntoView {
    expires_at.as_deref().map(|e| {
        let d = format!("This link works until {}", format::date(e));
        view! { <p class="pub-sub"><span inner_html=icon("clock")></span>{d}</p> }
    })
}

/// Shows file `name` (at `path` of the link) in a dialog: a picture, a video, sound or text. A PDF opens in a
/// new tab, where the browser's own viewer draws it (the server forbids framing its answers).
fn preview(token: &str, path: &str, name: &str) {
    let src = api::download_url(token, path, true);
    let dl = api::download_url(token, path, false);
    let k = format::kind(name);
    if k == Kind::Pdf {
        if let Some(w) = kit_leptos::rt::window() {
            let _ = w.open_with_url_and_target(&src, "_blank");
        }
        return;
    }
    let title = name.to_string();
    let opts = ModalOpts::with_view(name, move || {
        let inner = match k {
            Kind::Image => view! { <img src=src alt=title.clone()/> }.into_any(),
            Kind::Video => view! { <video src=src controls autoplay playsinline></video> }.into_any(),
            Kind::Audio => view! { <audio src=src controls autoplay></audio> }.into_any(),
            _ => {
                let text = RwSignal::new(None::<Result<String, String>>);
                api::fetch_text(&src, move |r| text.set(Some(r)));
                (move || match text.get() {
                    None => view! { <div class="spinner" style="margin:40px"></div> }.into_any(),
                    Some(Ok(t)) => view! { <pre class="pub-text">{t}</pre> }.into_any(),
                    Some(Err(e)) => view! { <div class="error-box" style="margin:20px">{e}</div> }.into_any(),
                })
                .into_any()
            }
        };
        view! { <div class="pub-preview">{inner}</div> }
    })
    .wide(true)
    .footer_view(move || view! { <a class="btn primary" href=dl download=""><span inner_html=icon("download")></span>"Download"</a> });
    modal::open(opts);
}

/// A link to one file.
#[component]
fn OneFile(token: String, share: PublicShare) -> impl IntoView {
    let dl = api::download_url(&token, "", false);
    let can_view = format::kind(&share.name) != Kind::Other;
    let (t, n) = (token.clone(), share.name.clone());
    let size = share.size.map(format::size).unwrap_or_default();
    view! {
        <div class="card pub-card narrow" id="share-file"><div class="pub-center">
            <span class="pub-big" inner_html=icon(format::icon_name(&share.name, false))></span>
            <h1 style="overflow-wrap:anywhere">{share.name.clone()}</h1>
            <p class="muted" style="margin:0">{size}</p>
            {expiry_line(&share.expires_at)}
            <div class="row wrap" style="justify-content:center;margin-top:6px">
                {can_view.then(|| view! { <button class="btn" type="button" on:click=move |_| preview(&t, "", &n)><span inner_html=icon("eye")></span>"View"</button> })}
                <a class="btn primary" href=dl download=""><span inner_html=icon("download")></span>"Download"</a>
            </div>
        </div></div>
    }
}

/// A shared folder (or a collection, or several files), at `path` inside the link.
#[component]
fn Folder(token: String, path: String, share: PublicShare) -> impl IntoView {
    let words = RwSignal::new(String::new());
    let sort = RwSignal::new(Sort::Name);
    let entries = share.entries.clone().unwrap_or_default();
    let total = entries.len();
    let token = StoredValue::new(token);
    let here = format::rel(&path);
    let zip = api::download_url(&token.get_value(), &here, false);
    let top_name = if here.is_empty() { share.name.clone() } else { "Shared folder".into() };
    let crumbs = (!here.is_empty()).then(|| {
        let all = format::crumbs(&top_name, &path);
        let last = all.len() - 1;
        let items = all
            .into_iter()
            .enumerate()
            .map(|(i, (name, p))| {
                if i == last {
                    return view! { <span class="here">{name}</span> }.into_any();
                }
                let r = Route::Public { token: token.get_value(), path: p };
                view! { <a href=url(&r.href()) on:click=on_link(r.clone())>{name}</a><span class="sep">"/"</span> }.into_any()
            })
            .collect_view();
        view! { <nav class="pub-crumbs" aria-label="Folder path">{items}</nav> }
    });
    let summary = {
        let dirs = entries.iter().filter(|e| e.is_dir).count();
        let files = total - dirs;
        let bytes: u64 = entries.iter().map(|e| e.size).sum();
        let mut parts = Vec::new();
        if dirs > 0 {
            parts.push(if dirs == 1 { "1 folder".to_string() } else { format!("{dirs} folders") });
        }
        if files > 0 {
            parts.push(if files == 1 { "1 file".to_string() } else { format!("{files} files") });
            parts.push(format::size(bytes));
        }
        parts.join(" · ")
    };
    let entries = StoredValue::new(entries);
    let rows = move || {
        let w = words.get().trim().to_lowercase();
        let mut list: Vec<PublicEntry> =
            entries.get_value().into_iter().filter(|e| w.is_empty() || e.name.to_lowercase().contains(&w)).collect();
        format::sort_entries(&mut list, sort.get());
        if list.is_empty() {
            let msg = if total == 0 { "This folder is empty." } else { "Nothing here matches." };
            return view! { <div class="empty"><span inner_html=icon("folder")></span><p class="muted">{msg}</p></div> }.into_any();
        }
        view! { <div class="list pub-list" id="share-entries">{list.into_iter().map(|e| row(token.get_value(), e)).collect_view()}</div> }
            .into_any()
    };
    view! {
        <div class="card pub-card" id="share-folder">
            <div class="pub-headline">
                <div class="row">
                    <span class="pub-ficon dir" inner_html=icon("folder")></span>
                    <div style="min-width:0">
                        <h1>{share.name.clone()}</h1>
                        <p class="pub-sub" style="margin-top:2px">{summary}</p>
                    </div>
                </div>
                <a class="btn primary" href=zip download=""><span inner_html=icon("download")></span>"Download all"</a>
            </div>
            {expiry_line(&share.expires_at)}
            {crumbs}
            <Show when=move || { total > 1 }>
                <div class="pub-tools">
                    <input class="input" type="search" placeholder="Filter by name" aria-label="Filter by name"
                        prop:value=move || words.get() on:input=move |e| words.set(event_target_value(&e))/>
                    <select class="input" aria-label="Sort" style="width:auto" on:change=move |e| sort.set(Sort::parse(&event_target_value(&e)))>
                        <option value="name">"Name"</option>
                        <option value="modified">"Newest first"</option>
                        <option value="size">"Largest first"</option>
                    </select>
                </div>
            </Show>
            <div class="mt">{rows}</div>
        </div>
    }
}

/// One entry of a shared folder.
fn row(token: String, e: PublicEntry) -> impl IntoView {
    let meta = match (&e.modified, e.is_dir) {
        (Some(m), true) => format::date(m),
        (Some(m), false) => format!("{} · {}", format::size(e.size), format::date(m)),
        (None, false) => format::size(e.size),
        (None, true) => String::new(),
    };
    let ic = format::icon_name(&e.name, e.is_dir);
    if e.is_dir {
        let r = Route::Public { token, path: format!("/{}", e.path) };
        return view! {
            <div class="list-item">
                <span class="pub-ficon dir" inner_html=icon(ic)></span>
                <div class="pub-name"><a href=url(&r.href()) on:click=on_link(r.clone())>{e.name.clone()}</a><small>{meta}</small></div>
            </div>
        }
        .into_any();
    }
    let dl = api::download_url(&token, &e.path, false);
    let name_view = if format::kind(&e.name) != Kind::Other {
        let (t, p, n) = (token.clone(), e.path.clone(), e.name.clone());
        view! { <button type="button" title="View" on:click=move |_| preview(&t, &p, &n)>{e.name.clone()}</button> }.into_any()
    } else {
        view! { <a href=dl.clone() download="">{e.name.clone()}</a> }.into_any()
    };
    let label = format!("Download {}", e.name);
    view! {
        <div class="list-item">
            <span class="pub-ficon" inner_html=icon(ic)></span>
            <div class="pub-name">{name_view}<small>{meta}</small></div>
            <a class="btn sm icon ghost" href=dl download="" title=label.clone() aria-label=label inner_html=icon("download")></a>
        </div>
    }
    .into_any()
}

// ---------- the drop box ----------

/// Where one upload is.
#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    /// Waiting for the ones before it.
    Waiting,
    /// On its way: bytes sent.
    Sending(f64),
    /// The connection dropped; trying again (bytes the server has).
    Retrying(f64),
    /// Stored, under this name.
    Done(String),
    /// Refused, with why.
    Failed(String),
    /// Stopped.
    Stopped,
}

/// One file the visitor dropped.
#[derive(Clone, Debug, PartialEq)]
pub struct Job {
    id: u32,
    name: String,
    size: f64,
    step: Step,
}

/// The words of a job's state.
pub fn step_words(j: &Job) -> String {
    match &j.step {
        Step::Waiting => "Waiting".into(),
        Step::Sending(sent) if j.size > 0.0 => {
            format!("{}% of {}", ((sent / j.size) * 100.0).floor().min(100.0), format::size(j.size as u64))
        }
        Step::Sending(_) => "Sending".into(),
        Step::Retrying(_) => "Connection lost. Trying again…".into(),
        Step::Done(saved) if *saved != j.name => format!("Sent, saved as {saved}"),
        Step::Done(_) => "Sent".into(),
        Step::Failed(why) => why.clone(),
        Step::Stopped => "Stopped".into(),
    }
}

/// The drop box's state, shared by its parts.
#[derive(Clone, Copy)]
struct Drop {
    token: StoredValue<String>,
    jobs: RwSignal<Vec<Job>>,
    files: StoredValue<Vec<(u32, web_sys::File)>, LocalStorage>,
    current: StoredValue<Option<(u32, Stop)>, LocalStorage>,
    next: StoredValue<u32>,
}

impl Drop {
    fn set_step(self, id: u32, step: Step) {
        self.jobs.update(|v| {
            if let Some(j) = v.iter_mut().find(|j| j.id == id) {
                j.step = step;
            }
        });
    }

    fn add(self, files: Vec<web_sys::File>) {
        for f in files {
            let id = self.next.get_value();
            self.next.set_value(id + 1);
            self.jobs.update(|v| v.push(Job { id, name: f.name(), size: f.size(), step: Step::Waiting }));
            self.files.update_value(|v| v.push((id, f)));
        }
        self.pump();
    }

    /// Starts the next waiting upload, one at a time.
    fn pump(self) {
        if self.current.with_value(Option::is_some) {
            return;
        }
        let Some(id) = self.jobs.with_untracked(|v| v.iter().find(|j| j.step == Step::Waiting).map(|j| j.id)) else {
            set_busy(false);
            return;
        };
        let Some(file) = self.files.with_value(|v| v.iter().find(|(i, _)| *i == id).map(|(_, f)| f.clone())) else { return };
        set_busy(true);
        self.set_step(id, Step::Sending(0.0));
        let stop = Stop::default();
        self.current.set_value(Some((id, stop.clone())));
        let token = self.token.get_value();
        let progress: std::rc::Rc<dyn Fn(Progress)> = std::rc::Rc::new(move |p| match p {
            Progress::Sent(sent) => self.set_step(id, Step::Sending(sent)),
            Progress::Retrying(sent) => self.set_step(id, Step::Retrying(sent)),
        });
        kit_leptos::rt::spawn(async move {
            let sent = upload::send_file(&token, &file, progress, stop).await;
            self.current.set_value(None);
            match sent {
                Sent::Stored(name) => {
                    self.files.update_value(|v| v.retain(|(i, _)| *i != id));
                    self.set_step(id, Step::Done(name));
                }
                Sent::Failed(why) => self.set_step(id, Step::Failed(why)),
                Sent::Cancelled => self.set_step(id, Step::Stopped),
            }
            self.pump();
        });
    }

    fn stop(self, id: u32) {
        let running = self.current.with_value(|c| c.as_ref().filter(|(i, _)| *i == id).map(|(_, s)| s.clone()));
        match running {
            Some(s) => s.stop(),
            None => self.set_step(id, Step::Stopped),
        }
    }

    fn retry(self, id: u32) {
        self.set_step(id, Step::Waiting);
        self.pump();
    }
}

thread_local! {
    static BUSY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static GUARD: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Marks uploads as under way: leaving the page asks first while they are.
fn set_busy(on: bool) {
    BUSY.with(|b| b.set(on));
    if GUARD.with(|g| g.replace(true)) {
        return;
    }
    kit_leptos::rt::on_window("beforeunload", |e| {
        if BUSY.with(std::cell::Cell::get) {
            e.prevent_default();
            let _ = js_sys::Reflect::set(e, &"returnValue".into(), &"".into());
        }
    });
}

/// A drop link: visitors add files to `name` but see nothing in it.
#[component]
fn DropBox(token: String, name: String) -> impl IntoView {
    let d = Drop {
        token: StoredValue::new(token),
        jobs: RwSignal::new(Vec::new()),
        files: StoredValue::new_local(Vec::new()),
        current: StoredValue::new_local(None),
        next: StoredValue::new(1),
    };
    let picked = move |e: leptos::ev::Event| {
        let Some(input) = e.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok()) else { return };
        let files: Vec<web_sys::File> = input.files().map(|l| (0..l.length()).filter_map(|i| l.get(i)).collect()).unwrap_or_default();
        input.set_value("");
        d.add(files);
    };
    let heading = if name.is_empty() { "Send files".to_string() } else { format!("Send files to {name}") };
    let tally = move || {
        let v = d.jobs.get();
        let sent = v.iter().filter(|j| matches!(j.step, Step::Done(_))).count();
        (!v.is_empty()).then(|| format!("{sent} of {} sent", v.len()))
    };
    let items = move || {
        d.jobs
            .get()
            .into_iter()
            .map(|j| {
                let id = j.id;
                let class = match j.step {
                    Step::Done(_) => "done",
                    Step::Failed(_) => "failed",
                    _ => "",
                };
                let words = step_words(&j);
                let pct = match j.step {
                    Step::Sending(s) | Step::Retrying(s) if j.size > 0.0 => Some((s / j.size * 100.0).min(100.0)),
                    Step::Sending(_) | Step::Retrying(_) => Some(0.0),
                    _ => None,
                };
                let action = match j.step {
                    Step::Waiting | Step::Sending(_) | Step::Retrying(_) => Some(view! {
                        <button class="btn sm icon ghost" type="button" title="Stop" aria-label="Stop" on:click=move |_| d.stop(id) inner_html=icon("x")></button>
                    }.into_any()),
                    Step::Failed(_) | Step::Stopped => Some(view! {
                        <button class="btn sm ghost" type="button" on:click=move |_| d.retry(id)><span inner_html=icon("refresh")></span>"Try again"</button>
                    }.into_any()),
                    Step::Done(_) => None,
                };
                view! {
                    <li class=class>
                        <div class="row">
                            <span class="name">{j.name.clone()}</span>
                            <span class="row" style="gap:6px"><span class="state">{words}</span>{action}</span>
                        </div>
                        {pct.map(|p| view! { <div class="drop-bar"><i style=format!("width:{p:.1}%")></i></div> })}
                    </li>
                }
            })
            .collect_view()
    };
    view! {
        <div class="card pub-card narrow" id="share-drop">
            <kit_leptos::dropzone::DropZone input="drop-input" class="drop-box">
                <div class="pub-center">
                    <span class="pub-big" inner_html=icon("upload")></span>
                    <h1>{heading}</h1>
                    <p class="muted">"Drop files here or choose them. You won't see the other files in this folder. If the connection drops, choose the same file again to carry on where it stopped."</p>
                    <input type="file" id="drop-input" multiple hidden on:change=picked/>
                    <label class="btn primary" for="drop-input"><span inner_html=icon("upload")></span>"Choose files"</label>
                </div>
            </kit_leptos::dropzone::DropZone>
            <p class="small faint mt" style="text-align:center">{tally}</p>
            <ul class="drop-list" id="drop-list">{items}</ul>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kit_leptos::api::ApiError;

    fn err(status: u16) -> ApiError {
        ApiError { status, message: "x".into(), data: serde_json::Value::Null }
    }

    #[test]
    fn failures_read_as_sentences() {
        assert!(open_error(&err(404)).contains("doesn't work any more"));
        assert!(open_error(&err(410)).contains("download limit"));
        assert_eq!(open_error(&err(500)), "x");
        assert_eq!(unlock_error(&err(401)), "That password didn't work.");
        assert!(unlock_error(&err(429)).starts_with("Too many tries"));
    }

    #[test]
    fn uploads_say_where_they_are() {
        let j = |step| Job { id: 1, name: "a.txt".into(), size: 2048.0, step };
        assert_eq!(step_words(&j(Step::Waiting)), "Waiting");
        assert_eq!(step_words(&j(Step::Sending(1024.0))), "50% of 2.0 KB");
        assert_eq!(step_words(&j(Step::Done("a.txt".into()))), "Sent");
        assert_eq!(step_words(&j(Step::Done("a (1).txt".into()))), "Sent, saved as a (1).txt");
        assert_eq!(step_words(&j(Step::Failed("Too big".into()))), "Too big");
        assert_eq!(step_words(&j(Step::Retrying(1024.0))), "Connection lost. Trying again…");
    }
}
