//! The browser's dialogs, all in the kit's modal: a name (new folder, rename, new text file), a folder picker
//! (move and copy), tags, comments, "Add to collection" and the trash.

use super::api::{self, Comment, TrashItem};
use super::icons::icon;
use super::paths::{crumbs, format_size, format_unix, join, name_problem, plural, stem_len};
use kit_leptos::modal::{self, Modal, ModalOpts};
use kit_leptos::toasts::toast;
use leptos::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;
use wasm_bindgen::JsCast;

thread_local! {
    /// Open dialogs, each with the root owner its view lives under.
    static OPEN: RefCell<Vec<(u64, Modal, Owner)>> = const { RefCell::new(Vec::new()) };
    static NEXT: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Closes the dialog it was handed to (a number, so views can hold it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Closer(u64);

impl Closer {
    /// Closes the dialog.
    pub fn close(&self) {
        let m = OPEN.with(|o| {
            let mut o = o.borrow_mut();
            let i = o.iter().position(|(k, _, _)| *k == self.0)?;
            Some(o.remove(i))
        });
        if let Some((_, m, owner)) = m {
            m.close();
            drop_later(owner);
        }
    }
}

/// Opens a kit modal titled `title` with the view `body` makes (given the [`Closer`]).
pub fn open<V: IntoView + 'static>(title: &str, wide: bool, body: impl FnOnce(Closer) -> V + 'static) -> Closer {
    let c = Closer(NEXT.with(|n| {
        n.set(n.get() + 1);
        n.get()
    }));
    // The modal's view would otherwise belong to whatever scope is current (a context menu that closes right
    // after, say), and die with it: resources would never load. Its own root lives until the dialog closes.
    let owner = Owner::new_root(None);
    let m = owner.with(|| modal::open(ModalOpts::with_view(title, move || body(c)).wide(wide)));
    let key = c.0;
    m.on_close(move || {
        let gone = OPEN.with(|o| {
            let mut o = o.borrow_mut();
            let i = o.iter().position(|(k, _, _)| *k == key)?;
            Some(o.remove(i).2)
        });
        if let Some(owner) = gone {
            drop_later(owner);
        }
    });
    OPEN.with(|o| o.borrow_mut().push((key, m, owner)));
    c
}

/// Drops a dialog's owner once its view is out of the page.
fn drop_later(owner: Owner) {
    kit_leptos::rt::after(0, move || drop(owner));
}

/// Focuses the first field of the open dialog (and selects a file name without its extension).
fn focus_first(select_stem: bool) {
    kit_leptos::rt::after(30, move || {
        let Some(el) = kit_leptos::rt::qs(".modal input:not([type=checkbox]), .modal textarea") else { return };
        let Ok(input) = el.dyn_into::<web_sys::HtmlInputElement>() else { return };
        let _ = input.focus();
        if select_stem {
            let v = input.value();
            let _ = input.set_selection_range(0, stem_len(&v) as u32);
        }
    });
}

/// Asks for a name; `on_save` gets the trimmed name when it is usable.
pub fn ask_name(title: &str, initial: &str, ok: &'static str, on_save: impl Fn(String) + 'static) {
    let initial = initial.to_string();
    let select_stem = !initial.is_empty();
    open(title, false, move |c| {
        let name = RwSignal::new(initial.clone());
        let touched = RwSignal::new(false);
        let problem = Memo::new(move |_| name_problem(&name.get()));
        let on_save = Rc::new(on_save);
        let c2 = c;
        let submit = move |e: leptos::ev::SubmitEvent| {
            e.prevent_default();
            touched.set(true);
            if problem.get_untracked().is_none() {
                c2.close();
                on_save(name.get_untracked().trim().to_string());
            }
        };
        view! {
            <form class="stack" on:submit=submit>
                <label class="field">
                    <span>"Name"</span>
                    <input class="input" prop:value=name on:input=move |e| { name.set(event_target_value(&e)); touched.set(true); } autocomplete="off" spellcheck="false"/>
                </label>
                <p class="small field-err" role="alert">{move || if touched.get() { problem.get().unwrap_or("") } else { "" }}</p>
                <div class="dlg-actions">
                    <button type="button" class="btn" on:click=move |_| c.close()>"Cancel"</button>
                    <button type="submit" class="btn primary" disabled=move || problem.get().is_some()>{ok}</button>
                </div>
            </form>
        }
    });
    focus_first(select_stem);
}

/// Picks a folder of `source` (move and copy); starts in `start`.
pub fn pick_folder(title: &str, source: String, source_name: String, start: String, ok: &'static str, on_pick: impl Fn(String) + 'static) {
    open(title, true, move |c| {
        let dir = RwSignal::new(start.clone());
        let typed = RwSignal::new(start.clone());
        let list = LocalResource::new(move || {
            let (s, d) = (source.clone(), dir.get());
            async move {
                api::list(&s, &d).await.map(|l| {
                    let mut v: Vec<_> = l.entries.into_iter().filter(|e| e.is_dir).collect();
                    v.sort_by(|a, b| super::paths::natural_cmp(&a.name, &b.name));
                    v
                })
            }
        });
        let go = move |p: String| {
            dir.set(p.clone());
            typed.set(p);
        };
        let on_pick = Rc::new(on_pick);
        let c2 = c;
        let submit = move |e: leptos::ev::SubmitEvent| {
            e.prevent_default();
            c2.close();
            on_pick(join("/", &typed.get_untracked()));
        };
        let sname = source_name.clone();
        view! {
            <form class="stack" on:submit=submit>
                <nav class="crumbs small" aria-label="Folder path">
                    {move || {
                        let trail = crumbs(&dir.get());
                        let last = trail.len() - 1;
                        trail
                            .into_iter()
                            .enumerate()
                            .map(|(i, (name, p))| {
                                let label = if i == 0 { sname.clone() } else { name };
                                view! {
                                    {(i > 0).then(|| view! { <span class="crumb-sep" inner_html=icon("chevron-right")></span> })}
                                    <button type="button" class="crumb" class:current=i == last on:click=move |_| go(p.clone())>{label}</button>
                                }
                            })
                            .collect_view()
                    }}
                </nav>
                <ul class="picker-list" aria-label="Subfolders">
                    {move || match list.get() {
                        None => view! { <li><div class="skeleton" style="height:36px"></div></li> }.into_any(),
                        Some(Err(e)) => view! { <li class="error-box">{e.message}</li> }.into_any(),
                        Some(Ok(v)) if v.is_empty() => view! { <li class="muted small picker-empty">"No folders here. Pick this one, or type a new path below."</li> }.into_any(),
                        Some(Ok(v)) => v
                            .into_iter()
                            .map(|d| {
                                let p = d.path.clone();
                                view! { <li><button type="button" on:click=move |_| go(p.clone())><span inner_html=icon("folder")></span><span class="ellipsis">{d.name}</span></button></li> }
                            })
                            .collect_view()
                            .into_any(),
                    }}
                </ul>
                <label class="field">
                    <span>"Destination folder"</span>
                    <input class="input mono" prop:value=typed on:input=move |e| typed.set(event_target_value(&e)) spellcheck="false"/>
                </label>
                <div class="dlg-actions">
                    <button type="button" class="btn" on:click=move |_| c.close()>"Cancel"</button>
                    <button type="submit" class="btn primary">{ok}</button>
                </div>
            </form>
        }
    });
}

/// Edits the tags of one path; `done` runs after a save.
pub fn edit_tags(source: String, path: String, current: Vec<String>, done: impl Fn() + 'static) {
    let name = super::paths::basename(&path).to_string();
    open(&format!("Tags of {name}"), false, move |c| {
        let tags = RwSignal::new(current.clone());
        let draft = RwSignal::new(String::new());
        let known = LocalResource::new(|| async { api::all_tags().await.unwrap_or_default() });
        let add = move |t: String| {
            let t = t.trim().to_string();
            if !t.is_empty() {
                tags.update(|v| {
                    if !v.iter().any(|x| x.eq_ignore_ascii_case(&t)) {
                        v.push(t)
                    }
                });
            }
            draft.set(String::new());
        };
        let done = Rc::new(done);
        let c2 = c;
        let save = move |e: leptos::ev::SubmitEvent| {
            e.prevent_default();
            let d = draft.get_untracked();
            if !d.trim().is_empty() {
                add(d);
                return;
            }
            let (s, p, list, done, c) = (source.clone(), path.clone(), tags.get_untracked(), done.clone(), c2);
            kit_leptos::rt::spawn(async move {
                match api::set_tags(&s, &p, &list).await {
                    Ok(_) => {
                        c.close();
                        toast(if list.is_empty() { "Tags removed" } else { "Tags saved" }, "ok");
                        done();
                    }
                    Err(e) => toast(&e.message, "err"),
                }
            });
        };
        view! {
            <form class="stack" on:submit=save>
                <div class="tag-list">
                    {move || tags.get().into_iter().map(|t| {
                        let t2 = t.clone();
                        view! { <span class="tag">{t}<button type="button" class="tag-x" title="Remove tag" aria-label=format!("Remove {t2}") on:click=move |_| tags.update(|v| v.retain(|x| *x != t2)) inner_html=icon("x")></button></span> }
                    }).collect_view()}
                    {move || tags.get().is_empty().then(|| view! { <span class="muted small">"No tags yet."</span> })}
                </div>
                <label class="field">
                    <span>"Add a tag"</span>
                    <input class="input" list="known-tags" placeholder="Type and press Enter" prop:value=draft on:input=move |e| draft.set(event_target_value(&e)) autocomplete="off"/>
                    <datalist id="known-tags">
                        {move || known.get().unwrap_or_default().into_iter().map(|t| view! { <option value=t.name></option> }).collect_view()}
                    </datalist>
                </label>
                <div class="dlg-actions">
                    <button type="button" class="btn" on:click=move |_| c.close()>"Cancel"</button>
                    <button type="submit" class="btn primary">"Save"</button>
                </div>
            </form>
        }
    });
    focus_first(false);
}

/// The comments on one path: read, add, edit and delete.
pub fn comments(source: String, path: String) {
    let name = super::paths::basename(&path).to_string();
    open(&format!("Comments on {name}"), false, move |_c| {
        let rev = RwSignal::new(0u64);
        let (s1, p1) = (source.clone(), path.clone());
        let list = LocalResource::new(move || {
            rev.track();
            let (s, p) = (s1.clone(), p1.clone());
            async move { api::comments(&s, &p).await }
        });
        let draft = RwSignal::new(String::new());
        let editing = RwSignal::new(None::<(i64, String)>);
        let busy = RwSignal::new(false);
        let post = move |e: leptos::ev::SubmitEvent| {
            e.prevent_default();
            let body = draft.get_untracked();
            if body.trim().is_empty() {
                return;
            }
            let (s, p) = (source.clone(), path.clone());
            busy.set(true);
            kit_leptos::rt::spawn(async move {
                match api::add_comment(&s, &p, body.trim()).await {
                    Ok(()) => {
                        draft.set(String::new());
                        rev.update(|n| *n += 1);
                    }
                    Err(e) => toast(&e.message, "err"),
                }
                busy.set(false);
            });
        };
        let row = move |c: Comment| {
            let id = c.id;
            let body = c.body.clone();
            let when = format_unix(c.created_at) + if c.edited_at.is_some() { " · edited" } else { "" };
            view! {
                <li class="comment">
                    <div class="row between small">
                        <strong>{c.username.clone().unwrap_or_else(|| "Deleted account".into())}</strong>
                        <span class="faint">{when}</span>
                    </div>
                    {move || match editing.get() {
                        Some((eid, text)) if eid == id => view! {
                            <div class="stack">
                                <textarea class="input" rows="3" prop:value=text on:input=move |e| editing.set(Some((id, event_target_value(&e))))></textarea>
                                <div class="dlg-actions">
                                    <button type="button" class="btn sm" on:click=move |_| editing.set(None)>"Cancel"</button>
                                    <button type="button" class="btn sm primary" on:click=move |_| {
                                        let Some((_, t)) = editing.get_untracked() else { return };
                                        kit_leptos::rt::spawn(async move {
                                            match api::edit_comment(id, t.trim()).await {
                                                Ok(()) => { editing.set(None); rev.update(|n| *n += 1); }
                                                Err(e) => toast(&e.message, "err"),
                                            }
                                        });
                                    }>"Save"</button>
                                </div>
                            </div>
                        }.into_any(),
                        _ => view! { <p class="comment-body">{body.clone()}</p> }.into_any(),
                    }}
                    <div class="row small">
                        {c.can_edit.then(|| { let b = c.body.clone(); view! { <button type="button" class="btn ghost sm" on:click=move |_| editing.set(Some((id, b.clone())))>"Edit"</button> } })}
                        {c.can_delete.then(|| view! { <button type="button" class="btn ghost sm danger-text" on:click=move |_| {
                            kit_leptos::rt::spawn(async move {
                                if !modal::confirm_dialog("Delete this comment?", "It is gone for good.", "Delete", true).await { return; }
                                match api::delete_comment(id).await {
                                    Ok(()) => rev.update(|n| *n += 1),
                                    Err(e) => toast(&e.message, "err"),
                                }
                            });
                        }>"Delete"</button> })}
                    </div>
                </li>
            }
        };
        view! {
            <div class="stack">
                <ul class="comments">
                    {move || match list.get() {
                        None => view! { <li><div class="skeleton" style="height:48px"></div></li> }.into_any(),
                        Some(Err(e)) => view! { <li class="error-box">{e.message}</li> }.into_any(),
                        Some(Ok(v)) if v.is_empty() => view! { <li class="muted small">"No comments yet."</li> }.into_any(),
                        Some(Ok(v)) => v.into_iter().map(row).collect_view().into_any(),
                    }}
                </ul>
                <form class="stack" on:submit=post>
                    <textarea class="input" rows="3" placeholder="Write a comment" prop:value=draft on:input=move |e| draft.set(event_target_value(&e))></textarea>
                    <div class="dlg-actions"><button type="submit" class="btn primary" disabled=move || busy.get() || draft.get().trim().is_empty()>"Post"</button></div>
                </form>
            </div>
        }
    });
}

/// Adds paths to a collection (an existing one, or a new one named here).
pub fn add_to_collection(source: String, paths: Vec<String>) {
    let title = format!("Add {} to a collection", plural(paths.len(), "item"));
    open(&title, false, move |c| {
        let list = LocalResource::new(|| async { api::collections().await });
        let new_name = RwSignal::new(String::new());
        let (s1, p1, c1) = (source.clone(), paths.clone(), c);
        let add_to = move |id: i64, name: String| {
            let (s, p, c) = (s1.clone(), p1.clone(), c1);
            kit_leptos::rt::spawn(async move {
                match api::add_to_collection(id, &s, &p).await {
                    Ok(()) => {
                        c.close();
                        toast(&format!("Added to {name}"), "ok");
                    }
                    Err(e) => toast(&e.message, "err"),
                }
            });
        };
        let (s2, p2, c2) = (source.clone(), paths.clone(), c);
        let create = move |e: leptos::ev::SubmitEvent| {
            e.prevent_default();
            let name = new_name.get_untracked().trim().to_string();
            if name.is_empty() {
                return;
            }
            let (s, p, c) = (s2.clone(), p2.clone(), c2);
            kit_leptos::rt::spawn(async move {
                match api::new_collection(&name, &s, &p).await {
                    Ok(()) => {
                        c.close();
                        toast(&format!("Made {name}"), "ok");
                    }
                    Err(e) => toast(&e.message, "err"),
                }
            });
        };
        view! {
            <div class="stack">
                <ul class="picker-list">
                    {move || match list.get() {
                        None => view! { <li><div class="skeleton" style="height:36px"></div></li> }.into_any(),
                        Some(Err(e)) => view! { <li class="error-box">{e.message}</li> }.into_any(),
                        Some(Ok(v)) if v.is_empty() => view! { <li class="muted small picker-empty">"No collections yet. Name a new one below."</li> }.into_any(),
                        Some(Ok(v)) => v.into_iter().map(|col| {
                            let add = add_to.clone();
                            let n = col.name.clone();
                            view! { <li><button type="button" on:click=move |_| add(col.id, n.clone())><span inner_html=icon("collection")></span><span class="grow ellipsis">{col.name.clone()}</span><span class="faint small">{plural(col.item_count as usize, "item")}</span></button></li> }
                        }).collect_view().into_any(),
                    }}
                </ul>
                <form class="row" on:submit=create>
                    <input class="input grow" placeholder="New collection name" prop:value=new_name on:input=move |e| new_name.set(event_target_value(&e))/>
                    <button type="submit" class="btn" disabled=move || new_name.get().trim().is_empty()>"Make"</button>
                </form>
            </div>
        }
    });
}

/// The trash of one source: restore, delete for good, empty. `changed` runs after a restore.
pub fn trash(source: String, source_name: String, can_delete: bool, changed: impl Fn() + 'static) {
    let changed = Rc::new(changed);
    open(&format!("Trash of {source_name}"), true, move |_c| {
        let changed = StoredValue::new_local(changed);
        let rev = RwSignal::new(0u64);
        let s1 = source.clone();
        let list = LocalResource::new(move || {
            rev.track();
            let s = s1.clone();
            async move { api::trash_list(&s).await }
        });
        let bump = move || rev.update(|n| *n += 1);
        let restore = {
            move |it: TrashItem| {
                kit_leptos::rt::spawn(async move {
                    let r = match api::restore(&it.id, false).await {
                        Err(e) if e.status == 409 => api::restore(&it.id, true).await,
                        r => r,
                    };
                    match r {
                        Ok(()) => {
                            toast(&format!("Restored {}", it.name), "ok");
                            bump();
                            changed.with_value(|f| f());
                        }
                        Err(e) => toast(&e.message, "err"),
                    }
                });
            }
        };
        let purge = move |it: TrashItem| {
            kit_leptos::rt::spawn(async move {
                let q = format!("Delete {} for good?", it.name);
                if !modal::confirm_dialog(&q, "It can't be restored afterwards.", "Delete for good", true).await {
                    return;
                }
                match api::purge(&it.id).await {
                    Ok(()) => bump(),
                    Err(e) => toast(&e.message, "err"),
                }
            });
        };
        let s2 = source.clone();
        let empty = move |_| {
            let s = s2.clone();
            kit_leptos::rt::spawn(async move {
                if !modal::confirm_dialog("Empty the trash?", "Everything in it is deleted for good.", "Empty trash", true).await {
                    return;
                }
                match api::empty_trash(&s).await {
                    Ok(()) => {
                        toast("Trash emptied", "ok");
                        bump();
                    }
                    Err(e) => toast(&e.message, "err"),
                }
            });
        };
        view! {
            <div class="stack">
                {move || match list.get() {
                    None => view! { <div class="skeleton" style="height:120px"></div> }.into_any(),
                    Some(Err(e)) => view! { <div class="error-box">{e.message}</div> }.into_any(),
                    Some(Ok(v)) if v.is_empty() => view! { <div class="empty" inner_html=format!(r#"{}<p class="muted">The trash is empty.</p>"#, icon("trash"))></div> }.into_any(),
                    Some(Ok(v)) => {
                        let n = v.len();
                        let rows = v.into_iter().map(|it| {
                            let (r, p) = (restore, purge);
                            let (a, b) = (it.clone(), it.clone());
                            let ic = super::kinds::file_icon(&it.name, it.kind == "dir");
                            view! {
                                <tr>
                                    <td><span class="name-cell"><span class="file-icon" inner_html=icon(ic)></span><span class="ellipsis" title=it.path.clone()>{it.name.clone()}</span></span>
                                        <div class="small faint ellipsis">{it.path.clone()}</div></td>
                                    <td class="hide-phone small">{if it.kind == "dir" { "—".to_string() } else { format_size(it.size.max(0) as u64) }}</td>
                                    <td class="hide-phone small">{format_unix(it.deleted_at)}<div class="faint">{it.deleted_by.clone().unwrap_or_default()}</div></td>
                                    <td class="actions">
                                        <button type="button" class="btn sm" on:click=move |_| r(a.clone())><span inner_html=icon("restore")></span>"Restore"</button>
                                        {can_delete.then(|| view! { <button type="button" class="btn ghost sm icon" title="Delete for good" aria-label=format!("Delete {} for good", b.name) on:click=move |_| p(b.clone()) inner_html=icon("trash")></button> })}
                                    </td>
                                </tr>
                            }
                        }).collect_view();
                        view! {
                            <p class="small muted">{format!("{} in the trash. Items are deleted for good after the server's retention time.", plural(n, "item"))}</p>
                            <div class="table-wrap"><table class="table trash-table"><tbody>{rows}</tbody></table></div>
                        }.into_any()
                    }
                }}
                {can_delete.then(|| view! { <div class="dlg-actions"><button type="button" class="btn" on:click=empty><span inner_html=icon("trash")></span>"Empty trash"</button></div> })}
            </div>
        }
    });
}
