//! The file browser for one folder of one source.
//!
//! Layout: a head with breadcrumbs and the folder's main actions (Upload, New folder, Share); a toolbar with the
//! folder filter, sort and view switcher, which turns into a selection bar while items are selected; then the
//! listing ([`super::listing`]). On phones the actions move to a bar at the bottom of the screen.
//!
//! Keyboard: Enter opens, Delete deletes, F2 renames, Ctrl/Cmd+A selects all, Escape clears the selection,
//! Backspace goes up, arrows move the selection (Shift extends it), `1`–`5` pick a view and `V` / `Shift+V` cycle
//! them, `S` stars, `N` makes a folder, `U` uploads, Shift+F10 or the menu key opens the menu.
//!
//! Dropping files or folders anywhere on the listing uploads them here, folders with their structure.

use super::icons::icon;
use super::kinds::ViewMode;
use super::listing::{drop_target, Listing};
use super::menu::{below, ContextMenu, MenuItem, MenuState};
use super::ops;
use super::paths::{crumbs, parent, plural, step_index, SortKey};
use super::preview::Preview;
use super::state::{Ctx, Load};
use super::{pick, upload};
use crate::api::Source;
use crate::route::{self, Route};
use leptos::prelude::*;
use std::collections::BTreeSet;
use wasm_bindgen::JsCast;

/// The id of the hidden file input the listing's drop zone feeds.
pub const FILE_INPUT: &str = "files-upload-input";
const FOLDER_INPUT: &str = "files-upload-folder";

fn click_input(id: &str) {
    if let Some(i) = kit_leptos::rt::by_id(id).and_then(|i| i.dyn_into::<web_sys::HtmlElement>().ok()) {
        i.click();
    }
}

/// Items per row in the current layout, measured on the page (1 for the table).
fn columns(mode: ViewMode) -> i64 {
    if mode == ViewMode::Details {
        return 1;
    }
    let items = kit_leptos::rt::qsa(".listing [data-path]");
    let Some(first) = items.first().and_then(|e| e.dyn_ref::<web_sys::HtmlElement>().map(|h| h.offset_top())) else { return 1 };
    items.iter().take_while(|e| e.dyn_ref::<web_sys::HtmlElement>().map(|h| h.offset_top()) == Some(first)).count().max(1) as i64
}

