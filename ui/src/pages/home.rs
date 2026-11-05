//! `/`: the overview, a greeting and one card per source the account can reach.

use crate::api::Source;
use crate::icons::icon;
use crate::route::{on_link, url, Route};
use leptos::prelude::*;

/// What a source's permissions allow, in a few words ("View, upload, share").
pub fn permission_words(s: &Source) -> String {
    let p = s.permissions;
    let words: Vec<&str> =
        [(p.view, "view"), (p.download, "download"), (p.upload, "upload"), (p.edit, "edit"), (p.delete, "delete"), (p.share, "share")]
            .into_iter()
            .filter_map(|(on, w)| on.then_some(w))
            .collect();
    if words.is_empty() {
        return "No access".into();
    }
    let s = words.join(", ");
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

/// One source's card.
fn source_card(s: Source) -> impl IntoView {
    let to = Route::Files { source: s.id.clone(), path: "/".into() };
    let href = url(&to.href());
    let words = permission_words(&s);
    view! {
        <a class="card source-card" href=href on:click=on_link(to)>
            <span class="source-icon" inner_html=icon("drive")></span>
            <div class="grow">
                <h3 class="ellipsis">{s.name}</h3>
                <div class="small faint ellipsis">{words}</div>
            </div>
        </a>
    }
}

/// The overview.
#[component]
pub fn Home(sources: LocalResource<Result<Vec<Source>, String>>) -> impl IntoView {
    let session = crate::use_session();
    let hello = move || match session.get.get() {
        crate::Session::SignedIn(u) => format!("Hello, {}", u.username),
        _ => "Hello".into(),
    };
    let cards = move || {
        match sources.get() {
        None => view! { <div class="source-grid"><div class="skeleton" style="height:84px"></div><div class="skeleton" style="height:84px"></div></div> }.into_any(),
        Some(Err(e)) => view! { <div class="error-box" data-details="" title=e.clone()>{e.clone()}</div> }.into_any(),
        Some(Ok(list)) if list.is_empty() => view! {
            <div class="card"><div class="empty" inner_html=format!(r#"{}<p class="muted">No sources yet. An administrator adds them in the server's settings.</p>"#, icon("drive"))></div></div>
        }
        .into_any(),
        Some(Ok(list)) => view! { <div class="source-grid">{list.into_iter().map(source_card).collect_view()}</div> }.into_any(),
    }
    };
    view! {
        <section class="hero">
            <h1>{hello}</h1>
            <p class="muted" style="margin:6px 0 0">"Pick a source to browse, or search everything from the bar above."</p>
        </section>
        <div class="section-head"><h2>"Sources"</h2></div>
        {cards}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::Permissions;

    #[test]
    fn permissions_read_as_words() {
        let mut s = Source { id: "a".into(), name: "A".into(), permissions: Permissions::default() };
        assert_eq!(permission_words(&s), "No access");
        s.permissions.view = true;
        s.permissions.share = true;
        assert_eq!(permission_words(&s), "View, share");
    }
}
