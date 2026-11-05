//! The listing: a details table, a compact list or tiles of three sizes, with the same selection, open,
//! context-menu, long-press and drag-to-move behaviour in each.
//!
//! Selection: a click selects one item, Ctrl/Cmd+click toggles, Shift+click selects a range, checkboxes toggle.
//! On touch screens a tap opens unless a selection is under way, then it toggles; a long press opens the menu.
//! Items can be dragged onto a folder (or a breadcrumb) to move them.

use super::kinds::{file_icon, wants_thumbnail, ViewMode};
use super::menu::MenuState;
use super::ops;
use super::paths::{format_date, format_size, next_selection, Mods, SortKey};
use super::state::{Ctx, Load};
use crate::api::{preview_url, Entry};
use crate::icons::icon;
use leptos::prelude::*;
use std::collections::BTreeSet;
use wasm_bindgen::JsCast;

/// The drag type of items dragged inside the browser (their paths as JSON).
pub const DRAG_TYPE: &str = "application/x-filebrary-paths";

thread_local! {
    static POINTER: std::cell::RefCell<String> = std::cell::RefCell::new("mouse".into());
    static PRESS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

fn touch() -> bool {
    POINTER.with(|p| p.borrow().as_str() == "touch")
}

/// A click on an entry: select (or open, for a tap with nothing selected).
fn click(ctx: &Ctx, e: &Entry, ev: &web_sys::MouseEvent) {
    if touch() && ctx.selected.get_untracked().is_empty() {
        return ops::open(ctx, e);
    }
    let order: Vec<String> = ctx.visible.get_untracked().into_iter().map(|x| x.path).collect();
    let (s, a) = next_selection(
        &order,
        &ctx.selected.get_untracked(),
        ctx.anchor.get_untracked().as_deref(),
        &e.path,
        Mods { shift: ev.shift_key(), toggle: ev.ctrl_key() || ev.meta_key() || touch() },
    );
    ctx.select(s, a);
}

fn toggle(ctx: &Ctx, e: &Entry) {
    let order: Vec<String> = ctx.visible.get_untracked().into_iter().map(|x| x.path).collect();
    let (s, a) = next_selection(
        &order,
        &ctx.selected.get_untracked(),
        ctx.anchor.get_untracked().as_deref(),
        &e.path,
        Mods { shift: false, toggle: true },
    );
    ctx.select(s, a);
}

/// The items a menu on `e` acts on: the selection when `e` is in it, else `e` alone (and it becomes the
/// selection).
pub fn menu_items(ctx: &Ctx, e: &Entry) -> Vec<Entry> {
    if ctx.selected.get_untracked().contains(&e.path) {
        ctx.selection_now()
    } else {
        ctx.select(BTreeSet::from([e.path.clone()]), Some(e.path.clone()));
        vec![e.clone()]
    }
}

/// Opens the item menu at (`x`, `y`).
pub fn open_menu(ctx: &Ctx, e: &Entry, x: f64, y: f64) {
    let items = menu_items(ctx, e);
    ctx.menu.set(Some(MenuState { x, y, items: ops::item_menu(ctx, items) }));
}

/// The pointer handlers every item shares: remember mouse or touch, and open the menu after a long press.
fn on_pointer_down(ctx: Ctx, e: Entry) -> impl Fn(leptos::ev::PointerEvent) + 'static {
    move |ev: leptos::ev::PointerEvent| {
        POINTER.with(|p| *p.borrow_mut() = ev.pointer_type());
        if ev.pointer_type() != "touch" {
            return;
        }
        let n = PRESS.with(|p| {
            p.set(p.get() + 1);
            p.get()
        });
        let (x, y) = (ev.client_x() as f64, ev.client_y() as f64);
        let e = e.clone();
        kit_leptos::rt::after(500, move || {
            if PRESS.with(|p| p.get()) == n {
                PRESS.with(|p| p.set(p.get() + 1));
                open_menu(&ctx, &e, x, y);
            }
        });
    }
}

fn cancel_press() {
    PRESS.with(|p| p.set(p.get() + 1));
}

/// Drag start: the dragged paths (the selection when the item is in it).
fn on_drag_start(ctx: Ctx, e: Entry) -> impl Fn(leptos::ev::DragEvent) + 'static {
    move |ev: leptos::ev::DragEvent| {
        cancel_press();
        let paths: Vec<String> = if ctx.selected.get_untracked().contains(&e.path) {
            ctx.selection_now().into_iter().map(|x| x.path).collect()
        } else {
            vec![e.path.clone()]
        };
        if let Some(dt) = ev.data_transfer() {
            let _ = dt.set_data(DRAG_TYPE, &serde_json::to_string(&paths).unwrap_or_default());
            let _ = dt.set_data("text/plain", &paths.join("\n"));
            dt.set_effect_allowed("move");
        }
    }
}

