//! `/collections`: the account's collections, the items of the open one, and share links that serve a whole
//! collection (its items may come from several sources).

use super::client::{self, Collection, CollectionItem};
use super::{done, empty, fail, failed, ic, loading};
use crate::route::{on_link, url, Route};
use kit_leptos::modal::{self, ModalOpts};
use kit_rules::dialogs::Confirm;
use leptos::prelude::*;

/// The folder an item opens in: a folder itself, a file its parent folder (`None` when it is out of reach).
pub fn open_route(it: &CollectionItem) -> Option<Route> {
    let path = it.path.clone()?;
    it.kind.as_ref()?;
    let folder = if it.kind.as_deref() == Some("dir") {
        path
    } else {
        match path.rsplit_once('/') {
            Some(("", _)) | None => "/".to_string(),
            Some((p, _)) => p.to_string(),
        }
    };
    Some(Route::Files { source: it.source.clone(), path: folder })
}

/// The Collections page.
#[component]
pub fn Collections() -> impl IntoView {
    let list = LocalResource::new(|| async { client::collections().await.map_err(|e| e.message) });
    let open_id = RwSignal::new(None::<i64>);
    let open = LocalResource::new(move || {
        let id = open_id.get();
        async move {
            match id {
                Some(id) => client::collection(id).await.map(Some).map_err(|e| e.message),
                None => Ok(None),
            }
        }
    });
    let reload = move || {
        list.refetch();
        open.refetch();
    };

    let new_one = move |_| {
        name_dialog("New collection", String::new(), "Make collection", move |name| {
            kit_leptos::rt::spawn(async move {
                match client::create_collection(&name).await {
                    Ok(c) => {
                        list.refetch();
                        open_id.set(Some(c.id));
                    }
                    Err(e) => fail(e),
                }
            })
        })
    };

    let side = move || {
        match list.get() {
        None => loading(),
        Some(Err(e)) => failed(e, move || list.refetch()),
        Some(Ok(cs)) if cs.is_empty() => empty("collection", "No collections yet", "Make one here, then add files to it from any source."),
        Some(Ok(cs)) => view! {
            <ul class="adm-coll-list" aria-label="Your collections">
                {cs.into_iter().map(|c| {
                    let id = c.id;
                    view! {
                        <li>
                            <button class:on=move || open_id.get() == Some(id) aria-current=move || (open_id.get() == Some(id)).then_some("true") on:click=move |_| open_id.set(Some(id))>
                                <span inner_html=ic("collection")></span>
                                <span class="grow ellipsis">{c.name.clone()}</span>
                                <span class="badge">{c.item_count}</span>
                            </button>
                        </li>
                    }
                }).collect_view()}
            </ul>
        }
        .into_any(),
    }
    };

    let detail = move || {
        match open.get() {
        _ if open_id.get().is_none() => match list.get() {
            Some(Ok(cs)) if !cs.is_empty() => view! { <div class="card">{empty("collection", "Pick a collection", "Choose one to see what is in it.")}</div> }.into_any(),
            _ => ().into_any(),
        },
        None | Some(Ok(None)) => view! { <div class="card">{loading()}</div> }.into_any(),
        Some(Err(e)) => failed(e, move || open.refetch()),
        Some(Ok(Some(c))) => view! { <Detail c=c reload=Callback::new(move |_| reload()) closed=Callback::new(move |_| { open_id.set(None); list.refetch(); })/> }.into_any(),
    }
    };

    view! {
        <section class="adm" data-page="collections">
            <div class="adm-toolbar">
                <div class="grow">
                    <h1>"Collections"</h1>
                    <p class="muted">"Group files from any source and share them with one link."</p>
                </div>
                <button class="btn primary" id="new-collection" on:click=new_one><span inner_html=ic("plus")></span>"New collection"</button>
            </div>
            <div class="adm-coll">
                <div class="card adm-coll-side">{side}</div>
                <div class="adm-coll-main">{detail}</div>
            </div>
        </section>
    }
}

