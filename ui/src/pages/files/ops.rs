//! What the browser does with files: open, download, star, rename, move, copy, delete (to the trash, with Undo),
//! tags, comments, collections and share. The menus ([`item_menu`], [`folder_menu`]) and every toolbar button
//! call these, so each command behaves the same wherever it is pressed.

use super::api;
use super::dialogs;
use super::menu::MenuItem;
use super::paths::{basename, join, parent, plural};
use super::state::Ctx;
use crate::api::{download_url, Entry};
use crate::route::{self, Route};
use kit_leptos::modal;
use kit_leptos::toasts::{toast, toast_action};
use leptos::prelude::*;
use wasm_bindgen::JsCast;

/// The window event the sharing page listens for to open its share dialog:
/// `filebrary:share` with `detail = {source, paths, single_folder}`.
pub const SHARE_EVENT: &str = "filebrary:share";

/// Opens an entry: a folder in the browser, a file in the viewer.
pub fn open(ctx: &Ctx, e: &Entry) {
    if e.is_dir {
        route::go(Route::Files { source: ctx.source.get_untracked(), path: e.path.clone() }, false);
    } else {
        ctx.viewing.set(Some(e.clone()));
    }
}

/// Starts a download through a hidden link (the browser keeps the session cookie and shows its own progress).
pub fn download_link(url: &str) {
    let Some(doc) = kit_leptos::rt::document() else { return };
    let Ok(a) = doc.create_element("a") else { return };
    let _ = a.set_attribute("href", url);
    let _ = a.set_attribute("download", "");
    if let Some(body) = doc.body() {
        let _ = body.append_child(&a);
        if let Ok(h) = a.clone().dyn_into::<web_sys::HtmlElement>() {
            h.click();
        }
        a.remove();
    }
}

/// Downloads entries: one file as itself, anything else as a zip.
pub fn download(ctx: &Ctx, items: &[Entry]) {
    if items.is_empty() || !ctx.perms().download {
        return;
    }
    let s = ctx.source.get_untracked();
    let url = match items {
        [one] if !one.is_dir => download_url(&s, &one.path, false),
        _ => api::zip_url(&s, &items.iter().map(|e| e.path.clone()).collect::<Vec<_>>()),
    };
    download_link(&url);
}

/// Stars entries (or takes the star off when all of them have one).
pub fn toggle_star(ctx: &Ctx, items: Vec<Entry>) {
    if items.is_empty() {
        return;
    }
    let ctx = *ctx;
    let on = !items.iter().all(|e| ctx.starred(e));
    let s = ctx.source.get_untracked();
    kit_leptos::rt::spawn(async move {
        let mut kept = false;
        for e in &items {
            match crate::kit::set_star(&s, &e.path, on).await {
                Ok(k) => kept |= k,
                Err(err) => {
                    toast(&err.message, "err");
                    return;
                }
            }
        }
        ctx.notes.update(|n| {
            for e in &items {
                n.entry(e.name.clone()).or_default().starred = on;
            }
        });
        let what = if items.len() == 1 { items[0].name.clone() } else { plural(items.len(), "item") };
        let done = if on { format!("Starred {what}") } else { format!("Took the star off {what}") };
        toast(&if kept { format!("{done} – it's sent when the server is back") } else { done }, "ok");
    });
}

/// Runs a change, then says how it went and reloads.
fn run(ctx: &Ctx, done: String, f: impl std::future::Future<Output = Result<(), crate::api::Error>> + 'static) {
    let ctx = *ctx;
    kit_leptos::rt::spawn(async move {
        match f.await {
            Ok(()) => toast(&done, "ok"),
            Err(e) => toast(&e.message, "err"),
        }
        ctx.refresh();
    });
}

/// Asks for a name and makes a folder.
pub fn new_folder(ctx: &Ctx) {
    let ctx = *ctx;
    dialogs::ask_name("New folder", "", "Make folder", move |name| {
        let (s, p) = (ctx.source.get_untracked(), join(&ctx.path.get_untracked(), &name));
        super::reveal::later(&name, false);
        run(&ctx, format!("Made {name}"), async move { api::mkdir(&s, &p).await });
    });
}

