//! `/shares`: the account's links, newest first, with a filter, each with Copy, Open, Change and Revoke, and
//! one button that clears the links that no longer work.

use super::api::{self, Share};
use super::format::{self, State};
use super::icon;
use crate::route::{on_link, url, Route};
use kit_rules::dialogs::Confirm;
use leptos::prelude::*;

/// Which links the list shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Filter {
    /// All of them.
    All,
    /// The ones that work.
    Active,
    /// The ones that don't any more.
    Dead,
}

/// The links `list` shows for `show` and the filter `words`, newest first.
pub fn visible(list: &[Share], show: Filter, words: &str) -> Vec<Share> {
    let mut v: Vec<Share> = list
        .iter()
        .filter(|s| match show {
            Filter::All => true,
            Filter::Active => format::state(s) == State::Active,
            Filter::Dead => format::state(s) != State::Active,
        })
        .filter(|s| format::matches(s, words))
        .cloned()
        .collect();
    v.sort_by(|a, b| b.created_at.cmp(&a.created_at).then(b.id.cmp(&a.id)));
    v
}

/// The question before revoking `s`.
pub fn revoke_question(s: &Share) -> Confirm {
    Confirm::new("Revoke this link?")
        .text(format!("{}: anyone using the link loses access straight away. This can't be undone.", format::target_words(s)))
        .ok("Revoke")
        .danger(true)
}

/// The question before clearing `n` links that no longer work.
pub fn clear_question(n: usize) -> Confirm {
    let what = if n == 1 { "1 link that no longer works".to_string() } else { format!("{n} links that no longer work") };
    Confirm::new("Remove dead links?").text(format!("This removes {what}. Links that still work stay.")).ok("Remove").danger(true)
}