/// The open collection: Share, Rename, Delete and its items.
#[component]
fn Detail(c: Collection, reload: Callback<()>, closed: Callback<()>) -> impl IntoView {
    let id = c.id;
    let name = c.name.clone();
    let count = c.items.len();
    let rename = {
        let name = name.clone();
        move |_| {
            name_dialog(&format!("Rename {name}"), name.clone(), "Rename", move |n| {
                kit_leptos::rt::spawn(async move {
                    match client::rename_collection(id, &n).await {
                        Ok(()) => reload.run(()),
                        Err(e) => fail(e),
                    }
                })
            })
        }
    };
    let delete = {
        let name = name.clone();
        move |_| {
            let name = name.clone();
            kit_leptos::rt::spawn(async move {
                let q = Confirm::new(format!("Delete {name}?"))
                    .text("The files stay where they are; share links to this collection stop working.")
                    .ok("Delete")
                    .danger(true);
                if !modal::confirm(q).await {
                    return;
                }
                match client::delete_collection(id).await {
                    Ok(()) => {
                        done(&format!("Deleted {name}."));
                        closed.run(());
                    }
                    Err(e) => fail(e),
                }
            })
        }
    };
    let share = {
        let name = name.clone();
        move |_| share_dialog(id, name.clone(), count)
    };
    let items = if c.items.is_empty() {
        view! { <div class="card">{empty("collection", "This collection is empty", "Select files in a source and choose Add to collection.")}</div> }.into_any()
    } else {
        view! {
            <ul class="card flat list adm-items">
                {c.items.into_iter().map(|it| {
                    let item = it.id;
                    let place = format!("{}{}", it.source, it.path.as_ref().map(|p| format!(":{p}")).unwrap_or_default());
                    let size = it.size.map(|s| client::format_size(s as i64)).unwrap_or_else(|| "—".into());
                    let icon_name = if it.kind.as_deref() == Some("dir") { "folder" } else { "file" };
                    let link = match open_route(&it) {
                        Some(r) => view! { <a class="ellipsis" href=url(&r.href()) on:click=on_link(r.clone())>{it.name.clone()}</a> }.into_any(),
                        None => view! { <span class="ellipsis faint" title="Missing or no longer within your reach">{format!("{} (unavailable)", it.name)}</span> }.into_any(),
                    };
                    let label = format!("Take {} out", it.name);
                    view! {
                        <li class="list-item">
                            <span class="adm-file" inner_html=ic(icon_name)></span>
                            <div class="grow adm-item-main">{link}<span class="faint small ellipsis">{place}</span></div>
                            <span class="small muted adm-hide-phone">{size}</span>
                            <button class="btn sm ghost icon" title=label.clone() aria-label=label inner_html=ic("x")
                                on:click=move |_| kit_leptos::rt::spawn(async move {
                                    match client::remove_item(id, item).await { Ok(()) => reload.run(()), Err(e) => fail(e) }
                                })></button>
                        </li>
                    }
                }).collect_view()}
            </ul>
        }
        .into_any()
    };
    view! {
        <div class="adm-toolbar">
            <h2 class="grow ellipsis">{name}</h2>
            <button class="btn sm" disabled=count == 0 on:click=share><span inner_html=ic("share")></span>"Share"</button>
            <button class="btn sm" on:click=rename>"Rename"</button>
            <button class="btn sm danger" on:click=delete><span inner_html=ic("trash")></span>"Delete"</button>
        </div>
        {items}
    }
}

/// A dialog that asks for a name; `save` gets it, trimmed.
fn name_dialog(title: &str, initial: String, ok: &'static str, save: impl Fn(String) + Clone + 'static) {
    let body = move || {
        let name = RwSignal::new(initial);
        let submit = move |e: leptos::ev::SubmitEvent| {
            e.prevent_default();
            let n = name.get_untracked().trim().to_string();
            if n.is_empty() {
                return;
            }
            modal::close_all();
            save(n);
        };
        view! {
            <form class="stack" on:submit=submit>
                <label class="field"><span>"Name"</span>
                    <input class="input" autofocus maxlength="200" prop:value=move || name.get() on:input=move |e| name.set(event_target_value(&e))/>
                </label>
                <div class="adm-actions">
                    <button class="btn ghost" type="button" data-close="">"Cancel"</button>
                    <button class="btn primary" type="submit" disabled=move || name.get().trim().is_empty()>{ok}</button>
                </div>
            </form>
        }
    };
    modal::open(ModalOpts::with_view(title, body));
}

