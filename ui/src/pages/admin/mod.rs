//! The account's settings and the administration pages.
//!
//! | Module | Page |
//! | --- | --- |
//! | [`settings`] | `/settings/<tab>`: Account (profile, storage, password), Security (two-factor, API keys), Preferences (default view), Appearance (theme) |
//! | [`users`] | `/admin/users`: accounts, with a dialog per account for role, password, access per source, groups, quota, two-factor |
//! | [`access`] | `/admin/permissions` (every rule, per source, for users and groups) and `/admin/groups` |
//! | [`jobs`] | `/admin/jobs`: background jobs and the search index |
//! | [`branding`] | `/admin/branding`: the name, logo and default theme |
//! | [`collections`] | `/collections`: the account's collections and their share links |
//!
//! The calls and pure helpers are in [`client`]; the pages' own styles in `admin.css` (tokens only, so every
//! kit theme draws them). Dialogs are the kit's ([`kit_leptos::modal`]), questions the kit's confirm dialog,
//! messages the kit's toasts (errors stay, with Copy).

mod access;
mod branding;
mod client;
mod collections;
mod jobs;
mod settings;
mod users;

use crate::api::{Error, Role, Source};
use crate::icons::icon;
use crate::route::{on_link, url, Route};
use leptos::prelude::*;

/// The admin pages: address segment, name and icon, in the order the tabs show them.
pub const ADMIN_PAGES: [(&str, &str, &str); 5] = [
    ("users", "Users", "users"),
    ("groups", "Groups", "users"),
    ("permissions", "Permissions", "lock"),
    ("jobs", "Jobs", "refresh"),
    ("branding", "Branding", "palette"),
];

/// The icon `name`: the few these pages draw that neither the kit nor the app has yet, else the app's.
fn ic(name: &str) -> String {
    let p = match name {
        "key" => r#"<circle cx="7.5" cy="15.5" r="4.5"/><path d="m10.7 12.3 9.8-9.8M17 6l3 3M14.5 8.5l2 2"/>"#,
        "file" => r#"<path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z"/><path d="M14 3v5h5"/>"#,
        _ => return icon(name),
    };
    format!(
        r#"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">{p}</svg>"#
    )
}

/// The pages' own styles, once per page.
#[component]
fn Styles() -> impl IntoView {
    view! { <style>{include_str!("admin.css")}</style> }
}

/// A row of tabs that are links (`current`: the one that is on).
#[component]
fn LinkTabs(
    tabs: Vec<(Route, &'static str, &'static str)>,
    #[prop(into)] current: Signal<Route>,
    #[prop(into)] label: String,
) -> impl IntoView {
    view! {
        <nav class="seg adm-tabs" aria-label=label>
            {tabs
                .into_iter()
                .map(|(to, name, ic)| {
                    let on = { let to = to.clone(); move || current.get() == to };
                    let on2 = on.clone();
                    view! {
                        <a href=url(&to.href()) class:on=on aria-current=move || on2().then_some("page") on:click=on_link(to.clone())>
                            <span inner_html=self::ic(ic)></span>{name}
                        </a>
                    }
                })
                .collect_view()}
        </nav>
    }
}

/// An error toast with the server's sentence (it stays, with Copy).
fn fail(e: Error) {
    kit_leptos::toasts::toast(&e.message, "err");
}

/// A short success toast.
fn done(msg: &str) {
    kit_leptos::toasts::toast(msg, "ok");
}

/// A card's loading state.
fn loading() -> AnyView {
    view! { <div class="stack"><div class="skeleton" style="height:18px;width:40%"></div><div class="skeleton" style="height:18px;width:70%"></div></div> }.into_any()
}

/// A failed load: the sentence (cut-off text opens in full) and Try again.
fn failed(msg: String, retry: impl Fn() + 'static) -> AnyView {
    view! {
        <div class="error-box row wrap" role="alert">
            <span class="grow" data-details="" title=msg.clone()>{msg.clone()}</span>
            <button class="btn sm" on:click=move |_| retry()>"Try again"</button>
        </div>
    }
    .into_any()
}

/// An empty state: icon, title and a line.
fn empty(ic: &str, title: &str, line: &str) -> AnyView {
    let html = format!(
        r#"{}<strong>{}</strong><p class="muted small">{}</p>"#,
        icon(ic),
        kit_leptos::html::esc(title),
        kit_leptos::html::esc(line)
    );
    view! { <div class="empty adm-empty" inner_html=html></div> }.into_any()
}