/// Whether a drag carries items of this browser.
pub fn carries_items(ev: &leptos::ev::DragEvent) -> bool {
    let Some(dt) = ev.data_transfer() else { return false };
    let types = js_sys::Reflect::get(dt.as_ref(), &"types".into()).unwrap_or_default();
    js_sys::Array::from(&types).iter().any(|t| t.as_string().as_deref() == Some(DRAG_TYPE))
}

/// Drop handlers that move dragged items into `dir`.
pub fn drop_target(
    ctx: Ctx,
    dir: String,
) -> (impl Fn(leptos::ev::DragEvent) + Clone + 'static, impl Fn(leptos::ev::DragEvent) + Clone + 'static) {
    let over = move |ev: leptos::ev::DragEvent| {
        if carries_items(&ev) && ctx.perms().edit {
            ev.prevent_default();
            if let Some(dt) = ev.data_transfer() {
                dt.set_drop_effect("move");
            }
            if let Some(el) = ev.current_target().and_then(|t| t.dyn_into::<web_sys::Element>().ok()) {
                let _ = el.class_list().add_1("drag-over");
            }
        }
    };
    let drop = move |ev: leptos::ev::DragEvent| {
        if let Some(el) = ev.current_target().and_then(|t| t.dyn_into::<web_sys::Element>().ok()) {
            let _ = el.class_list().remove_1("drag-over");
        }
        if !carries_items(&ev) {
            return;
        }
        ev.prevent_default();
        ev.stop_propagation();
        let raw = ev.data_transfer().and_then(|dt| dt.get_data(DRAG_TYPE).ok()).unwrap_or_default();
        let paths: Vec<String> = serde_json::from_str(&raw).unwrap_or_default();
        ops::move_into(&ctx, paths, dir.clone(), false);
    };
    (over, drop)
}

fn leave(ev: leptos::ev::DragEvent) {
    if let Some(el) = ev.current_target().and_then(|t| t.dyn_into::<web_sys::Element>().ok()) {
        let _ = el.class_list().remove_1("drag-over");
    }
}

/// The star and tags shown after a name.
fn marks(ctx: Ctx, name: String) -> impl IntoView {
    move || {
        ctx.notes.with(|n| {
            n.get(&name).map(|a| {
                let star = a.starred.then(|| view! { <span class="star-mark" title="Starred" inner_html=icon("star-filled")></span> });
                let tags = a
                    .tags
                    .iter()
                    .map(|t| {
                        let style = t.color.as_ref().map(|c| format!("--tag:{c}")).unwrap_or_default();
                        view! { <span class="tag" style=style>{t.name.clone()}</span> }
                    })
                    .collect_view();
                view! { {star}{tags} }
            })
        })
    }
}

fn tooltip(e: &Entry) -> String {
    if e.is_dir {
        format!("{}\n{}", e.name, format_date(&e.modified))
    } else {
        format!("{}\n{} · {}", e.name, format_size(e.size), format_date(&e.modified))
    }
}

/// A column header that sorts.
fn sort_header(ctx: Ctx, key: SortKey, label: &'static str, class: &'static str) -> impl IntoView {
    let aria = move || {
        let (k, asc) = ctx.sort.get();
        if k == key {
            if asc {
                "ascending"
            } else {
                "descending"
            }
        } else {
            "none"
        }
    };
    view! {
        <th class=class aria-sort=aria>
            <button type="button" class="th-sort" on:click=move |_| ctx.sort.update(|(k, asc)| { *asc = if *k == key { !*asc } else { true }; *k = key; })>
                {label}
                {move || { let (k, asc) = ctx.sort.get(); (k == key).then(|| view! { <span inner_html=icon(if asc { "arrow-up" } else { "arrow-down" })></span> }) }}
            </button>
        </th>
    }
}

