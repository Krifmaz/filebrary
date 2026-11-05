//! `/search`: one search across every source the account can see, by name (the name index) or by the text
//! inside files (the content index), with type chips and filters for source, modified date, size, extension
//! and sort. Results page in with "Show more", names show the searched words marked, text matches show an
//! excerpt, and every filter lives in the address so a search is a link that can be shared or bookmarked.
//!
//! | File | What |
//! | --- | --- |
//! | `query.rs` | the filters, the address and API queries they make, the words a result shows (tested) |
//! | `client.rs` | `GET /api/search` and its answer |
//! | `search.css` | the page's own rules (design tokens only) |

pub mod client;
pub mod query;

use crate::api::{self, Source};
use crate::route::{on_link, url, Route};
use client::{Hit, Results};
use leptos::prelude::*;
use query::{Mode, Search, SIZES, TYPES, WHEN};

/// The page's own CSS, added to the page with it.
const CSS: &str = include_str!("search.css");

/// What the results area shows.
#[derive(Clone, Debug, PartialEq)]
enum Shown {
    /// No words and no filters yet.
    Idle,
    /// The first page is on its way.
    Loading,
    /// The search failed.
    Failed(api::Error),
    /// Results (maybe none).
    Done(Results),
}

/// The icon of a result's type: Filebrary's folder, or a file shape for the rest.
fn kind_svg(kind: &str, is_dir: bool) -> String {
    let name = query::kind_icon(kind, is_dir);
    let p = match name {
        "folder" => return crate::icons::icon("folder"),
        "image" => r#"<rect x="3" y="3" width="18" height="18" rx="2"/><circle cx="9" cy="9" r="2"/><path d="m21 15-5-5L5 21"/>"#,
        "video" => r#"<rect x="2" y="5" width="14" height="14" rx="2"/><path d="m16 10 6-3v10l-6-3z"/>"#,
        "audio" => r#"<path d="M9 18V5l12-2v13"/><circle cx="6" cy="18" r="3"/><circle cx="18" cy="16" r="3"/>"#,
        "document" => r#"<path d="M14 3H6a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V9z"/><path d="M14 3v6h6M8 13h8M8 17h5"/>"#,
        "code" => r#"<path d="m16 18 6-6-6-6M8 6l-6 6 6 6"/>"#,
        "archive" => r#"<rect x="3" y="3" width="18" height="5" rx="1"/><path d="M5 8v11a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8M10 12h4"/>"#,
        _ => r#"<path d="M14 3H6a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V9z"/><path d="M14 3v6h6"/>"#,
    };
    format!(
        r#"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">{p}</svg>"#
    )
}

/// The address bar's query now (`?q=…`).
fn location_query() -> String {
    kit_leptos::rt::window().and_then(|w| w.location().search().ok()).unwrap_or_default()
}

/// Today (UTC) as `YYYY-MM-DD`, and now in Unix seconds.
fn today_and_now() -> (String, u64) {
    let d = js_sys::Date::new_0();
    let iso: String = d.to_iso_string().into();
    (iso.get(..10).unwrap_or_default().to_string(), (js_sys::Date::now() / 1000.0) as u64)
}

/// Puts `s` in the address bar without a new history entry (filters change the search, not the page).
fn write_address(s: &Search) {
    let target = url(&format!("/search{}", s.address_query()));
    let Some(w) = kit_leptos::rt::window() else { return };
    let now = format!("{}{}", w.location().pathname().unwrap_or_default(), w.location().search().unwrap_or_default());
    if now != target {
        if let Ok(h) = w.history() {
            let _ = h.replace_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(&target));
        }
    }
}

/// One `<select>` of the filter bar over a `(value, label, …)` table.
fn picker(
    label: &'static str,
    id: &'static str,
    choices: Vec<(String, String)>,
    value: Signal<String>,
    set: impl Fn(String) + 'static,
) -> impl IntoView {
    view! {
        <select class="input" id=id aria-label=label title=label
            on:change=move |e| set(event_target_value(&e))
            prop:value=move || value.get()>
            {choices.into_iter().map(|(v, l)| {
                let v2 = v.clone();
                view! { <option value=v selected=move || value.get() == v2>{l}</option> }
            }).collect_view()}
        </select>
    }
}

