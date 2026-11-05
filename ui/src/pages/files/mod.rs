//! Browsing: the file list, uploads, previews, stars and trash. The scaffold draws the frame only.

use crate::pages::Placeholder;
use leptos::prelude::*;

/// `/files/<source>/<path…>`: one folder of a source.
#[component]
pub fn FilesPage(source: Signal<String>, path: Signal<String>) -> impl IntoView {
    let title = move || format!("{} {}", source.get(), path.get());
    view! { <div data-page="files" data-title=title><Placeholder title="Files" icon="folder"/></div> }
}

/// `/starred`: starred files and folders.
#[component]
pub fn StarredPage() -> impl IntoView {
    view! { <Placeholder title="Starred" icon="star"/> }
}