/// Asks for a name and opens a new, empty text file in the editor.
pub fn new_text_file(ctx: &Ctx) {
    let ctx = *ctx;
    dialogs::ask_name("New text file", "notes.txt", "Make file", move |name| {
        let (s, p) = (ctx.source.get_untracked(), join(&ctx.path.get_untracked(), &name));
        kit_leptos::rt::spawn(async move {
            match api::save_text(&s, &p, "", None).await {
                Ok(api::Saved::Ok(_)) => {
                    super::reveal::later(&name, true);
                    ctx.refresh();
                }
                Ok(api::Saved::Conflict(_)) => toast(&format!("{name} is already there"), "err"),
                Err(e) => toast(&e.message, "err"),
            }
        });
    });
}

/// Asks for a new name.
pub fn rename(ctx: &Ctx, e: &Entry) {
    let ctx = *ctx;
    let e = e.clone();
    let title = if e.is_dir { "Rename folder" } else { "Rename file" };
    dialogs::ask_name(title, &e.name.clone(), "Rename", move |name| {
        if name == e.name {
            return;
        }
        let (s, from) = (ctx.source.get_untracked(), e.path.clone());
        let to = join(&parent(&from), &name);
        super::reveal::later(&name, false);
        run(&ctx, format!("Renamed to {name}"), async move { api::move_to(&s, &from, &to).await });
    });
}

/// Moves (or copies) paths into a folder picked in a dialog.
pub fn move_or_copy(ctx: &Ctx, paths: Vec<String>, copy: bool) {
    if paths.is_empty() {
        return;
    }
    let ctx = *ctx;
    let title = format!("{} {} to", if copy { "Copy" } else { "Move" }, plural(paths.len(), "item"));
    let ok = if copy { "Copy here" } else { "Move here" };
    dialogs::pick_folder(&title, ctx.source.get_untracked(), ctx.source_name(), ctx.path.get_untracked(), ok, move |dir| {
        move_into(&ctx, paths.clone(), dir, copy);
    });
}

/// Moves (or copies) paths into `dir` of the same source.
pub fn move_into(ctx: &Ctx, paths: Vec<String>, dir: String, copy: bool) {
    let paths: Vec<String> = paths.into_iter().filter(|p| *p != dir && !dir.starts_with(&format!("{p}/"))).collect();
    if paths.is_empty() {
        return;
    }
    if !copy {
        ctx.clear();
    }
    let s = ctx.source.get_untracked();
    let n = paths.len();
    let done = format!("{} {}", if copy { "Copied" } else { "Moved" }, plural(n, "item"));
    run(ctx, done, async move {
        for p in &paths {
            let to = join(&dir, basename(p));
            if to == *p {
                continue;
            }
            if copy {
                api::copy_to(&s, p, &to).await?;
            } else {
                api::move_to(&s, p, &to).await?;
            }
        }
        Ok(())
    });
}

/// The confirm question for deleting `paths`.
pub fn delete_question(paths: &[String]) -> kit_rules::dialogs::Confirm {
    let title = match paths {
        [one] => format!("Delete “{}”?", basename(one)),
        _ => format!("Delete {}?", plural(paths.len(), "item")),
    };
    let mut text = String::from("Deleted items go to the trash, where you can restore them. When the trash is off they are gone for good.");
    if paths.len() > 1 {
        let names: Vec<&str> = paths.iter().take(5).map(|p| basename(p)).collect();
        let more = if paths.len() > 5 { format!(" and {} more", paths.len() - 5) } else { String::new() };
        text = format!("{}{more}. {text}", names.join(", "));
    }
    kit_rules::dialogs::Confirm::new(title).text(text).ok("Delete").danger(true)
}