/// The signed-in account (the pages draw inside the signed-in shell).
fn me() -> Option<crate::api::User> {
    match crate::use_session().get.get_untracked() {
        crate::Session::SignedIn(u) => Some(u),
        _ => None,
    }
}

/// The sources the signed-in account sees (administrators see every one).
fn sources_resource() -> LocalResource<Result<Vec<Source>, String>> {
    LocalResource::new(|| async { crate::api::sources().await.map_err(|e| e.message) })
}

/// `/settings/<tab>`: Account, Security, Preferences and Appearance.
#[component]
pub fn SettingsPage(tab: Signal<String>) -> impl IntoView {
    let tabs = vec![
        (Route::Settings { tab: "account".into() }, "Account", "user"),
        (Route::Settings { tab: "security".into() }, "Security", "lock"),
        (Route::Settings { tab: "preferences".into() }, "Preferences", "settings"),
        (Route::Settings { tab: "appearance".into() }, "Appearance", "palette"),
    ];
    let current = Signal::derive(move || Route::Settings { tab: tab.get() });
    let Some(user) = me() else { return ().into_any() };
    let body = move || {
        let user = user.clone();
        match tab.get().as_str() {
            "security" => view! { <settings::SecurityTab user=user/> }.into_any(),
            "preferences" => view! { <settings::PreferencesTab/> }.into_any(),
            "appearance" => view! { <settings::AppearanceTab admin=user.role() == Role::Admin/> }.into_any(),
            _ => view! { <settings::AccountTab user=user/> }.into_any(),
        }
    };
    view! {
        <Styles/>
        <section class="adm" data-page="settings" data-tab=move || tab.get()>
            <h1>"Settings"</h1>
            <p class="muted">"Your account, sign-in security and how Filebrary looks for you."</p>
            <LinkTabs tabs=tabs current=current label="Settings sections"/>
            <div class="stack">{body}</div>
        </section>
    }
    .into_any()
}

/// `/admin/<page>`: Users, Groups, Permissions, Jobs and Branding (administrators only).
#[component]
pub fn AdminPage(page: Signal<String>) -> impl IntoView {
    let admin = me().is_some_and(|u| u.role() == Role::Admin);
    if !admin {
        return view! {
            <Styles/>
            <section class="adm" data-page="admin">
                <h1>"Administration"</h1>
                <div class="card">{empty("lock", "Administrators only", "Ask an administrator of this server if you need something changed here.")}</div>
            </section>
        }
        .into_any();
    }
    let me_id = me().map(|u| u.id).unwrap_or_default();
    let tabs = ADMIN_PAGES.iter().map(|(id, name, ic)| (Route::Admin { page: id.to_string() }, *name, *ic)).collect();
    let current = Signal::derive(move || Route::Admin { page: page.get() });
    let sources = sources_resource();
    let body = move || match page.get().as_str() {
        "users" => view! { <users::UsersPage me=me_id sources=sources/> }.into_any(),
        "groups" => view! { <access::GroupsPage/> }.into_any(),
        "permissions" => view! { <access::PermissionsPage sources=sources/> }.into_any(),
        "jobs" => view! { <jobs::JobsPage/> }.into_any(),
        "branding" => view! { <branding::BrandingPage/> }.into_any(),
        _ => view! { <div class="card">{empty("alert", "No such admin page", "Pick one of the tabs above.")}</div> }.into_any(),
    };
    view! {
        <Styles/>
        <section class="adm" data-page="admin" data-admin=move || page.get()>
            <h1>"Administration"</h1>
            <p class="muted">"Accounts, who can reach what, background work and how the server presents itself."</p>
            <LinkTabs tabs=tabs current=current label="Administration sections"/>
            <div class="stack">{body}</div>
        </section>
    }
    .into_any()
}

/// `/collections`.
#[component]
pub fn CollectionsPage() -> impl IntoView {
    view! { <Styles/><collections::Collections/> }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_admin_page_has_an_address_and_an_icon() {
        for (id, name, ic) in ADMIN_PAGES {
            let r = Route::Admin { page: id.into() };
            assert_eq!(Route::parse(&r.href(), ""), r, "{name}");
            assert!(super::ic(ic).len() > 150, "{ic}");
        }
        for name in ["key", "file", "refresh", "plus", "trash", "copy", "x", "collection", "folder", "share", "search", "alert"] {
            assert!(super::ic(name).len() > 150, "{name}");
        }
    }

    #[test]
    fn the_styles_use_tokens_only() {
        let css = include_str!("admin.css");
        // colours come from the kit's tokens, so every theme draws the pages
        assert!(!css.contains('#'), "no literal colours in admin.css");
    }
}
