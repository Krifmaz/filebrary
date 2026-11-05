//! The viewer: a full-screen layer over the browser for the file being looked at. Pictures show the server's
//! large preview (RAW and HEIC included), video and audio play (with Cast where the browser offers it), PDFs open
//! in the browser's own reader, office files show their first page, text and code open in the editor, and
//! anything else gets a card with Download.
//!
//! ← and → (or the side buttons, or a swipe) step through the folder's files; Escape closes.

use super::api;
use super::editor::Editor;
use super::icons::icon;
use super::kinds::{file_icon, viewer_for, Viewer};
use super::ops;
use super::paths::{format_date, format_size};
use super::state::Ctx;
use crate::api::{download_url, preview_url, Entry};
use leptos::prelude::*;
use wasm_bindgen::JsCast;

/// The file `step` places from `current` among the folder's files (folders are skipped), wrapping around.
pub fn neighbour(files: &[Entry], current: &str, step: i64) -> Option<Entry> {
    let files: Vec<&Entry> = files.iter().filter(|e| !e.is_dir).collect();
    let n = files.len() as i64;
    if n < 2 {
        return None;
    }
    let i = files.iter().position(|e| e.path == current)? as i64;
    Some(files[((i + step).rem_euclid(n)) as usize].clone())
}

/// Whether the browser can hand media to a cast device (the Remote Playback API).
fn can_cast() -> bool {
    kit_leptos::rt::window()
        .and_then(|w| js_sys::Reflect::get(&w, &"HTMLMediaElement".into()).ok())
        .and_then(|c| js_sys::Reflect::get(&c, &"prototype".into()).ok())
        .is_some_and(|p| js_sys::Reflect::has(&p, &"remote".into()).unwrap_or(false))
}

/// Opens the device picker for the media element on screen.
fn cast() {
    let Some(m) = kit_leptos::rt::qs(".viewer video, .viewer audio") else { return };
    let remote = js_sys::Reflect::get(&m, &"remote".into()).unwrap_or_default();
    if let Ok(prompt) = js_sys::Reflect::get(&remote, &"prompt".into()).and_then(|f| f.dyn_into::<js_sys::Function>()) {
        if let Ok(p) = prompt.call0(&remote).and_then(|p| p.dyn_into::<js_sys::Promise>()) {
            kit_leptos::rt::spawn(async move {
                if let Err(e) = super::http::promise(p).await {
                    let msg = js_sys::Reflect::get(&e, &"message".into()).ok().and_then(|m| m.as_string()).unwrap_or_default();
                    if !msg.is_empty() && !msg.contains("cancel") {
                        kit_leptos::toasts::toast(&format!("Casting didn't start: {msg}"), "err");
                    }
                }
            });
        }
    }
}

/// A video or audio player. It plays from a short-lived cookie-free address, so a cast device can take it over;
/// when that can't be had, from the normal download address.
fn media(source: String, e: Entry, video: bool) -> impl IntoView {
    let src = RwSignal::new(None::<String>);
    let fallback = download_url(&source, &e.path, true);
    let (s, p) = (source.clone(), e.path.clone());
    let fb = fallback.clone();
    kit_leptos::rt::spawn(async move {
        let u = api::cast_url(&s, &p).await.unwrap_or(fb);
        let _ = src.try_set(Some(u));
    });
    let stream = format!("/api{}/stream{}", crate::api::source_path(&source), crate::api::query(&[("path", &e.path)]));
    let tried_stream = StoredValue::new(false);
    // a file the browser can't play natively: the server's transcoded stream
    let on_error = move |_| {
        if video && !tried_stream.get_value() {
            tried_stream.set_value(true);
            src.set(Some(stream.clone()));
        }
    };
    move || {
        src.get().map(|u| {
            if video {
                view! { <video class="viewer-media" src=u controls autoplay playsinline preload="metadata" on:error=on_error.clone()></video> }.into_any()
            } else {
                view! { <div class="viewer-audio"><span class="viewer-big-icon" inner_html=icon("file-audio")></span><audio src=u controls autoplay preload="metadata"></audio></div> }.into_any()
            }
        })
    }
}