/// One result row: a link to the folder (or to the folder the file is in), with a download button for files.
fn hit_row(h: Hit, words: String, mode: Mode, source_name: String, can_download: bool, today: String) -> impl IntoView {
    let folder = if h.is_dir { h.path.clone() } else { query::parent(&h.path) };
    let to = Route::Files { source: h.source.clone(), path: folder };
    let href = url(&to.href());
    let name_html = if mode == Mode::Names { query::marked(&h.name, &words) } else { kit_leptos::html::esc(&h.name) };
    let where_ = format!("{source_name} · {}", h.path);
    let tip = where_.clone();
    let snippet = h.snippet.as_deref().filter(|s| !s.trim().is_empty()).map(query::safe_snippet);
    let size = (!h.is_dir).then(|| query::size_words(h.size));
    let date = h.modified.as_deref().map(|m| query::date_words(m, &today)).unwrap_or_default();
    let dl = (can_download && !h.is_dir).then(|| api::download_url(&h.source, &h.path, false));
    let (dl_title, dl_label) = (format!("Download {}", h.name), format!("Download {}", h.name));
    let kind = h.kind.clone();
    view! {
        <a class="sr-item" href=href on:click=on_link(to) data-kind=kind title=tip>
            <span class="sr-icon" inner_html=kind_svg(&h.kind, h.is_dir)></span>
            <span class="sr-text">
                <span class="sr-name" inner_html=name_html></span>
                <span class="sr-path">{where_}</span>
                {snippet.map(|s| view! { <span class="sr-snip" inner_html=s></span> })}
            </span>
            <span class="sr-meta">
                {size.map(|s| view! { <span>{s}</span> })}
                <span>{date}</span>
            </span>
            {dl.map(|d| view! {
                <span class="sr-dl btn ghost icon" role="button" tabindex="0" title=dl_title aria-label=dl_label
                    inner_html=crate::icons::icon("download")
                    on:click=move |e: leptos::ev::MouseEvent| {
                        e.prevent_default();
                        e.stop_propagation();
                        if let Some(w) = kit_leptos::rt::window() { let _ = w.location().set_href(&d); }
                    }></span>
            })}
        </a>
    }
}

