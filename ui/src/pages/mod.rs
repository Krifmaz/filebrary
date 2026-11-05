//! The pages. `auth` (sign-in, setup) and `home` belong to the app frame; `files`, `search`, `shares` and
//! `admin` are folders of their own, each grown in its own change.

pub mod admin;
pub mod auth;
pub mod files;
pub mod home;
pub mod search;
pub mod shares;

use crate::route::{on_link, url, Route};
use leptos::prelude::*;

/// A page that is not built yet in this UI: its title and a line saying so, with a link to the overview.
#[component]
pub fn Placeholder(#[prop(into)] title: String, #[prop(into)] icon: String) -> impl IntoView {
    view! {
        <h1>{title}</h1>
        <div class="card mt">
            <div class="empty" inner_html=format!(r#"{}<p class="muted">This page is still being built in the new UI.</p>"#, crate::icons::icon(&icon))></div>
        </div>
    }
}

/// The page for an address the app does not know.
#[component]
pub fn NotFound() -> impl IntoView {
    view! {
        <div class="empty" id="not-found">
            <h1>"Page not found"</h1>
            <p class="muted">"This address doesn't lead anywhere in Filebrary. Check the link, or go back to the overview."</p>
            <a class="btn primary mt" href=url("/") on:click=on_link(Route::Home)>"Go to the overview"</a>
        </div>
    }
}