/// A picture: the large preview when the server makes one, else the file itself.
fn picture(ctx: Ctx, source: String, e: Entry, kind: &'static str) -> impl IntoView {
    let caps = ctx.caps.get_untracked().unwrap_or_default();
    let large = caps.enabled && caps.kinds.get(kind).copied().unwrap_or(false);
    let original = download_url(&source, &e.path, true);
    let src = RwSignal::new(if large { preview_url(&source, &e.path, "large", &e.modified) } else { original.clone() });
    let failed = RwSignal::new(false);
    let on_error = move |_| {
        if src.get_untracked() != original {
            src.set(original.clone());
        } else {
            failed.set(true);
        }
    };
    let name = e.name.clone();
    move || {
        if failed.get() {
            other(&source, &e, "This picture can't be shown in the browser.").into_any()
        } else {
            view! { <img class="viewer-img" src=move || src.get() alt=name.clone() on:error=on_error.clone() decoding="async"/> }.into_any()
        }
    }
}

/// The card for a file nothing here can show.
fn other(source: &str, e: &Entry, why: &str) -> impl IntoView {
    let url = download_url(source, &e.path, false);
    view! {
        <div class="viewer-card card">
            <span class="viewer-big-icon" inner_html=icon(file_icon(&e.name, false))></span>
            <h3 class="ellipsis">{e.name.clone()}</h3>
            <p class="small muted">{format!("{} · {}", format_size(e.size), format_date(&e.modified))}</p>
            <p class="small muted">{why.to_string()}</p>
            <button type="button" class="btn primary mt" on:click=move |_| ops::download_link(&url)><span inner_html=icon("download")></span>"Download"</button>
        </div>
    }
}