/// One row of the details table.
fn row(ctx: Ctx, e: Entry) -> impl IntoView {
    let path = e.path.clone();
    let on = Memo::new(move |_| ctx.selected.with(|s| s.contains(&path)));
    let (e1, e2, e3, e4, e5, e6) = (e.clone(), e.clone(), e.clone(), e.clone(), e.clone(), e.clone());
    let can_drag = move || ctx.perms().edit;
    let (over, drop) = drop_target(ctx, e.path.clone());
    let is_dir = e.is_dir;
    let name_view = if e.is_dir {
        let to = crate::route::Route::Files { source: ctx.source.get_untracked(), path: e.path.clone() };
        let href = crate::route::url(&to.href());
        let e7 = e.clone();
        view! {
            <a class="name-text" href=href on:click=move |ev: leptos::ev::MouseEvent| {
                ev.prevent_default();
                if ev.shift_key() || ev.ctrl_key() || ev.meta_key() { return; }
                ev.stop_propagation();
                ops::open(&ctx, &e7);
            }>{e.name.clone()}</a>
        }
        .into_any()
    } else {
        view! { <span class="name-text">{e.name.clone()}</span> }.into_any()
    };
    let meta = if e.is_dir { format_date(&e.modified) } else { format!("{} · {}", format_size(e.size), format_date(&e.modified)) };
    view! {
        <tr data-path=e.path.clone() class:selected=on aria-selected=move || on.get().to_string() draggable=move || can_drag().to_string()
            on:click=move |ev| click(&ctx, &e1, &ev)
            on:dblclick=move |_| ops::open(&ctx, &e2)
            on:contextmenu=move |ev: leptos::ev::MouseEvent| { ev.prevent_default(); ev.stop_propagation(); open_menu(&ctx, &e3, ev.client_x() as f64, ev.client_y() as f64); }
            on:pointerdown=on_pointer_down(ctx, e4)
            on:pointerup=|_| cancel_press() on:pointermove=|_| cancel_press() on:pointercancel=|_| cancel_press()
            on:dragstart=on_drag_start(ctx, e5)
            on:dragover=move |ev| if is_dir { over(ev) }
            on:dragleave=leave
            on:drop=move |ev| if is_dir { drop(ev) }>
            <td class="check" on:click=|ev| ev.stop_propagation()>
                <input type="checkbox" prop:checked=on on:change=move |_| toggle(&ctx, &e6) aria-label=format!("Select {}", e.name)/>
            </td>
            <td class="name">
                <span class="name-cell">
                    <span class="file-icon" class:dir=e.is_dir inner_html=icon(file_icon(&e.name, e.is_dir))></span>
                    {name_view}
                    {marks(ctx, e.name.clone())}
                </span>
                <span class="name-meta">{meta}</span>
            </td>
            <td class="size">{if e.is_dir { "—".to_string() } else { format_size(e.size) }}</td>
            <td class="date">{format_date(&e.modified)}</td>
            <td class="actions" on:click=|ev| ev.stop_propagation()>
                <button type="button" class="btn ghost icon sm row-menu" aria-label=format!("Actions for {}", e.name) title="Actions" aria-haspopup="menu"
                    on:click={
                        let e = e.clone();
                        move |ev: leptos::ev::MouseEvent| {
                            let (x, y) = ev.current_target().and_then(|t| t.dyn_into::<web_sys::Element>().ok()).map(|el| super::menu::below(&el, true)).unwrap_or((0.0, 0.0));
                            open_menu(&ctx, &e, x, y);
                        }
                    }
                    inner_html=icon("more-v")></button>
            </td>
        </tr>
    }
}

/// The details table.
fn details(ctx: Ctx, empty: impl Fn() -> AnyView + Send + Sync + 'static) -> impl IntoView {
    let count = move || ctx.selected.with(|s| s.len());
    let total = move || ctx.visible.with(|v| v.len());
    let all = move || total() > 0 && count() == total();
    let some = move || count() > 0 && count() < total();
    view! {
        <div class="table-wrap">
            <table class="files">
                <thead>
                    <tr>
                        <th class="check">
                            <input type="checkbox" aria-label="Select all" prop:checked=all prop:indeterminate=some
                                on:change=move |ev| { if event_target_checked(&ev) { ctx.select(ctx.visible.get_untracked().into_iter().map(|e| e.path).collect(), None) } else { ctx.clear() } }/>
                        </th>
                        {sort_header(ctx, SortKey::Name, "Name", "name")}
                        {sort_header(ctx, SortKey::Size, "Size", "size")}
                        {sort_header(ctx, SortKey::Modified, "Modified", "date")}
                        <th class="actions"><span class="sr-only">"Actions"</span></th>
                    </tr>
                </thead>
                <tbody>
                    {move || match ctx.load.get() {
                        Load::Loading => view! { <tr class="placeholder"><td colspan="5">{skeleton_rows(7)}</td></tr> }.into_any(),
                        _ if ctx.visible.with(|v| v.is_empty()) => view! { <tr class="placeholder"><td colspan="5">{empty()}</td></tr> }.into_any(),
                        _ => view! { <For each=move || ctx.visible.get() key=|e| (e.path.clone(), e.modified.clone(), e.size) children=move |e| row(ctx, e)/> }.into_any(),
                    }}
                </tbody>
            </table>
        </div>
    }
}

/// Grey rows while a folder loads.
pub fn skeleton_rows(n: usize) -> impl IntoView {
    view! { <div class="stack skel-rows">{(0..n).map(|_| view! { <div class="skeleton" style="height:34px"></div> }).collect_view()}</div> }
}

