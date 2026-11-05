//! The account's settings and the administration pages. The scaffold draws the frame only.

use crate::pages::Placeholder;
use leptos::prelude::*;

/// `/settings/<tab>`: Account, Security, Preferences and Appearance.
#[component]
pub fn SettingsPage(tab: Signal<String>) -> impl IntoView {
    view! { <div data-page="settings" data-tab=move || tab.get()><Placeholder title="Settings" icon="settings"/></div> }
}

/// `/admin/<page>`: Users and the other administration pages.
#[component]
pub fn AdminPage(page: Signal<String>) -> impl IntoView {
    view! { <div data-page="admin" data-admin=move || page.get()><Placeholder title="Users" icon="users"/></div> }
}

/// `/collections`.
#[component]
pub fn CollectionsPage() -> impl IntoView {
    view! { <Placeholder title="Collections" icon="collection"/> }
}
