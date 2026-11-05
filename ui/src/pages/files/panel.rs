//! The upload panel: a card in the corner with the overall progress, time left and each file with Pause,
//! Resume, Retry, Cancel, and Replace or Keep both for a file that is already there. It is mounted once, the
//! first time something is uploaded, and stays while the person moves between folders.

use super::kinds::file_icon;
use super::paths::format_size;
use super::upload::{self, eta_words, summary, Item, Status};
use crate::icons::icon;
use leptos::prelude::*;

thread_local! {
    static SHOWN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Puts the panel on the page (once).
pub fn show() {
    if SHOWN.with(|s| s.replace(true)) {
        return;
    }
    let Some(doc) = kit_leptos::rt::document() else { return };
    let Some(body) = doc.body() else { return };
    let Ok(host) = doc.create_element("div") else { return };
    let _ = host.set_attribute("id", "uploads");
    let _ = body.append_child(&host);
    // its own root, kept for the page's life: the scope that first showed it (the folder) can go away
    let owner = Owner::new_root(None);
    kit_leptos::rt::keep("uploads", owner.with(|| kit_leptos::rt::mount(&host, || view! { <UploadPanel/> })));
    std::mem::forget(owner);
}

/// The words under a file's name.
pub fn line(i: &Item) -> String {
    match i.status {
        Status::Failed | Status::Exists => i.error.clone(),
        Status::Cancelled => "Cancelled".into(),
        Status::Paused => format!("Paused · {} of {}", format_size(i.sent), format_size(i.size)),
        Status::Done => format_size(i.size),
        Status::Queued => "Waiting".into(),
        Status::Uploading if i.rate > 0.0 => {
            format!("{} of {} · {}/s", format_size(i.sent), format_size(i.size), format_size(i.rate as u64))
        }
        Status::Uploading => format!("{} of {}", format_size(i.sent), format_size(i.size)),
    }
}

fn file_row(i: Item) -> impl IntoView {
    let k = i.key;
    let pct = if i.size == 0 {
        if i.status == Status::Done {
            100.0
        } else {
            0.0
        }
    } else {
        (i.sent as f64 / i.size as f64 * 100.0).min(100.0)
    };
    let name = i.name.clone();
    let status_class = format!("upload {}", format!("{:?}", i.status).to_lowercase());
    let buttons = match i.status {
        Status::Uploading | Status::Queued => view! {
            <button type="button" class="btn ghost icon sm" title="Pause" aria-label=format!("Pause {name}") on:click=move |_| upload::pause(k) inner_html=icon("pause")></button>
            <button type="button" class="btn ghost icon sm" title="Cancel" aria-label=format!("Cancel {name}") on:click=move |_| upload::cancel(k) inner_html=icon("x")></button>
        }
        .into_any(),
        Status::Paused | Status::Failed => {
            let (t, ic) = if i.status == Status::Failed { ("Retry", "refresh") } else { ("Resume", "play") };
            view! {
                <button type="button" class="btn ghost icon sm" title=t aria-label=format!("{t} {name}") on:click=move |_| upload::resume(k) inner_html=icon(ic)></button>
                <button type="button" class="btn ghost icon sm" title="Cancel" aria-label=format!("Cancel {name}") on:click=move |_| upload::cancel(k) inner_html=icon("x")></button>
            }
            .into_any()
        }
        Status::Exists => view! {
            <button type="button" class="btn sm" on:click=move |_| upload::replace(k)>"Replace"</button>
            <button type="button" class="btn sm" on:click=move |_| upload::keep_both(k)>"Keep both"</button>
            <button type="button" class="btn ghost icon sm" title="Skip" aria-label=format!("Skip {name}") on:click=move |_| upload::cancel(k) inner_html=icon("x")></button>
        }
        .into_any(),
        Status::Done => view! { <span class="upload-done" title="Done" inner_html=icon("check")></span> }.into_any(),
        Status::Cancelled => ().into_any(),
    };
    let err = matches!(i.status, Status::Failed | Status::Exists);
    view! {
        <li class=status_class>
            <span class="file-icon" inner_html=icon(file_icon(&i.name, false))></span>
            <div class="upload-name ellipsis" title=i.path.clone()>{i.name.clone()}</div>
            <span class="upload-actions">{buttons}</span>
            <div class="upload-meta">
                <span class="upload-bar" role="progressbar" aria-label=format!("{} progress", i.name) aria-valuemin="0" aria-valuemax="100" aria-valuenow=pct.round().to_string()>
                    <i style=format!("width:{pct:.1}%")></i>
                </span>
                <span class="ellipsis" class:field-err=err data-details="" title=line(&i)>{line(&i)}</span>
            </div>
        </li>
    }
}

/// The panel.
#[component]
pub fn UploadPanel() -> impl IntoView {
    let items: RwSignal<Vec<Item>> = upload::items().into();
    let collapsed = RwSignal::new(false);
    let s = Memo::new(move |_| summary(&items.get()));
    let title = move || {
        let s = s.get();
        if s.active > 0 {
            format!("Uploading {} file{}", s.active, if s.active == 1 { "" } else { "s" })
        } else if s.failed > 0 {
            format!("{} upload{} need{} you", s.failed, if s.failed == 1 { "" } else { "s" }, if s.failed == 1 { "s" } else { "" })
        } else {
            "Uploads complete".into()
        }
    };
    let sub = move || {
        let s = s.get();
        let eta = s.eta.map(|e| format!(" · {}", eta_words(e))).unwrap_or_default();
        format!("{} of {} · {}%{eta}", format_size(s.sent), format_size(s.total), s.pct)
    };
    let head_icon = move || {
        let s = s.get();
        if s.active > 0 {
            view! { <span class="spinner"></span> }.into_any()
        } else if s.failed > 0 {
            view! { <span class="field-err" inner_html=icon("alert")></span> }.into_any()
        } else {
            view! { <span class="upload-done" inner_html=icon("check-circle")></span> }.into_any()
        }
    };
    view! {
        <Show when=move || !items.get().is_empty()>
            <aside class="uploads" aria-label="Uploads">
                <header class="uploads-head">
                    {head_icon}
                    <div class="uploads-title">
                        <strong>{title}</strong>
                        <span class="small muted">{sub}</span>
                    </div>
                    <button type="button" class="btn ghost icon sm" title=move || if collapsed.get() { "Show files" } else { "Hide files" }
                        on:click=move |_| collapsed.update(|c| *c = !*c) inner_html=move || icon(if collapsed.get() { "arrow-up" } else { "arrow-down" })></button>
                    <button type="button" class="btn ghost icon sm" title=move || { if s.get().active > 0 { "Cancel all" } else { "Clear" } }
                        on:click=move |_| { if s.get_untracked().active > 0 {
                            kit_leptos::rt::spawn(async {
                                if kit_leptos::modal::confirm_dialog("Cancel every upload?", "Files that finished stay. The rest stop and are not kept.", "Cancel uploads", true).await {
                                    upload::cancel_all();
                                }
                            });
                        } else { upload::clear_finished() } }
                        inner_html=icon("x")></button>
                </header>
                <div class="uploads-total" role="progressbar" aria-label="Total progress" aria-valuemin="0" aria-valuemax="100" aria-valuenow=move || s.get().pct.to_string()>
                    <i style=move || format!("width:{}%", s.get().pct)></i>
                </div>
                <Show when=move || !collapsed.get()>
                    <ul class="uploads-list">
                        {move || items.get().into_iter().rev().map(file_row).collect_view()}
                    </ul>
                </Show>
            </aside>
        </Show>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_say_where_a_file_stands() {
        let mut i = Item {
            key: 1,
            source: "s".into(),
            path: "/a".into(),
            name: "a".into(),
            size: 2000,
            sent: 1000,
            status: Status::Uploading,
            error: String::new(),
            rate: 500.0,
        };
        assert_eq!(line(&i), "1.0 kB of 2.0 kB · 500 B/s");
        i.status = Status::Paused;
        assert_eq!(line(&i), "Paused · 1.0 kB of 2.0 kB");
        i.status = Status::Exists;
        i.error = "A file with this name is already there".into();
        assert_eq!(line(&i), "A file with this name is already there");
    }
}