/// The viewer for `ctx.viewing`.
#[component]
pub fn Preview(ctx: Ctx) -> impl IntoView {
    let dirty = RwSignal::new(false);
    let close = move || {
        if !dirty.get_untracked() {
            ctx.viewing.set(None);
            return;
        }
        kit_leptos::rt::spawn(async move {
            if kit_leptos::modal::confirm_dialog("Close without saving?", "Your changes to this file are lost.", "Close", true).await {
                dirty.set(false);
                ctx.viewing.set(None);
            }
        });
    };
    let step = move |n: i64| {
        let Some(cur) = ctx.viewing.get_untracked() else { return };
        if dirty.get_untracked() {
            return;
        }
        if let Some(next) = neighbour(&ctx.visible.get_untracked(), &cur.path, n) {
            ctx.select(std::collections::BTreeSet::from([next.path.clone()]), Some(next.path.clone()));
            ctx.viewing.set(Some(next));
        }
    };
    // keys: Escape closes, arrows step (not while typing)
    let keys = StoredValue::new_local(kit_leptos::rt::document().map(|d| {
        kit_leptos::rt::listen(&d, "keydown", move |ev| {
            if ctx.viewing.get_untracked().is_none() || kit_leptos::rt::qs(".modal-bg").is_some() {
                return;
            }
            let Some(k) = ev.dyn_ref::<web_sys::KeyboardEvent>() else { return };
            let typing =
                kit_leptos::rt::target_el(ev).is_some_and(|t| t.closest("input, textarea, select, video, audio").ok().flatten().is_some());
            match k.key().as_str() {
                "Escape" => {
                    k.prevent_default();
                    close();
                }
                "ArrowRight" if !typing => step(1),
                "ArrowLeft" if !typing => step(-1),
                _ => {}
            }
        })
    }));
    let _ = keys;
    // swipe left and right on touch screens
    let start_x = StoredValue::new(None::<(f64, f64)>);
    let on_down = move |e: leptos::ev::PointerEvent| {
        if e.pointer_type() == "touch" {
            start_x.set_value(Some((e.client_x() as f64, e.client_y() as f64)));
        }
    };
    let on_up = move |e: leptos::ev::PointerEvent| {
        if let Some((x, y)) = start_x.get_value() {
            start_x.set_value(None);
            let (dx, dy) = (e.client_x() as f64 - x, e.client_y() as f64 - y);
            if dx.abs() > 60.0 && dx.abs() > dy.abs() * 1.5 {
                step(if dx < 0.0 { 1 } else { -1 });
            }
        }
    };
    move || {
        ctx.viewing.get().map(|e| {
            let source = ctx.source.get_untracked();
            let perms = ctx.perms();
            let kind = viewer_for(&e).unwrap_or(Viewer::Other);
            let many = neighbour(&ctx.visible.get_untracked(), &e.path, 1).is_some();
            let body = if !perms.download {
                other(&source, &e, "You may see this file but not open or download it.").into_any()
            } else {
                match kind {
                    Viewer::Image => picture(ctx, source.clone(), e.clone(), super::kinds::preview_kind(&e.name, false).unwrap_or("image")).into_any(),
                    Viewer::Video => media(source.clone(), e.clone(), true).into_any(),
                    Viewer::Audio => media(source.clone(), e.clone(), false).into_any(),
                    Viewer::Pdf => view! { <iframe class="viewer-frame" src=download_url(&source, &e.path, true) title=e.name.clone()></iframe> }.into_any(),
                    Viewer::Text => view! { <Editor source=source.clone() entry=e.clone() read_only=!perms.edit dirty=dirty changed=move |_| ctx.refresh()/> }.into_any(),
                    Viewer::Office => {
                        let caps = ctx.caps.get_untracked().unwrap_or_default();
                        if caps.enabled && caps.kinds.get("office").copied().unwrap_or(false) {
                            picture(ctx, source.clone(), e.clone(), "office").into_any()
                        } else {
                            other(&source, &e, "Previews of office files are off on this server.").into_any()
                        }
                    }
                    Viewer::Other => other(&source, &e, "There is no preview for this kind of file.").into_any(),
                }
            };
            let media_kind = matches!(kind, Viewer::Video | Viewer::Audio);
            let (e1, e2, e3) = (e.clone(), e.clone(), e.clone());
            let open_url = download_url(&source, &e.path, true);
            view! {
                <div class="viewer" role="dialog" aria-modal="true" aria-label=e.name.clone() on:pointerdown=on_down on:pointerup=on_up>
                    <div class="viewer-bar">
                        <button type="button" class="btn ghost icon" title="Close (Esc)" aria-label="Close" on:click=move |_| close() inner_html=icon("x")></button>
                        <div class="grow viewer-title">
                            <strong class="ellipsis">{e.name.clone()}</strong>
                            <span class="small faint hide-phone">{format!("{} · {}", format_size(e.size), format_date(&e.modified))}</span>
                        </div>
                        {(media_kind && can_cast()).then(|| view! { <button type="button" class="btn ghost icon" title="Cast" aria-label="Cast" on:click=|_| cast() inner_html=icon("cast")></button> })}
                        <button type="button" class="btn ghost icon" title=move || if ctx.starred(&e1) { "Remove star" } else { "Star" } aria-label="Star"
                            on:click=move |_| ops::toggle_star(&ctx, vec![e2.clone()])
                            inner_html=move || icon(if ctx.starred(&e3) { "star-filled" } else { "star" })></button>
                        {perms.download.then(|| view! {
                            <a class="btn ghost icon hide-phone" href=open_url target="_blank" rel="noopener" title="Open in a new tab" aria-label="Open in a new tab" inner_html=icon("external")></a>
                        })}
                        {perms.download.then(|| { let e = e.clone(); view! {
                            <button type="button" class="btn ghost icon" title="Download" aria-label="Download" on:click=move |_| ops::download(&ctx, std::slice::from_ref(&e)) inner_html=icon("download")></button>
                        }})}
                        {
                            let e = e.clone();
                            view! { <button type="button" class="btn ghost icon" title="More" aria-label="More actions" on:click=move |ev: leptos::ev::MouseEvent| {
                                let (x, y) = ev.current_target().and_then(|t| t.dyn_into::<web_sys::Element>().ok()).map(|el| super::menu::below(&el, true)).unwrap_or((0.0, 0.0));
                                let items = vec![e.clone()];
                                ctx.menu.set(Some(super::menu::MenuState { x, y, items: ops::item_menu(&ctx, items) }));
                            } inner_html=icon("more")></button> }
                        }
                    </div>
                    <div class="viewer-body" class:text=kind == Viewer::Text>
                        {many.then(|| view! { <button type="button" class="viewer-nav prev" title="Previous (←)" aria-label="Previous file" on:click=move |_| step(-1) inner_html=icon("chevron-left")></button> })}
                        {body}
                        {many.then(|| view! { <button type="button" class="viewer-nav next" title="Next (→)" aria-label="Next file" on:click=move |_| step(1) inner_html=icon("chevron-right")></button> })}
                    </div>
                </div>
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(name: &str, dir: bool) -> Entry {
        Entry { name: name.into(), path: format!("/{name}"), is_dir: dir, size: 1, modified: String::new(), mime: None }
    }

    #[test]
    fn stepping_skips_folders_and_wraps() {
        let v = vec![f("d", true), f("a", false), f("b", false), f("c", false)];
        assert_eq!(neighbour(&v, "/a", 1).unwrap().name, "b");
        assert_eq!(neighbour(&v, "/c", 1).unwrap().name, "a");
        assert_eq!(neighbour(&v, "/a", -1).unwrap().name, "c");
        assert!(neighbour(&v[..2], "/a", 1).is_none());
        assert!(neighbour(&v, "/zz", 1).is_none());
    }
}