/// Asks, then deletes paths: into the trash with an Undo toast, or for good when the trash is off.
pub fn delete(ctx: &Ctx, paths: Vec<String>) {
    if paths.is_empty() || !ctx.perms().delete {
        return;
    }
    let ctx = *ctx;
    kit_leptos::rt::spawn(async move {
        if !modal::confirm(delete_question(&paths)).await {
            return;
        }
        ctx.clear();
        let s = ctx.source.get_untracked();
        match api::trash(&s, &paths).await {
            Ok(r) => {
                ctx.refresh();
                let n = r.trashed.len() + r.deleted.len();
                if r.trashed.is_empty() {
                    toast(&format!("Deleted {}", plural(n, "item")), "ok");
                    return;
                }
                let ids: Vec<String> = r.trashed.iter().map(|t| t.id.clone()).collect();
                toast_action(&format!("Moved {} to the trash", plural(r.trashed.len(), "item")), "Undo", 10_000, move || {
                    kit_leptos::rt::spawn(async move {
                        for id in &ids {
                            if let Err(e) = api::restore(id, true).await {
                                toast(&e.message, "err");
                                break;
                            }
                        }
                        ctx.refresh();
                    });
                });
            }
            Err(e) => {
                toast(&e.message, "err");
                ctx.refresh();
            }
        }
    });
}

/// Asks the sharing page to open its share dialog for `paths` (a whole folder when `single_folder`).
pub fn share(ctx: &Ctx, items: &[Entry]) {
    let (paths, single_folder) = if items.is_empty() {
        (vec![ctx.path.get_untracked()], true)
    } else {
        (items.iter().map(|e| e.path.clone()).collect(), items.len() == 1 && items[0].is_dir)
    };
    let detail = serde_json::json!({ "source": ctx.source.get_untracked(), "paths": paths, "single_folder": single_folder });
    let Some(w) = kit_leptos::rt::window() else { return };
    let init = web_sys::CustomEventInit::new();
    if let Ok(d) = js_sys::JSON::parse(&detail.to_string()) {
        init.set_detail(&d);
    }
    if let Ok(ev) = web_sys::CustomEvent::new_with_event_init_dict(SHARE_EVENT, &init) {
        let _ = w.dispatch_event(&ev);
    }
}

/// Tags of one entry.
pub fn tags(ctx: &Ctx, e: &Entry) {
    let ctx = *ctx;
    let current: Vec<String> =
        ctx.notes.with_untracked(|n| n.get(&e.name).map(|a| a.tags.iter().map(|t| t.name.clone()).collect()).unwrap_or_default());
    dialogs::edit_tags(ctx.source.get_untracked(), e.path.clone(), current, move || ctx.refresh());
}

/// Copies an entry's in-app address.
pub fn copy_link(ctx: &Ctx, e: &Entry) {
    let path = route::url(&Route::Files { source: ctx.source.get_untracked(), path: e.path.clone() }.href());
    let origin = kit_leptos::rt::window().and_then(|w| w.location().origin().ok()).unwrap_or_default();
    kit_leptos::clipboard::copy_then(&format!("{origin}{path}"), "Link copied");
}

