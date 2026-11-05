//! "Show this file once its folder is open": a starred file, a search hit or a file's own address opens its
//! folder, then selects the file, scrolls to it and (when asked) opens it in the viewer.

use super::state::Ctx;
use leptos::prelude::*;
use std::cell::RefCell;

thread_local! {
    static PENDING: RefCell<Option<(String, bool)>> = const { RefCell::new(None) };
}

/// Shows the entry called `name` once the next folder has loaded (`open` opens it in the viewer too).
pub fn later(name: &str, open: bool) {
    PENDING.with(|p| *p.borrow_mut() = Some((name.to_string(), open)));
}

/// Shows the pending entry, if the folder now on screen holds it.
pub fn apply(ctx: &Ctx) {
    let Some((name, open)) = PENDING.with(|p| p.borrow_mut().take()) else { return };
    let Some(e) = ctx.visible.get_untracked().into_iter().find(|e| e.name == name) else { return };
    ctx.select(std::collections::BTreeSet::from([e.path.clone()]), Some(e.path.clone()));
    let path = e.path.clone();
    kit_leptos::rt::after(50, move || {
        let sel = format!(r#"[data-path="{}"]"#, path.replace('\\', "\\\\").replace('"', "\\\""));
        if let Some(el) = kit_leptos::rt::qs(&sel) {
            el.scroll_into_view_with_bool(false);
        }
    });
    if open && !e.is_dir {
        ctx.viewing.set(Some(e));
    }
}