/// The Shared page.
#[component]
pub fn SharesPage() -> impl IntoView {
    let links = RwSignal::new(None::<Result<Vec<Share>, String>>);
    let load = move || {
        kit_leptos::rt::spawn(async move {
            links.set(Some(api::list().await.map_err(|e| e.message)));
        })
    };
    load();
    let words = RwSignal::new(String::new());
    let show = RwSignal::new(Filter::All);

    let replace = move |s: Share| {
        links.update(|l| {
            if let Some(Ok(v)) = l {
                if let Some(x) = v.iter_mut().find(|x| x.id == s.id) {
                    *x = s;
                }
            }
        })
    };
    let remove = move |ids: Vec<i64>| {
        links.update(|l| {
            if let Some(Ok(v)) = l {
                v.retain(|x| !ids.contains(&x.id));
            }
        })
    };

    let revoke = move |s: Share| {
        kit_leptos::rt::spawn(async move {
            if !kit_leptos::modal::confirm(revoke_question(&s)).await {
                return;
            }
            match api::revoke(s.id).await {
                Ok(()) => {
                    remove(vec![s.id]);
                    kit_leptos::toasts::toast("Link revoked", "ok");
                }
                Err(e) => kit_leptos::toasts::toast(&e.message, "err"),
            }
        })
    };

    let dead = move || match links.get() {
        Some(Ok(v)) => v.iter().filter(|s| format::state(s) != State::Active).map(|s| s.id).collect::<Vec<_>>(),
        _ => Vec::new(),
    };
    let clear_dead = move |_| {
        let ids = dead();
        if ids.is_empty() {
            return;
        }
        kit_leptos::rt::spawn(async move {
            if !kit_leptos::modal::confirm(clear_question(ids.len())).await {
                return;
            }
            let mut gone = Vec::new();
            let mut failed = 0;
            for id in ids {
                match api::revoke(id).await {
                    Ok(()) => gone.push(id),
                    Err(_) => failed += 1,
                }
            }
            let n = gone.len();
            remove(gone);
            if failed > 0 {
                kit_leptos::toasts::toast(&format!("Removed {n}; {failed} could not be removed. Try again."), "err");
            } else {
                kit_leptos::toasts::toast(if n == 1 { "Removed 1 link" } else { "Removed the dead links" }, "ok");
            }
        })
    };

    let body = move || {
        match links.get() {
        None => view! {
            <div class="card flat"><div class="list">
                {(0..3).map(|_| view! { <div class="list-item"><div class="skeleton grow" style="height:40px"></div></div> }).collect_view()}
            </div></div>
        }
        .into_any(),
        Some(Err(e)) => view! {
            <div class="card">
                <div class="error-box" data-details="" title=e.clone()>{e.clone()}</div>
                <button class="btn mt" on:click=move |_| { links.set(None); load(); }><span inner_html=icon("refresh")></span>"Try again"</button>
            </div>
        }
        .into_any(),
        Some(Ok(all)) if all.is_empty() => view! {
            <div class="card"><div class="empty" id="shares-empty">
                <span inner_html=icon("share")></span>
                <h3>"No share links yet"</h3>
                <p class="muted">"Select files or a folder, then choose Share to make a link anyone can open."</p>
            </div></div>
        }
        .into_any(),
        Some(Ok(all)) => {
            let shown = visible(&all, show.get(), &words.get());
            if shown.is_empty() {
                return view! { <div class="card"><div class="empty"><p class="muted">"No links match."</p></div></div> }.into_any();
            }
            view! {
                <div class="card flat"><div class="list" id="share-list">
                    {shown.into_iter().map(|s| view! { <Item share=s on_saved=Callback::new(replace) on_revoke=Callback::new(revoke)/> }).collect_view()}
                </div></div>
            }
            .into_any()
        }
    }
    };

    let count = move || match links.get() {
        Some(Ok(v)) if !v.is_empty() => {
            let active = v.iter().filter(|s| format::state(s) == State::Active).count();
            format!("{} links, {active} working", v.len())
        }
        _ => String::new(),
    };

    view! {
        <div class="sh-head">
            <div>
                <h1>"Shared"</h1>
                <p class="muted">"Links you made. Anyone with a link can open it until it expires or you revoke it."</p>
            </div>
            <Show when=move || !dead().is_empty()>
                <button class="btn" id="clear-dead" on:click=clear_dead>
                    <span inner_html=icon("trash")></span>{move || format!("Remove dead links ({})", dead().len())}
                </button>
            </Show>
        </div>
        <Show when=move || matches!(links.get(), Some(Ok(v)) if !v.is_empty())>
            <div class="sh-tools">
                <input class="input" type="search" placeholder="Filter links" aria-label="Filter links" id="share-filter"
                    prop:value=move || words.get() on:input=move |e| words.set(event_target_value(&e))/>
                <div class="seg" role="radiogroup" aria-label="Show">
                    {[(Filter::All, "All"), (Filter::Active, "Working"), (Filter::Dead, "Dead")].map(|(v, l)| view! {
                        <button type="button" role="radio" class:on=move || show.get() == v aria-checked=move || (show.get() == v).to_string()
                            on:click=move |_| show.set(v)>{l}</button>
                    }).collect_view()}
                </div>
                <span class="small faint sh-hide-phone">{count}</span>
            </div>
        </Show>
        {body}
    }
}