/// The commands for some entries (the context menu, the ⋮ button and "More").
pub fn item_menu(ctx: &Ctx, items: Vec<Entry>) -> Vec<MenuItem> {
    let p = ctx.perms();
    let c = *ctx;
    let single = (items.len() == 1).then(|| items[0].clone());
    let all_starred = !items.is_empty() && items.iter().all(|e| ctx.starred(e));
    let paths: Vec<String> = items.iter().map(|e| e.path.clone()).collect();
    let zip = items.len() > 1 || single.as_ref().is_some_and(|e| e.is_dir);
    let mut m = Vec::new();
    if let Some(e) = single.clone() {
        let ic = if e.is_dir { "folder" } else { "eye" };
        m.push(MenuItem::new("Open", ic, move || open(&c, &e)).key("Enter"));
    }
    let (i1, i2, i3) = (items.clone(), items.clone(), items.clone());
    m.push(MenuItem::new(if zip { "Download as zip" } else { "Download" }, "download", move || download(&c, &i1)).disabled(!p.download));
    m.push(MenuItem::new("Share…", "share", move || share(&c, &i2)).disabled(!p.share));
    m.push(MenuItem::new(if all_starred { "Remove star" } else { "Star" }, "star", move || toggle_star(&c, i3.clone())).key("S"));
    if let Some(e) = single.clone() {
        let e2 = e.clone();
        m.push(MenuItem::new("Copy link", "copy", move || copy_link(&c, &e2)));
        m.push(MenuItem::new("Tags…", "tag", move || tags(&c, &e)).disabled(!p.edit));
    }
    m.push(MenuItem::Separator);
    if let Some(e) = single.clone() {
        m.push(MenuItem::new("Rename…", "edit", move || rename(&c, &e)).key("F2").disabled(!p.edit));
    }
    let (p1, p2, p3, p4) = (paths.clone(), paths.clone(), paths.clone(), paths.clone());
    m.push(MenuItem::new("Move to…", "move", move || move_or_copy(&c, p1.clone(), false)).disabled(!p.edit));
    m.push(MenuItem::new("Copy to…", "copy", move || move_or_copy(&c, p2.clone(), true)).disabled(!p.upload));
    m.push(MenuItem::new("Add to collection…", "collection", move || dialogs::add_to_collection(c.source.get_untracked(), p3.clone())));
    if let Some(e) = single {
        m.push(MenuItem::new("Comments…", "comment", move || dialogs::comments(c.source.get_untracked(), e.path.clone())));
    }
    m.push(MenuItem::Separator);
    m.push(MenuItem::new("Delete…", "trash", move || delete(&c, p4.clone())).key("Del").danger().disabled(!p.delete));
    m
}

/// The commands for the folder itself (right-click on empty space, the phone's "More").
pub fn folder_menu(ctx: &Ctx, pick_files: impl Fn() + 'static, pick_folder: impl Fn() + 'static) -> Vec<MenuItem> {
    let p = ctx.perms();
    let c = *ctx;
    let root = ctx.path.get_untracked() == "/";
    let all = ctx.visible.get_untracked();
    let none = all.is_empty();
    vec![
        MenuItem::new("Upload files", "upload", pick_files).disabled(!p.upload),
        MenuItem::new("Upload a folder", "folder", pick_folder).disabled(!p.upload),
        MenuItem::new("New folder…", "folder-plus", move || new_folder(&c)).disabled(!p.upload),
        MenuItem::new("New text file…", "file-text", move || new_text_file(&c)).disabled(!p.upload),
        MenuItem::Separator,
        MenuItem::new("Share this folder…", "share", move || share(&c, &[])).disabled(!p.share || root),
        MenuItem::new("Download folder as zip", "download", move || {
            let s = c.source.get_untracked();
            download_link(&api::zip_url(&s, &[c.path.get_untracked()]));
        })
        .disabled(!p.download),
        MenuItem::new("Select all", "check", move || c.select(all.iter().map(|e| e.path.clone()).collect(), None))
            .key("Ctrl A")
            .disabled(none),
        MenuItem::new("Refresh", "refresh", move || c.refresh()),
        MenuItem::Separator,
        MenuItem::new("Trash…", "trash", move || {
            dialogs::trash(c.source.get_untracked(), c.source_name(), c.perms().delete, move || c.refresh())
        }),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delete_asks_with_names() {
        let q = delete_question(&["/a/b.txt".into()]);
        assert_eq!(q.title, "Delete “b.txt”?");
        assert!(q.danger && q.ok == "Delete");
        let many: Vec<String> = (0..7).map(|i| format!("/f{i}")).collect();
        let q = delete_question(&many);
        assert_eq!(q.title, "Delete 7 items?");
        assert!(q.text.starts_with("f0, f1, f2, f3, f4 and 2 more."));
    }
}