/// `/search?q=…&type=…` plus the page's own filters (`in`, `src`, `when`, `size`, `sort`, `ext`).
#[component]
pub fn SearchPage(q: Signal<String>, kind: Signal<String>) -> impl IntoView {
    // The whole search, filters included. A new address (the top bar's search, Back, Forward) reads it again;
    // a plain top-bar search keeps the chosen mode and sort.
    let search = RwSignal::new(Search::from_query(&q.get_untracked(), &kind.get_untracked(), &location_query()));
    Effect::new(move |first: Option<()>| {
        let (q, kind) = (q.get(), kind.get());
        if first.is_some() {
            let loc = location_query();
            let mut next = Search::from_query(&q, &kind, &loc);
            if Search::from_query("", "", &loc) == Search::default() {
                let prev = search.get_untracked();
                next.mode = prev.mode;
                next.sort = prev.sort;
            }
            search.set(next);
        }
    });
    let change = move |f: &dyn Fn(&mut Search)| {
        let mut s = search.get_untracked();
        f(&mut s);
        search.set(s);
    };
    Effect::new(move |_| write_address(&search.get()));

    let sources = LocalResource::new(|| async { api::sources().await.unwrap_or_default() });
    let source_list = move || sources.get().unwrap_or_default();

    // Fetching: each search gets a number, and answers to an older one are dropped.
    let (shown, set_shown) = signal(Shown::Idle);
    let (more_busy, set_more_busy) = signal(false);
    let generation = StoredValue::new(0u64);
    let (retry, set_retry) = signal(0u32);
    Effect::new(move |_| {
        retry.track();
        let s = search.get();
        let gen = generation.get_value() + 1;
        generation.set_value(gen);
        set_more_busy.set(false);
        if s.is_empty() {
            set_shown.set(Shown::Idle);
            return;
        }
        set_shown.set(Shown::Loading);
        let path = s.api_path(0, today_and_now().1);
        kit_leptos::rt::spawn(async move {
            let r = client::search(&path).await;
            if generation.get_value() == gen {
                set_shown.set(match r {
                    Ok(r) => Shown::Done(r),
                    Err(e) => Shown::Failed(e),
                });
            }
        });
    });
    let load_more = move |_| {
        let Shown::Done(have) = shown.get_untracked() else { return };
        let gen = generation.get_value();
        set_more_busy.set(true);
        let path = search.get_untracked().api_path(have.hits.len() as u32, today_and_now().1);
        kit_leptos::rt::spawn(async move {
            let r = client::search(&path).await;
            if generation.get_value() != gen {
                return;
            }
            set_more_busy.set(false);
            match r {
                Ok(r) => set_shown.update(|s| {
                    if let Shown::Done(have) = s {
                        have.hits.extend(r.hits);
                        have.has_more = r.has_more;
                        have.indexing = r.indexing;
                    }
                }),
                Err(e) => kit_leptos::toasts::toast(&e.message, "err"),
            }
        });
    };

    let title = move || search.with(Search::title);
    let count = move || match shown.get() {
        Shown::Done(r) => query::count_line(r.hits.len(), r.has_more, r.indexing),
        _ => String::new(),
    };

    // the type chips
    let chips = TYPES
        .iter()
        .map(|&(t, label)| {
            let on = move || search.with(|s| s.kind == t);
            view! {
                <button type="button" class="sr-chip" role="radio" aria-checked=move || on().to_string()
                    on:click=move |_| change(&|s: &mut Search| s.kind = t.to_string())>{label}</button>
            }
        })
        .collect_view();

    // the filter bar
    let (filters_open, set_filters_open) = signal(false);
    let mode_btn = move |m: Mode, label: &'static str, tip: &'static str| {
        view! {
            <button type="button" class:on=move || search.with(|s| s.mode == m) aria-pressed=move || search.with(|s| s.mode == m).to_string() title=tip
                on:click=move |_| change(&|s: &mut Search| s.mode = m)>{label}</button>
        }
    };
    let field = move |f: fn(&Search) -> String| Signal::derive(move || search.with(f));
    let table = |t: &[(&str, &str)]| t.iter().map(|(v, l)| (v.to_string(), l.to_string())).collect::<Vec<_>>();
    let when_choices = table(&WHEN.map(|w| (w.0, w.1)));
    let size_choices = table(&SIZES.map(|w| (w.0, w.1)));
    let source_picker = move || {
        let mut choices = vec![(String::new(), "All sources".to_string())];
        choices.extend(source_list().into_iter().map(|s: Source| (s.id, s.name)));
        picker("Source", "sr-source", choices, field(|s| s.source.clone()), move |v| change(&|s: &mut Search| s.source = v.clone()))
    };
    let ext_input = NodeRef::<leptos::html::Input>::new();
    let set_ext = move || {
        let v = query::clean_ext(&ext_input.get().map(|i| i.value()).unwrap_or_default());
        if let Some(i) = ext_input.get() {
            i.set_value(&v);
        }
        change(&|s: &mut Search| s.ext = v.clone());
    };
    let mode = Memo::new(move |_| search.with(|s| s.mode));
    let filter_count = move || search.with(Search::filter_count);
    let toggle_words = move || match filter_count() {
        0 => "Filters".to_string(),
        n => format!("Filters ({n})"),
    };

    // the results area
    let body = move || {
        let today = today_and_now().0;
        let s = search.get();
        match shown.get() {
            Shown::Idle => view! {
                <div class="card"><div class="empty" id="sr-idle" inner_html=format!(
                    r#"{}<h3 style="margin:0">Search all your files</h3><p class="muted" style="margin:0">Type in the search box at the top to look across every source, or pick a type above to list, say, every image.</p>"#,
                    crate::icons::icon("search"))></div></div>
            }.into_any(),
            Shown::Loading => view! {
                <div class="card sr-list" aria-busy="true">{(0..6).map(|_| view! { <div class="skeleton sr-skel"></div> }).collect_view()}</div>
            }.into_any(),
            Shown::Failed(e) if client::content_off(&e) => view! {
                <div class="card"><div class="empty" id="sr-content-off">
                    <span inner_html=crate::icons::icon("search")></span>
                    <h3 style="margin:0">"Text search is off on this server"</h3>
                    <p class="muted" style="margin:0">"An administrator can turn on the content index. Names can still be searched."</p>
                    <button class="btn primary" on:click=move |_| change(&|s: &mut Search| s.mode = Mode::Names)>"Search names instead"</button>
                </div></div>
            }.into_any(),
            Shown::Failed(e) if client::search_off(&e) => view! {
                <div class="card"><div class="empty" id="sr-off">
                    <span inner_html=crate::icons::icon("search")></span>
                    <h3 style="margin:0">"Search is turned off on this server"</h3>
                    <p class="muted" style="margin:0">"An administrator can turn the search index on in the server's settings."</p>
                </div></div>
            }.into_any(),
            Shown::Failed(e) => {
                let msg = e.message.clone();
                view! {
                    <div class="card"><div class="empty" id="sr-failed">
                        <h3 style="margin:0">"Search failed"</h3>
                        <div class="error-box" data-details=msg.clone() data-details-title="Search failed">{msg.clone()}</div>
                        <button class="btn" on:click=move |_| set_retry.update(|n| *n += 1)>"Try again"</button>
                    </div></div>
                }.into_any()
            }
            Shown::Done(r) if r.hits.is_empty() => {
                let filtered = s.filter_count() > 0 || !s.kind.is_empty();
                let names = s.mode == Mode::Names && !s.q.trim().is_empty();
                view! {
                    <div class="card"><div class="empty" id="sr-none">
                        <span inner_html=crate::icons::icon("search")></span>
                        <h3 style="margin:0">"No matches"</h3>
                        <p class="muted" style="margin:0">{if filtered { "Try fewer words, or loosen the filters." } else { "Try fewer or shorter words." }}</p>
                        <div class="row wrap" style="justify-content:center">
                            {filtered.then(|| view! { <button class="btn" on:click=move |_| change(&|s: &mut Search| *s = s.cleared())>"Clear filters"</button> })}
                            {names.then(|| view! { <button class="btn" on:click=move |_| change(&|s: &mut Search| s.mode = Mode::Content)>"Search inside files"</button> })}
                        </div>
                    </div></div>
                }.into_any()
            }
            Shown::Done(r) => {
                let list = source_list();
                let rows = r.hits.into_iter().map(|h| {
                    let src = list.iter().find(|x| x.id == h.source);
                    let name = src.map_or_else(|| h.source.clone(), |x| x.name.clone());
                    let dl = src.is_some_and(|x| x.permissions.download);
                    hit_row(h, s.q.clone(), s.mode, name, dl, today.clone())
                }).collect_view();
                view! {
                    <div class="card sr-list" id="sr-results" role="list">
                        {rows}
                        {r.has_more.then(|| view! {
                            <div class="sr-more">
                                <button class="btn" id="sr-more" disabled=move || more_busy.get() on:click=load_more>
                                    {move || if more_busy.get() { "Loading…" } else { "Show more" }}
                                </button>
                            </div>
                        })}
                    </div>
                }.into_any()
            }
        }
    };

    view! {
        <style>{CSS}</style>
        <div data-page="search">
            <div class="sr-head">
                <div style="min-width:0">
                    <h1>{title}</h1>
                    <p class="muted small" id="sr-count" aria-live="polite">{count}</p>
                </div>
            </div>
            <div class="sr-chips" role="radiogroup" aria-label="Type">{chips}</div>
            <div class="sr-bar">
                <div class="seg" role="group" aria-label="Look in">
                    {mode_btn(Mode::Names, "Names", "Search file and folder names")}
                    {mode_btn(Mode::Content, "Inside files", "Search the text inside files")}
                </div>
                <button type="button" class="btn sr-toggle" aria-expanded=move || filters_open.get().to_string() aria-controls="sr-filters"
                    on:click=move |_| set_filters_open.update(|o| *o = !*o)>{toggle_words}</button>
                <div class="sr-filters" id="sr-filters" class:open=filters_open>
                    {source_picker}
                    {picker("Modified", "sr-when", when_choices, field(|s| s.when.clone()), move |v| change(&|s: &mut Search| s.when = v.clone()))}
                    {picker("Size", "sr-size", size_choices, field(|s| s.size.clone()), move |v| change(&|s: &mut Search| s.size = v.clone()))}
                    <input class="input sr-ext" id="sr-ext" type="text" placeholder="Extensions: pdf, jpg" aria-label="Extensions" autocomplete="off"
                        node_ref=ext_input prop:value=move || search.with(|s| s.ext.clone())
                        on:change=move |_| set_ext()
                        on:keydown=move |e: leptos::ev::KeyboardEvent| if e.key() == "Enter" { set_ext() }/>
                    {move || {
                        let shown_sort = field(|s| if s.mode == Mode::Names && s.sort == "name" { String::new() } else { s.sort.clone() });
                        picker("Sort", "sr-sort", query::sort_choices(mode.get()), shown_sort, move |v| change(&|s: &mut Search| s.sort = v.clone()))
                    }}
                    <Show when=move || { filter_count() > 0 }>
                        <button type="button" class="btn ghost" id="sr-clear" on:click=move |_| change(&|s: &mut Search| *s = s.cleared())>"Clear filters"</button>
                    </Show>
                </div>
            </div>
            {body}
        </div>
    }
}