/// The share dialog of a collection: password, expiry, download limit, then the link with Copy.
fn share_dialog(id: i64, name: String, count: usize) {
    let title = format!("Share {name}");
    let body = move || {
        let password = RwSignal::new(String::new());
        let expires = RwSignal::new(String::new());
        let limit = RwSignal::new(String::new());
        let link = RwSignal::new(None::<String>);
        let busy = RwSignal::new(false);
        let error = RwSignal::new(None::<String>);
        let submit = move |e: leptos::ev::SubmitEvent| {
            e.prevent_default();
            let max = match limit.get_untracked().trim() {
                "" => None,
                s => match s.parse::<u64>() {
                    Ok(n) if n > 0 => Some(n),
                    _ => {
                        error.set(Some("Use a whole number of downloads, or leave it empty.".into()));
                        return;
                    }
                },
            };
            busy.set(true);
            error.set(None);
            kit_leptos::rt::spawn(async move {
                match client::share_collection(id, &password.get_untracked(), client::local_to_rfc3339(&expires.get_untracked()), max).await
                {
                    Ok(s) => link.set(Some(s.url)),
                    Err(e) => error.set(Some(e.message)),
                }
                busy.set(false);
            });
        };
        let words = format!("Anyone with the link can open the {count} items of {name}.");
        move || {
            match link.get() {
            Some(u) => {
                let u2 = u.clone();
                view! {
                    <div class="stack">
                        <p>{words.clone()}</p>
                        <div class="adm-pw">
                            <input class="input mono" readonly prop:value=u aria-label="Share address" on:focus=|e| { if let Some(i) = e.target().and_then(|t| wasm_bindgen::JsCast::dyn_into::<web_sys::HtmlInputElement>(t).ok()) { i.select(); } }/>
                            <button class="btn" on:click=move |_| kit_leptos::clipboard::copy_then(&u2, "Link copied")><span inner_html=ic("copy")></span>"Copy"</button>
                        </div>
                        <div class="adm-actions"><button class="btn primary" data-close="" on:click=|_| modal::close_all()>"Done"</button></div>
                    </div>
                }
                .into_any()
            }
            None => view! {
                <form class="stack" on:submit=submit>
                    <p class="muted small">"The link always shows the collection's items as they are now, as far as you may share them."</p>
                    <label class="field"><span>"Password (optional)"</span>
                        <input class="input" type="password" autocomplete="new-password" prop:value=move || password.get() on:input=move |e| password.set(event_target_value(&e))/>
                    </label>
                    <div class="form-grid">
                        <label class="field"><span>"Expires (optional)"</span>
                            <input class="input" type="datetime-local" prop:value=move || expires.get() on:input=move |e| expires.set(event_target_value(&e))/>
                        </label>
                        <label class="field"><span>"Download limit (optional)"</span>
                            <input class="input" inputmode="numeric" prop:value=move || limit.get() on:input=move |e| limit.set(event_target_value(&e))/>
                        </label>
                    </div>
                    <p class="field-err small" role="alert" class:hidden=move || error.get().is_none()>{move || error.get()}</p>
                    <div class="adm-actions">
                        <button class="btn ghost" type="button" on:click=|_| modal::close_all()>"Cancel"</button>
                        <button class="btn primary" type="submit" disabled=move || busy.get()>"Make link"</button>
                    </div>
                </form>
            }
            .into_any(),
        }
        }
    };
    modal::open(ModalOpts::with_view(&title, body));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(path: Option<&str>, kind: Option<&str>) -> CollectionItem {
        CollectionItem {
            id: 1,
            source: "home".into(),
            path: path.map(Into::into),
            name: "x".into(),
            kind: kind.map(Into::into),
            size: None,
        }
    }

    #[test]
    fn items_open_in_their_folder() {
        let f = |p: &str| Some(Route::Files { source: "home".into(), path: p.into() });
        assert_eq!(open_route(&item(Some("/a/b.txt"), Some("file"))), f("/a"));
        assert_eq!(open_route(&item(Some("/b.txt"), Some("file"))), f("/"));
        assert_eq!(open_route(&item(Some("/a/c"), Some("dir"))), f("/a/c"));
        assert_eq!(open_route(&item(None, Some("file"))), None);
        assert_eq!(open_route(&item(Some("/gone"), None)), None);
    }
}