/// A tile's picture: the server's thumbnail, or the type icon when there is none or it fails.
fn thumb(ctx: Ctx, e: &Entry, grid: bool) -> impl IntoView {
    let show = grid && wants_thumbnail(e, ctx.caps.get_untracked().as_ref());
    let failed = RwSignal::new(false);
    let ic = icon(file_icon(&e.name, e.is_dir));
    let src = preview_url(&ctx.source.get_untracked(), &e.path, "thumb", &e.modified);
    let is_dir = e.is_dir;
    let ic2 = ic.clone();
    view! {
        {if grid {
            view! {
                <span class="thumb" class:dir=is_dir aria-hidden="true">
                    {move || if show && !failed.get() {
                        view! { <img src=src.clone() alt="" loading="lazy" decoding="async" draggable="false" on:error=move |_| failed.set(true)/> }.into_any()
                    } else {
                        view! { <span class="thumb-icon" inner_html=ic.clone()></span> }.into_any()
                    }}
                </span>
            }.into_any()
        } else {
            view! { <span class="file-icon" class:dir=is_dir inner_html=ic2></span> }.into_any()
        }}
    }
}

/// One tile (or compact line).
fn tile(ctx: Ctx, e: Entry, mode: ViewMode) -> impl IntoView {
    let path = e.path.clone();
    let on = Memo::new(move |_| ctx.selected.with(|s| s.contains(&path)));
    let (e1, e2, e3, e4, e5, e6, e7) = (e.clone(), e.clone(), e.clone(), e.clone(), e.clone(), e.clone(), e.clone());
    let (over, drop) = drop_target(ctx, e.path.clone());
    let is_dir = e.is_dir;
    let meta = if e.is_dir { "Folder".to_string() } else { format_size(e.size) };
    view! {
        <li role="option" data-path=e.path.clone() class:selected=on aria-selected=move || on.get().to_string() title=tooltip(&e)
            draggable=move || ctx.perms().edit.to_string()
            on:click=move |ev| click(&ctx, &e1, &ev)
            on:dblclick=move |_| ops::open(&ctx, &e2)
            on:contextmenu=move |ev: leptos::ev::MouseEvent| { ev.prevent_default(); ev.stop_propagation(); open_menu(&ctx, &e3, ev.client_x() as f64, ev.client_y() as f64); }
            on:pointerdown=on_pointer_down(ctx, e4)
            on:pointerup=|_| cancel_press() on:pointermove=|_| cancel_press() on:pointercancel=|_| cancel_press()
            on:dragstart=on_drag_start(ctx, e5)
            on:dragover=move |ev| if is_dir { over(ev) }
            on:dragleave=leave
            on:drop=move |ev| if is_dir { drop(ev) }>
            <input type="checkbox" class="tile-check" prop:checked=on on:click=|ev| ev.stop_propagation() on:change=move |_| toggle(&ctx, &e6) aria-label=format!("Select {}", e.name)/>
            {thumb(ctx, &e, mode.is_grid())}
            <span class="tile-text">
                <span class="tile-name">{e.name.clone()}</span>
                {(mode != ViewMode::Small).then(|| view! { <span class="tile-meta">{meta}</span> })}
            </span>
            {move || ctx.starred(&e7).then(|| view! { <span class="star-mark" title="Starred" inner_html=icon("star-filled")></span> })}
        </li>
    }
}

/// The compact list or the tiles.
fn grid(ctx: Ctx, mode: ViewMode, empty: impl Fn() -> AnyView + Send + Sync + 'static) -> impl IntoView {
    let class = if mode.is_grid() { format!("file-grid {}", mode.id()) } else { "file-compact".to_string() };
    view! {
        <div class="grid-wrap">
            {move || match ctx.load.get() {
                Load::Loading if mode.is_grid() => view! { <div class=format!("file-grid {}", mode.id())>{(0..12).map(|_| view! { <div class="skeleton tile-skel"></div> }).collect_view()}</div> }.into_any(),
                Load::Loading => skeleton_rows(6).into_any(),
                _ if ctx.visible.with(|v| v.is_empty()) => empty(),
                _ => {
                    let class = class.clone();
                    view! {
                        <ul class=class role="listbox" aria-multiselectable="true" aria-label="Files">
                            <For each=move || ctx.visible.get() key=|e| (e.path.clone(), e.modified.clone(), e.size) children=move |e| tile(ctx, e, mode)/>
                        </ul>
                    }.into_any()
                }
            }}
        </div>
    }
}

/// The listing in the current mode; `empty` draws the empty folder.
#[component]
pub fn Listing(ctx: Ctx, #[prop(into)] empty: Callback<(), AnyView>) -> impl IntoView {
    move || {
        let m = ctx.mode.get();
        let empty = move || empty.run(());
        if m == ViewMode::Details {
            details(ctx, empty).into_any()
        } else {
            grid(ctx, m, empty).into_any()
        }
    }
}
