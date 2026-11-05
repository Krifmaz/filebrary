//! The text editor: a plain, fast text area for text and code files up to 5 MiB.
//!
//! It loads the file with its tag and saves with `If-Match`, so a change made elsewhere in the meantime
//! (another tab, another person, WebDAV, the disk) comes back as a conflict instead of being overwritten: the
//! person picks Overwrite or Reload. Accounts without `edit` get a read-only view. Ctrl/Cmd+S saves, Tab
//! indents, the wrap switch is remembered per browser, and closing with unsaved changes asks first.

use super::api::{self, Saved};
use super::icons::icon;
use crate::api::Entry;
use kit_leptos::modal;
use kit_leptos::toasts::toast;
use leptos::prelude::*;
use wasm_bindgen::JsCast;

const WRAP_KEY: &str = "filebrary.editor.wrap";

fn wrap_saved() -> bool {
    kit_leptos::rt::window()
        .and_then(|w| w.local_storage().ok().flatten())
        .and_then(|s| s.get_item(WRAP_KEY).ok().flatten())
        .map(|v| v != "0")
        .unwrap_or(true)
}

fn save_wrap(on: bool) {
    if let Some(s) = kit_leptos::rt::window().and_then(|w| w.local_storage().ok().flatten()) {
        let _ = s.set_item(WRAP_KEY, if on { "1" } else { "0" });
    }
}

/// Where the editor stands.
#[derive(Clone, Debug, PartialEq)]
enum State {
    Loading,
    Failed(String),
    Ready,
}

/// The words for an error loading a file.
pub fn load_error(code: &str, message: &str) -> String {
    match code {
        "not_text" => "This file isn't text, so the editor can't open it. Download it instead.".into(),
        "payload_too_large" => "This file is larger than 5 MB, too large for the editor. Download it instead.".into(),
        _ => message.to_string(),
    }
}

/// The number of lines of `text` (at least 1).
pub fn line_count(text: &str) -> usize {
    text.split('\n').count().max(1)
}

