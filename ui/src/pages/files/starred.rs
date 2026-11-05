//! `/starred`: every file and folder the account starred, newest first. A folder opens in the browser; a file
//! opens its folder with the file shown and open in the viewer. The star button takes the star off, with Undo.

use super::api::{self, Star};
use super::kinds::file_icon;
use super::paths::{format_unix, parent};
use crate::icons::icon;
use crate::route::{self, Route};
use kit_leptos::toasts::{toast, toast_action};
use leptos::prelude::*;

/// Opens a star: a folder in the browser, a file in its folder's viewer.
fn open(st: &Star) {
    let st = st.clone();
    kit_leptos::rt::spawn(async move {
        match api::stat(&st.source, &st.path).await {
            Ok(e) if e.is_dir => route::go(Route::Files { source: st.source.clone(), path: st.path.clone() }, false),
            Ok(_) => {
                super::reveal::later(&st.name, true);
                route::go(Route::Files { source: st.source.clone(), path: parent(&st.path) }, false);
            }
            Err(e) => toast(&e.message, "err"),
        }
    });
}

/// The starred page.
#[component]
pub fn StarredPage() -> impl IntoView {
    let rev = RwSignal::new(0u64);
    let stars = LocalResource::new(move || {
        rev.track();
        async { api::stars().await }
    });
    let unstar = move |st: Star| {
        kit_leptos::rt::spawn(async move {
            match api::set_star(&st.source, &st.path, false).await {
                Ok(()) => {
                    rev.update(|n| *n += 1);
                    let back = st.clone();
                    toast_action(&format!("Took the star off {}", st.name), "Undo", 8000, move || {
                        kit_leptos::rt::spawn(async move {
                            if let Err(e) = api::set_star(&back.source, &back.path, true).await {
                                toast(&e.message, "err");
                            }
                            rev.update(|n| *n += 1);
                        });
                    });
                }
                Err(e) => toast(&e.message, "err"),
            }
        });
    };
    view! {
        <div data-page="starred">
            <h1>"Starred"</h1>
            <p class="muted" style="margin:6px 0 18px">"Files and folders you starred, newest first."</p>
            {move || match stars.get() {
                None => view! { <div class="card">{super::listing::skeleton_rows(5)}</div> }.into_any(),
                Some(Err(e)) => view! {
                    <div class="card"><div class="empty">
                        <span inner_html=icon("alert")></span>
                        <h3>"Your stars could not load"</h3>
                        <p class="muted" data-details="" title=e.message.clone()>{e.message.clone()}</p>
                        <button type="button" class="btn mt" on:click=move |_| rev.update(|n| *n += 1)>"Try again"</button>
                    </div></div>
                }.into_any(),
                Some(Ok(list)) if list.is_empty() => view! {
                    <div class="card"><div class="empty">
                        <span inner_html=icon("star")></span>
                        <h3>"Nothing starred yet"</h3>
                        <p class="muted">"Right-click a file or folder and choose Star, or press S, to find it here."</p>
                    </div></div>
                }.into_any(),
                Some(Ok(list)) => view! {
                    <ul class="card flat quick-list">
                        {list.into_iter().map(|st| {
                            let (a, b) = (st.clone(), st.clone());
                            let where_ = format!("{} · {}", st.source, parent(&st.path));
                            view! {
                                <li class="quick-row">
                                    <button type="button" class="quick-link" on:click=move |_| open(&a)>
                                        <span class="file-icon" inner_html=icon(file_icon(&st.name, !st.name.contains('.')))></span>
                                        <span class="grow" style="min-width:0">
                                            <span class="ellipsis" style="display:block;font-weight:600">{st.name.clone()}</span>
                                            <span class="small faint ellipsis" style="display:block">{where_}</span>
                                        </span>
                                        <span class="small faint hide-phone">{format_unix(st.starred_at)}</span>
                                    </button>
                                    <button type="button" class="btn ghost icon star-on" title="Remove star" aria-label=format!("Remove the star from {}", st.name)
                                        on:click=move |_| unstar(b.clone()) inner_html=icon("star-filled")></button>
                                </li>
                            }
                        }).collect_view()}
                    </ul>
                }.into_any(),
            }}
        </div>
    }
}