/// One link of the list.
#[component]
fn Item(share: Share, on_saved: Callback<Share>, on_revoke: Callback<Share>) -> impl IntoView {
    let st = format::state(&share);
    let link = format::link(&share.token);
    let icon_name = if share.collection_id.is_some() {
        "collection"
    } else if share.upload_only {
        "inbox"
    } else {
        "link"
    };
    let title = format::target_words(&share);
    // a link to one path opens that place in the file list
    let place = match (share.collection_id, share.paths.as_slice()) {
        (None, [p]) => Some(Route::Files { source: share.source.clone(), path: p.clone() }),
        (Some(_), _) => Some(Route::Collections),
        _ => None,
    };
    let title_view = match place {
        Some(r) => view! { <a href=url(&r.href()) on:click=on_link(r.clone()) title=share.paths.join("\n")>{title}</a> }.into_any(),
        None => view! { <span title=share.paths.join("\n")>{title}</span> }.into_any(),
    };
    let expires = match (&share.expires_at, st) {
        (Some(e), State::Expired) => format!("Expired {}", format::date(e)),
        (Some(e), _) => format!("Expires {}", format::date(e)),
        (None, _) => "Never expires".into(),
    };
    let downloads = (!share.upload_only).then(|| format::downloads_words(share.downloads, share.max_downloads));
    let source = (share.collection_id.is_none() && !share.source.is_empty()).then(|| share.source.clone());
    let created = (!share.created_at.is_empty()).then(|| format!("Made {}", format::date(&share.created_at)));
    let copy = {
        let link = link.clone();
        move |_| kit_leptos::clipboard::copy_then(&link, "Link copied")
    };
    let edit = {
        let s = share.clone();
        move |_| super::dialog::edit_dialog(s.clone(), on_saved)
    };
    let revoke = {
        let s = share.clone();
        move |_| on_revoke.run(s.clone())
    };
    view! {
        <div class="list-item sh-item" class:dead=st != State::Active data-share=share.id>
            <span class="sh-icon" inner_html=icon(icon_name)></span>
            <div class="sh-main">
                <div class="sh-title">
                    {title_view}
                    {share.upload_only.then(|| view! { <span class="badge info">"Drop link"</span> })}
                    {share.has_password.then(|| view! { <span class="badge sh-badge"><span inner_html=icon("lock")></span>"Password"</span> })}
                    {(st != State::Active).then(|| view! { <span class="badge err">{st.label()}</span> })}
                </div>
                <div class="sh-meta">
                    {source.map(|s| view! { <span>{s}</span> })}
                    <span>{expires}</span>
                    {downloads.map(|d| view! { <span>{d}</span> })}
                    {created.map(|c| view! { <span class="sh-hide-phone">{c}</span> })}
                </div>
            </div>
            <div class="sh-actions">
                <button class="btn sm" type="button" on:click=copy disabled=st != State::Active><span inner_html=icon("copy")></span>"Copy link"</button>
                <a class="btn sm icon" href=link target="_blank" rel="noopener" title="Open the link" aria-label="Open the link" inner_html=icon("external")></a>
                <button class="btn sm icon" type="button" title="Change" aria-label="Change the link" on:click=edit inner_html=icon("edit")></button>
                <button class="btn sm icon" type="button" title="Revoke" aria-label="Revoke the link" on:click=revoke inner_html=icon("trash")></button>
            </div>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(id: i64, created: &str, expired: bool) -> Share {
        serde_json::from_value(
            serde_json::json!({"id": id, "token": "t", "source": "home", "paths": ["/a"], "created_at": created, "expired": expired}),
        )
        .unwrap()
    }

    #[test]
    fn the_list_is_newest_first_and_filters() {
        let all = vec![s(1, "2026-01-01T00:00:00Z", false), s(2, "2026-03-01T00:00:00Z", true), s(3, "2026-02-01T00:00:00Z", false)];
        let ids = |v: Vec<Share>| v.into_iter().map(|x| x.id).collect::<Vec<_>>();
        assert_eq!(ids(visible(&all, Filter::All, "")), [2, 3, 1]);
        assert_eq!(ids(visible(&all, Filter::Active, "")), [3, 1]);
        assert_eq!(ids(visible(&all, Filter::Dead, "")), [2]);
        assert!(visible(&all, Filter::All, "nothing like it").is_empty());
    }

    #[test]
    fn questions_name_what_goes() {
        let q = revoke_question(&s(1, "", false));
        assert!(q.danger && q.ok == "Revoke" && q.text.starts_with("a:"));
        assert!(clear_question(1).text.contains("1 link that"));
        assert!(clear_question(3).text.contains("3 links"));
    }
}