/// The text editor for `entry`; `dirty` says whether there are unsaved changes; `changed` runs after a save.
#[component]
pub fn Editor(source: String, entry: Entry, read_only: bool, dirty: RwSignal<bool>, #[prop(into)] changed: Callback<()>) -> impl IntoView {
    let state = RwSignal::new(State::Loading);
    let text = RwSignal::new(String::new());
    let saved_text = RwSignal::new(String::new());
    let etag = RwSignal::new(String::new());
    let saving = RwSignal::new(false);
    let wrap = RwSignal::new(wrap_saved());
    let area = NodeRef::<leptos::html::Textarea>::new();
    Effect::new(move |_| dirty.set(text.get() != saved_text.get()));

    let (s0, p0) = (source.clone(), entry.path.clone());
    kit_leptos::rt::spawn(async move {
        match api::load_text(&s0, &p0).await {
            Ok(f) => {
                let _ = text.try_set(f.content.clone());
                let _ = saved_text.try_set(f.content);
                let _ = etag.try_set(f.etag);
                let _ = state.try_set(State::Ready);
            }
            Err(e) => {
                let _ = state.try_set(State::Failed(load_error(crate::api::code(&e), &e.message)));
            }
        }
    });

    let (s1, p1) = (source.clone(), entry.path.clone());
    let save = move || {
        if read_only || saving.get_untracked() || !dirty.get_untracked() {
            return;
        }
        let (s, p, content, tag) = (s1.clone(), p1.clone(), text.get_untracked(), etag.get_untracked());
        saving.set(true);
        kit_leptos::rt::spawn(async move {
            let mut tag = Some(tag);
            loop {
                match api::save_text(&s, &p, &content, tag.as_deref()).await {
                    Ok(Saved::Ok(t)) => {
                        etag.set(t);
                        saved_text.set(content.clone());
                        toast("Saved", "ok");
                        changed.run(());
                        break;
                    }
                    Ok(Saved::Conflict(now)) => {
                        let overwrite = modal::confirm_dialog(
                            "This file changed since you opened it",
                            "Someone or something else saved it meanwhile. Overwrite their version with yours, or cancel and copy your text before reloading.",
                            "Overwrite",
                            true,
                        )
                        .await;
                        if !overwrite {
                            break;
                        }
                        tag = now;
                    }
                    Err(e) => {
                        toast(&e.message, "err");
                        break;
                    }
                }
            }
            saving.set(false);
        });
    };
    let save2 = save.clone();
    let on_key = move |e: leptos::ev::KeyboardEvent| {
        if (e.ctrl_key() || e.meta_key()) && e.key().eq_ignore_ascii_case("s") {
            e.prevent_default();
            save2();
        } else if e.key() == "Tab" && !e.ctrl_key() && !e.alt_key() && !read_only {
            // indent instead of leaving the text
            let Some(t) = area.get() else { return };
            let t: web_sys::HtmlTextAreaElement = t.unchecked_into();
            e.prevent_default();
            let v = t.value();
            let (a, b) = (t.selection_start().ok().flatten().unwrap_or(0) as usize, t.selection_end().ok().flatten().unwrap_or(0) as usize);
            let units: Vec<u16> = v.encode_utf16().collect();
            let (a, b) = (a.min(units.len()), b.min(units.len()));
            let mut out: Vec<u16> = units[..a].to_vec();
            out.extend("\t".encode_utf16());
            out.extend_from_slice(&units[b..]);
            let nv = String::from_utf16_lossy(&out);
            t.set_value(&nv);
            let _ = t.set_selection_range(a as u32 + 1, a as u32 + 1);
            text.set(nv);
        }
    };
    let reload = {
        let (s, p) = (source.clone(), entry.path.clone());
        move |_| {
            let (s, p) = (s.clone(), p.clone());
            kit_leptos::rt::spawn(async move {
                if dirty.get_untracked()
                    && !modal::confirm_dialog(
                        "Reload and lose your changes?",
                        "Your unsaved text is replaced by the file as it is now.",
                        "Reload",
                        true,
                    )
                    .await
                {
                    return;
                }
                match api::load_text(&s, &p).await {
                    Ok(f) => {
                        text.set(f.content.clone());
                        saved_text.set(f.content);
                        etag.set(f.etag);
                    }
                    Err(e) => toast(&e.message, "err"),
                }
            });
        }
    };
    let lines = move || line_count(&text.get());
    view! {
        <div class="editor">
            <div class="editor-bar">
                <span class="small muted">{move || if read_only { "Read only".to_string() } else if dirty.get() { "Unsaved changes".to_string() } else { "Saved".to_string() }}</span>
                <span class="small faint">{move || format!("{} lines", lines())}</span>
                <span class="grow"></span>
                <button type="button" class="btn ghost sm" class:on=wrap title="Wrap long lines" aria-pressed=move || wrap.get().to_string()
                    on:click=move |_| { let w = !wrap.get_untracked(); wrap.set(w); save_wrap(w); }>
                    <span inner_html=icon("wrap")></span>"Wrap"
                </button>
                <button type="button" class="btn ghost sm" title="Load the file again" on:click=reload><span inner_html=icon("refresh")></span>"Reload"</button>
                {(!read_only).then(|| view! {
                    <button type="button" class="btn primary sm" title="Save (Ctrl+S)" disabled=move || !dirty.get() || saving.get() on:click=move |_| save()>
                        <span inner_html=icon("save")></span>{move || if saving.get() { "Saving…" } else { "Save" }}
                    </button>
                })}
            </div>
            {move || match state.get() {
                State::Loading => view! { <div class="viewer-center"><span class="spinner"></span></div> }.into_any(),
                State::Failed(m) => view! { <div class="viewer-center"><div class="error-box" data-details="">{m}</div></div> }.into_any(),
                State::Ready => view! {
                    <textarea class="editor-text mono" class:nowrap=move || !wrap.get() node_ref=area spellcheck="false" autocomplete="off" readonly=read_only
                        aria-label=format!("Text of {}", entry.name)
                        prop:value=move || text.get() on:input=move |e| text.set(event_target_value(&e)) on:keydown=on_key.clone()></textarea>
                }.into_any(),
            }}
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_and_lines() {
        assert!(load_error("not_text", "x").contains("isn't text"));
        assert!(load_error("payload_too_large", "x").contains("5 MB"));
        assert_eq!(load_error("", "Nope"), "Nope");
        assert_eq!(line_count(""), 1);
        assert_eq!(line_count("a\nb\n"), 3);
    }
}