fn scroll_to(path: &str) {
    let sel = format!(r#".listing [data-path="{}"]"#, path.replace('\\', "\\\\").replace('"', "\\\""));
    if let Some(el) = kit_leptos::rt::qs(&sel) {
        // only when it is off screen, so the page doesn't jump on every arrow key
        let r = el.get_bounding_client_rect();
        let h = kit_leptos::rt::window().and_then(|w| w.inner_height().ok()).and_then(|v| v.as_f64()).unwrap_or(800.0);
        if r.top() < 0.0 || r.bottom() > h {
            el.scroll_into_view_with_bool(r.top() < 0.0);
        }
    }
}

/// Moves the selection `delta` items (Shift extends it).
fn move_selection(ctx: &Ctx, delta: i64, extend: bool) {
    let v = ctx.visible.get_untracked();
    let cursor = if extend {
        ctx.selected.get_untracked().into_iter().next_back().or(ctx.anchor.get_untracked())
    } else {
        ctx.anchor.get_untracked()
    };
    let at = cursor.and_then(|c| v.iter().position(|e| e.path == c));
    let Some(i) = step_index(at, delta, v.len()) else { return };
    let target = v[i].path.clone();
    if extend {
        ctx.selected.update(|s| {
            s.insert(target.clone());
        });
    } else {
        ctx.select(BTreeSet::from([target.clone()]), Some(target.clone()));
    }
    scroll_to(&target);
}

/// The keys of the browser (not while typing, not under a dialog or the viewer).
fn on_key(ctx: Ctx, ev: &web_sys::Event) {
    let Some(e) = ev.dyn_ref::<web_sys::KeyboardEvent>() else { return };
    if ev.default_prevented()
        || ctx.viewing.get_untracked().is_some()
        || ctx.menu.get_untracked().is_some()
        || kit_leptos::rt::qs(".modal-bg").is_some()
    {
        return;
    }
    if kit_leptos::rt::target_el(ev).is_some_and(|t| t.closest("input, textarea, select, [contenteditable]").ok().flatten().is_some()) {
        return;
    }
    let plain = !e.ctrl_key() && !e.meta_key() && !e.alt_key();
    let key = e.key();
    let perms = ctx.perms();
    let sel = ctx.selection_now();
    let one = (sel.len() == 1).then(|| sel[0].clone());
    let mode = ctx.mode.get_untracked();
    let handled = match key.as_str() {
        k if plain && !e.shift_key() && ViewMode::for_key(k).is_some() => {
            ctx.set_mode(ViewMode::for_key(k).unwrap_or(mode));
            true
        }
        "v" | "V" if plain => {
            ctx.set_mode(mode.step(if e.shift_key() { -1 } else { 1 }));
            true
        }
        "ArrowDown" | "ArrowUp" | "ArrowLeft" | "ArrowRight" if plain => {
            let cols = columns(mode);
            let d = match key.as_str() {
                "ArrowDown" => cols,
                "ArrowUp" => -cols,
                "ArrowRight" if mode != ViewMode::Details => 1,
                "ArrowLeft" if mode != ViewMode::Details => -1,
                _ => 0,
            };
            if d != 0 {
                move_selection(&ctx, d, e.shift_key());
            }
            d != 0
        }
        "Escape" if !sel.is_empty() => {
            ctx.clear();
            true
        }
        "a" | "A" if e.ctrl_key() || e.meta_key() => {
            ctx.select(ctx.visible.get_untracked().into_iter().map(|x| x.path).collect(), None);
            true
        }
        "Delete" if perms.delete && !sel.is_empty() => {
            ops::delete(&ctx, sel.iter().map(|x| x.path.clone()).collect());
            true
        }
        "F2" if perms.edit && one.is_some() => {
            ops::rename(&ctx, one.as_ref().expect("one"));
            true
        }
        "Enter" if one.is_some() => {
            ops::open(&ctx, one.as_ref().expect("one"));
            true
        }
        "s" | "S" if plain && !sel.is_empty() => {
            ops::toggle_star(&ctx, sel.clone());
            true
        }
        "n" | "N" if plain && perms.upload => {
            ops::new_folder(&ctx);
            true
        }
        "u" | "U" if plain && perms.upload => {
            click_input(FILE_INPUT);
            true
        }
        "ContextMenu" | "F10" if key == "ContextMenu" || e.shift_key() => {
            let (x, y) = one
                .as_ref()
                .and_then(|o| kit_leptos::rt::qs(&format!(r#".listing [data-path="{}"]"#, o.path.replace('"', "\\\""))))
                .map(|el| below(&el, false))
                .unwrap_or((120.0, 160.0));
            let items = if sel.is_empty() {
                ops::folder_menu(&ctx, || click_input(FILE_INPUT), || click_input(FOLDER_INPUT))
            } else {
                ops::item_menu(&ctx, sel)
            };
            ctx.menu.set(Some(MenuState { x: x + 24.0, y, items }));
            true
        }
        "Backspace" if ctx.path.get_untracked() != "/" => {
            route::go(Route::Files { source: ctx.source.get_untracked(), path: parent(&ctx.path.get_untracked()) }, false);
            true
        }
        _ => false,
    };
    if handled {
        e.prevent_default();
    }
}

/// The breadcrumbs, each a drop target for dragged items.
fn breadcrumbs(ctx: Ctx) -> impl IntoView {
    move || {
        let trail = crumbs(&ctx.path.get());
        let last = trail.len() - 1;
        let name = ctx.source_name();
        let source = ctx.source.get();
        view! {
            <nav class="crumbs" aria-label="Breadcrumb">
                <ol>
                    {trail.into_iter().enumerate().map(|(i, (seg, p))| {
                        let label = if i == 0 { name.clone() } else { seg };
                        let to = Route::Files { source: source.clone(), path: p.clone() };
                        let (over, drop) = drop_target(ctx, p.clone());
                        let first = (i == 0).then(|| view! { <span inner_html=icon("drive")></span> });
                        view! {
                            <li>
                                {(i > 0).then(|| view! { <span class="crumb-sep" inner_html=icon("chevron-right")></span> })}
                                {if i == last {
                                    view! { <span class="crumb current" aria-current="page">{first}{label}</span> }.into_any()
                                } else {
                                    view! { <a class="crumb" href=route::url(&to.href()) on:click=route::on_link(to.clone()) on:dragover=over on:dragleave=|ev: leptos::ev::DragEvent| { if let Some(el) = ev.current_target().and_then(|t| t.dyn_into::<web_sys::Element>().ok()) { let _ = el.class_list().remove_1("drag-over"); } } on:drop=drop>{first}{label}</a> }.into_any()
                                }}
                            </li>
                        }
                    }).collect_view()}
                </ol>
            </nav>
        }
    }
}

/// A button of the phone's bottom bar: an icon over a short word.
fn bar_button(
    ic: &'static str,
    label: &'static str,
    disabled: Signal<bool>,
    danger: bool,
    f: impl Fn(leptos::ev::MouseEvent) + 'static,
) -> impl IntoView {
    view! {
        <button type="button" class="bar-button" class:danger=danger disabled=disabled on:click=f>
            <span inner_html=icon(ic)></span>
            <span>{label}</span>
        </button>
    }
}

/// `/files/<source>/<path…>`: one folder of a source.
#[component]
pub fn FilesPage(source: Signal<String>, path: Signal<String>) -> impl IntoView {
    super::ensure_css();
    let sources = LocalResource::new(|| async { crate::api::sources().await.map_err(|e| e.message) });
    let user = match crate::use_session().get.get_untracked() {
        crate::Session::SignedIn(u) => u.id,
        _ => 0,
    };
    let ctx = Ctx::new(source, path, sources, user);
    ctx.watch();
    let unknown = Memo::new(move |_| matches!(sources.get(), Some(Ok(ref l)) if !l.iter().any(|s: &Source| s.id == source.get())));

    let keys = StoredValue::new_local(kit_leptos::rt::document().map(|d| kit_leptos::rt::listen(&d, "keydown", move |ev| on_key(ctx, ev))));
    let _ = keys;

    let perms = Signal::derive(move || ctx.perms());
    let count = Memo::new(move |_| ctx.selected.with(|s| s.len()));
    let one = Signal::derive(move || {
        let s = ctx.selection();
        (s.len() == 1).then(|| s[0].clone())
    });
    let at_root = Signal::derive(move || path.get() == "/");
    let go_up = move |_| route::go(Route::Files { source: source.get_untracked(), path: parent(&path.get_untracked()) }, false);

    let on_files = move |ev: leptos::ev::Event| {
        let Some(input) = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok()) else { return };
        let files = pick::from_input(&input);
        input.set_value("");
        if !files.is_empty() {
            upload::add(&source.get_untracked(), &path.get_untracked(), files);
        }
    };
    // a drop of files from the computer: walk dropped folders, then upload here
    let on_drop = move |ev: leptos::ev::DragEvent| {
        if !kit_leptos::dropzone::has_files(&ev) || !ctx.perms().upload {
            return; // the kit says why it can't be dropped here
        }
        ev.prevent_default();
        let dt: wasm_bindgen::JsValue = ev.data_transfer().map(Into::into).unwrap_or_default();
        let (s, p) = (source.get_untracked(), path.get_untracked());
        kit_leptos::rt::spawn(async move {
            let files = pick::from_drop(&dt).await;
            if !files.is_empty() {
                kit_leptos::toasts::toast(&format!("Uploading {}", plural(files.len(), "file")), "");
                upload::add(&s, &p, files);
            }
        });
    };
    let on_empty_menu = move |ev: leptos::ev::MouseEvent| {
        if kit_leptos::rt::closest(&ev, "[data-path]").is_some() {
            return;
        }
        ev.prevent_default();
        ctx.clear();
        ctx.menu.set(Some(MenuState {
            x: ev.client_x() as f64,
            y: ev.client_y() as f64,
            items: ops::folder_menu(&ctx, || click_input(FILE_INPUT), || click_input(FOLDER_INPUT)),
        }));
    };
    let on_empty_click = move |ev: leptos::ev::MouseEvent| {
        if kit_leptos::rt::closest(&ev, "[data-path], button, a, input, thead").is_none() {
            ctx.clear();
        }
    };
    let open_more = move |ev: leptos::ev::MouseEvent, folder: bool| {
        let (x, y) =
            ev.current_target().and_then(|t| t.dyn_into::<web_sys::Element>().ok()).map(|el| below(&el, true)).unwrap_or((0.0, 0.0));
        let items = if folder {
            ops::folder_menu(&ctx, || click_input(FILE_INPUT), || click_input(FOLDER_INPUT))
        } else {
            ops::item_menu(&ctx, ctx.selection_now())
        };
        ctx.menu.set(Some(MenuState { x, y, items }));
    };
    let upload_menu = move |ev: leptos::ev::MouseEvent| {
        let (x, y) =
            ev.current_target().and_then(|t| t.dyn_into::<web_sys::Element>().ok()).map(|el| below(&el, true)).unwrap_or((0.0, 0.0));
        ctx.menu.set(Some(MenuState {
            x,
            y,
            items: vec![
                MenuItem::new("Upload files", "upload", || click_input(FILE_INPUT)).key("U"),
                MenuItem::new("Upload a folder", "folder", || click_input(FOLDER_INPUT)),
                MenuItem::Separator,
                MenuItem::new("New text file…", "file-text", move || ops::new_text_file(&ctx)),
            ],
        }));
    };

    let empty = Callback::new(move |_| {
        let f = ctx.filter.get();
        if let Load::Failed(msg) = ctx.load.get() {
            return view! {
                <div class="empty">
                    <span inner_html=icon("alert")></span>
                    <h3>"This folder could not be opened"</h3>
                    <p class="muted" data-details="" title=msg.clone()>{msg.clone()}</p>
                    <button type="button" class="btn mt" on:click=move |_| ctx.refresh()><span inner_html=icon("refresh")></span>"Try again"</button>
                </div>
            }
            .into_any();
        }
        if !f.trim().is_empty() {
            return view! {
                <div class="empty compact">
                    <span inner_html=icon("search")></span>
                    <h3>"No matches"</h3>
                    <p class="muted">{format!("Nothing in this folder matches “{}”.", f.trim())}</p>
                    <button type="button" class="btn mt" on:click=move |_| ctx.filter.set(String::new())>"Clear filter"</button>
                </div>
            }
            .into_any();
        }
        let up = ctx.perms().upload;
        view! {
            <div class="empty">
                <span inner_html=icon(if up { "upload" } else { "folder" })></span>
                <h3>"This folder is empty"</h3>
                <p class="muted">{if up { "Drop files or folders here, or use the buttons below." } else { "There is nothing here yet." }}</p>
                {up.then(|| view! {
                    <div class="row center mt">
                        <button type="button" class="btn primary" on:click=|_| click_input(FILE_INPUT)><span inner_html=icon("upload")></span>"Upload files"</button>
                        <button type="button" class="btn" on:click=move |_| ops::new_folder(&ctx)><span inner_html=icon("folder-plus")></span>"New folder"</button>
                    </div>
                })}
            </div>
        }
        .into_any()
    });

    let subtitle = move || match ctx.load.get() {
        Load::Ready(list) => {
            let dirs = list.iter().filter(|e| e.is_dir).count();
            format!("{}, {}", plural(dirs, "folder"), plural(list.len() - dirs, "file"))
        }
        Load::Loading => "Loading…".into(),
        Load::Failed(_) => String::new(),
    };

    view! {
        <section class=move || { if count.get() > 0 { "browser has-selection" } else { "browser" } } data-page="files" aria-label=move || format!("{} {}", ctx.source_name(), path.get())>
            <Show when=move || unknown.get()>
                <div class="error-box">"There is no source with this name, or you can't see it. Pick one from the menu."</div>
            </Show>
            <header class="browser-head">
                <div class="browser-titles">
                    {breadcrumbs(ctx)}
                    <p class="browser-sub small muted">{subtitle}</p>
                </div>
                <div class="browser-actions hide-phone">
                    <Show when=move || !at_root.get()>
                        <button type="button" class="btn icon" title="Up one folder (Backspace)" aria-label="Up one folder" on:click=go_up inner_html=icon("arrow-up")></button>
                    </Show>
                    <Show when=move || perms.get().share && !at_root.get()>
                        <button type="button" class="btn" on:click=move |_| ops::share(&ctx, &[])><span inner_html=icon("share")></span>"Share"</button>
                    </Show>
                    <Show when=move || perms.get().upload>
                        <button type="button" class="btn" title="New folder (N)" on:click=move |_| ops::new_folder(&ctx)><span inner_html=icon("folder-plus")></span>"New folder"</button>
                        <span class="split">
                            <button type="button" class="btn primary" title="Upload files (U)" on:click=|_| click_input(FILE_INPUT)><span inner_html=icon("upload")></span>"Upload"</button>
                            <button type="button" class="btn primary icon" title="More ways to add" aria-label="More ways to add" aria-haspopup="menu" on:click=upload_menu inner_html=icon("chevron")></button>
                        </span>
                    </Show>
                    <button type="button" class="btn icon" title="Folder actions" aria-label="Folder actions" aria-haspopup="menu" on:click=move |ev| open_more(ev, true) inner_html=icon("more")></button>
                </div>
            </header>

            <div class="toolbar" role="toolbar" aria-label=move || { if count.get() > 0 { "Selection actions" } else { "View options" } }>
                {move || if count.get() > 0 {
                    let n = count.get();
                    let zip = n > 1 || one.get().is_some_and(|e| e.is_dir);
                    view! {
                        <div class="selection-bar">
                            <button type="button" class="btn ghost icon sm" title="Clear selection (Esc)" aria-label="Clear selection" on:click=move |_| ctx.clear() inner_html=icon("x")></button>
                            <span class="selection-count" aria-live="polite">{format!("{} selected", plural(n, "item"))}</span>
                            <span class="selection-actions hide-phone">
                                <button type="button" class="btn sm" title="Download" disabled=move || !perms.get().download on:click=move |_| ops::download(&ctx, &ctx.selection_now())><span inner_html=icon("download")></span><span class="sel-label">{if zip { "Download zip" } else { "Download" }}</span></button>
                                <button type="button" class="btn sm" title="Share" disabled=move || !perms.get().share on:click=move |_| ops::share(&ctx, &ctx.selection_now())><span inner_html=icon("share")></span><span class="sel-label">"Share"</span></button>
                                <button type="button" class="btn sm" title="Move" disabled=move || !perms.get().edit on:click=move |_| ops::move_or_copy(&ctx, ctx.selection_now().into_iter().map(|e| e.path).collect(), false)><span inner_html=icon("move")></span><span class="sel-label">"Move"</span></button>
                                <button type="button" class="btn sm" title="Copy" disabled=move || !perms.get().upload on:click=move |_| ops::move_or_copy(&ctx, ctx.selection_now().into_iter().map(|e| e.path).collect(), true)><span inner_html=icon("copy")></span><span class="sel-label">"Copy"</span></button>
                                <button type="button" class="btn sm" title="Rename (F2)" disabled=move || !perms.get().edit || one.get().is_none() on:click=move |_| if let Some(e) = one.get_untracked() { ops::rename(&ctx, &e) }><span inner_html=icon("edit")></span><span class="sel-label">"Rename"</span></button>
                                <button type="button" class="btn sm danger" title="Delete (Del)" disabled=move || !perms.get().delete on:click=move |_| ops::delete(&ctx, ctx.selection_now().into_iter().map(|e| e.path).collect())><span inner_html=icon("trash")></span><span class="sel-label">"Delete"</span></button>
                            </span>
                            <button type="button" class="btn ghost sm" aria-haspopup="menu" title="More actions" on:click=move |ev| open_more(ev, false)><span inner_html=icon("more")></span><span class="hide-phone">"More"</span></button>
                        </div>
                    }.into_any()
                } else {
                    view! {
                        <label class="filter">
                            <span inner_html=icon("filter")></span>
                            <input class="input" type="search" placeholder="Filter this folder" aria-label="Filter this folder" prop:value=move || ctx.filter.get() on:input=move |e| ctx.filter.set(event_target_value(&e))/>
                        </label>
                    }.into_any()
                }}
                <span class="view-controls">
                    <span class="sort-control">
                        <select class="input" aria-label="Sort by" prop:value=move || ctx.sort.get().0.id() on:change=move |e| { let k = SortKey::parse(&event_target_value(&e)); ctx.sort.update(|s| s.0 = k); }>
                            <option value="name">"Name"</option>
                            <option value="size">"Size"</option>
                            <option value="modified">"Modified"</option>
                        </select>
                        <button type="button" class="btn icon" title=move || if ctx.sort.get().1 { "Ascending" } else { "Descending" } aria-label="Sort direction"
                            on:click=move |_| ctx.sort.update(|s| s.1 = !s.1) inner_html=move || icon(if ctx.sort.get().1 { "arrow-up" } else { "arrow-down" })></button>
                    </span>
                    <span class="seg view-switch" role="radiogroup" aria-label="View">
                        {ViewMode::ALL.into_iter().enumerate().map(|(i, m)| view! {
                            <button type="button" role="radio" class:on=move || ctx.mode.get() == m aria-checked=move || (ctx.mode.get() == m).to_string()
                                title=format!("{} ({})", m.label(), i + 1) aria-label=m.label() on:click=move |_| ctx.set_mode(m) inner_html=icon(m.icon())></button>
                        }).collect_view()}
                    </span>
                </span>
            </div>

            <input type="file" id=FILE_INPUT multiple hidden disabled=move || !perms.get().upload on:change=on_files/>
            <input type="file" id=FOLDER_INPUT hidden prop:webkitdirectory=true disabled=move || !perms.get().upload on:change=on_files data-no-drop=""/>

            <div class="listing" data-dropzone=FILE_INPUT on:drop=on_drop on:contextmenu=on_empty_menu on:click=on_empty_click>
                <Listing ctx=ctx empty=empty/>
                <div class="drop-card" aria-hidden="true">
                    <span inner_html=icon("upload")></span>
                    <strong>"Drop to upload"</strong>
                    <span class="small muted">{move || format!("to {}{}", ctx.source_name(), if path.get() == "/" { String::new() } else { format!(" · {}", path.get()) })}</span>
                </div>
            </div>

            <div class="bottom-bar" role="toolbar" aria-label=move || { if count.get() > 0 { "Selection actions" } else { "Folder actions" } }>
                {move || if count.get() > 0 {
                    view! {
                        {bar_button("download", "Download", Signal::derive(move || !perms.get().download), false, move |_| ops::download(&ctx, &ctx.selection_now()))}
                        {bar_button("share", "Share", Signal::derive(move || !perms.get().share), false, move |_| ops::share(&ctx, &ctx.selection_now()))}
                        {bar_button("move", "Move", Signal::derive(move || !perms.get().edit), false, move |_| ops::move_or_copy(&ctx, ctx.selection_now().into_iter().map(|e| e.path).collect(), false))}
                        {bar_button("trash", "Delete", Signal::derive(move || !perms.get().delete), true, move |_| ops::delete(&ctx, ctx.selection_now().into_iter().map(|e| e.path).collect()))}
                        {bar_button("more", "More", Signal::derive(|| false), false, move |ev| open_more(ev, false))}
                    }.into_any()
                } else {
                    view! {
                        {bar_button("arrow-up", "Up", Signal::derive(move || at_root.get()), false, go_up)}
                        {bar_button("upload", "Upload", Signal::derive(move || !perms.get().upload), false, |_| click_input(FILE_INPUT))}
                        {bar_button("folder-plus", "Folder", Signal::derive(move || !perms.get().upload), false, move |_| ops::new_folder(&ctx))}
                        {bar_button("share", "Share", Signal::derive(move || !perms.get().share || at_root.get()), false, move |_| ops::share(&ctx, &[]))}
                        {bar_button("more", "More", Signal::derive(|| false), false, move |ev| open_more(ev, true))}
                    }.into_any()
                }}
            </div>

            <ContextMenu state=ctx.menu/>
            <Preview ctx=ctx/>
        </section>
    }
}
