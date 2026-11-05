//! Search across sources (names and full text). The scaffold draws the frame only.

use crate::pages::Placeholder;
use leptos::prelude::*;

/// `/search?q=…&type=…`.
#[component]
pub fn SearchPage(q: Signal<String>, kind: Signal<String>) -> impl IntoView {
    let words = move || format!("{} {}", q.get(), kind.get());
    view! { <div data-page="search" data-query=words><Placeholder title="Search" icon="search"/></div> }
}
