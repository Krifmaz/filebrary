//! The app frame around every signed-in page: the kit shell's sidebar (sources and sections), a top bar (menu
//! button on phones, search), the phone drawer and the page for the address.
//!
//! The page is chosen by its kind only ([`Kind`]), so moving between folders, search words or settings tabs
//! keeps the page mounted; pages read the rest of the address from signals.

use crate::api::{self, Role, Source, User};
use crate::icons::icon;
use crate::pages;
use crate::route::{self, on_link, url, Route};
use leptos::prelude::*;

/// Which page an address shows, without its details.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// The overview.
    Home,
    /// A folder in a source.
    Files,
    /// Starred items.
    Starred,
    /// The account's share links.
    Shares,
    /// Search.
    Search,
    /// Collections.
    Collections,
    /// The account's settings.
    Settings,
    /// Administration.
    Admin,
    /// No such page.
    NotFound,
}

impl Kind {
    /// The kind of a route (sign-in, setup and public pages are drawn outside the shell: they count as Home).
    pub fn of(r: &Route) -> Kind {
        match r {
            Route::Home | Route::Login { .. } | Route::Setup | Route::Public { .. } => Kind::Home,
            Route::Files { .. } => Kind::Files,
            Route::Starred => Kind::Starred,
            Route::Shares => Kind::Shares,
            Route::Search { .. } => Kind::Search,
            Route::Collections => Kind::Collections,
            Route::Settings { .. } => Kind::Settings,
            Route::Admin { .. } => Kind::Admin,
            Route::NotFound => Kind::NotFound,
        }
    }
}

/// The question Sign out asks first (the kit's confirm dialog).
pub fn sign_out_question() -> kit_rules::dialogs::Confirm {
    kit_rules::dialogs::Confirm::new("Sign out?")
        .text("This browser forgets its sign-in. You'll need your password to sign in again.")
        .ok("Sign out")
        .danger(true)
}

/// One link of the sidebar; `active` says when it is the current page.
#[component]
fn NavLink(to: Route, icon_name: &'static str, label: String, #[prop(into)] active: Signal<bool>, on_go: Callback<()>) -> impl IntoView {
    let href = url(&to.href());
    let go = on_link(to);
    view! {
        <a href=href class:active=move || active.get() aria-current=move || active.get().then_some("page")
            on:click=move |e| { go(e); on_go.run(()); }>
            <span inner_html=icon(icon_name)></span>
            <span class="ellipsis">{label}</span>
        </a>
    }
}

/// The sources in the sidebar (a spinner while they load, a short line when there are none).
#[component]
fn SourceLinks(sources: LocalResource<Result<Vec<Source>, String>>, current: Memo<Option<String>>, on_go: Callback<()>) -> impl IntoView {
    move || {
        match sources.get() {
        None => view! { <div class="nav-note"><span class="spinner"></span></div> }.into_any(),
        Some(Err(e)) => view! { <div class="nav-note small faint ellipsis" data-details="" title=e.clone()>"Sources could not load"</div> }.into_any(),
        Some(Ok(list)) if list.is_empty() => view! { <div class="nav-note small faint">"No sources yet"</div> }.into_any(),
        Some(Ok(list)) => list
            .into_iter()
            .map(|s| {
                let id = s.id.clone();
                let active = Signal::derive(move || current.get().as_deref() == Some(id.as_str()));
                view! { <NavLink to=Route::Files { source: s.id.clone(), path: "/".into() } icon_name="drive" label=s.name.clone() active=active on_go=on_go/> }
            })
            .collect_view()
            .into_any(),
    }
    }
}

/// The signed-in frame: sidebar, top bar and page.
#[component]
pub fn Shell(user: User) -> impl IntoView {
    let route = route::current();
    let kind = Memo::new(move |_| Kind::of(&route.get()));
    let in_source = Memo::new(move |_| match route.get() {
        Route::Files { source, .. } => Some(source),
        _ => None,
    });
    let (drawer, set_drawer) = signal(false);
    let close = Callback::new(move |_| set_drawer.set(false));
    let sources = LocalResource::new(|| async { api::sources().await.map_err(|e| e.message) });
    let admin = user.role() == Role::Admin;
    let session = crate::use_session();
    // the files browser's Share commands open the share dialog
    pages::shares::answer_share_requests();

    let sign_out = move |_| {
        kit_leptos::rt::spawn(async move {
            if kit_leptos::modal::confirm(sign_out_question()).await {
                match api::logout().await {
                    Ok(()) => {
                        route::go(Route::Login { next: "/".into() }, true);
                        session.set.set(crate::Session::SignedOut);
                    }
                    Err(e) => kit_leptos::toasts::toast(&e.message, "err"),
                }
            }
        })
    };

    let search_input = NodeRef::<leptos::html::Input>::new();
    let search = move |e: leptos::ev::SubmitEvent| {
        e.prevent_default();
        let q = search_input.get().map(|i| i.value()).unwrap_or_default();
        if !q.trim().is_empty() {
            route::go(Route::Search { q: q.trim().to_string(), kind: String::new() }, false);
        }
    };
    // the search box shows the words of the search page
    Effect::new(move |_| {
        if let (Route::Search { q, .. }, Some(i)) = (route.get(), search_input.get()) {
            i.set_value(&q);
        }
    });

    let (theme_now, set_theme_now) = signal(crate::theme::saved());
    let cycle_theme = move |_| {
        let t = crate::theme::next(&theme_now.get_untracked()).to_string();
        crate::theme::set(&t);
        set_theme_now.set(t);
    };
    let theme_title = move || format!("Theme: {} (click to change)", crate::theme::label(&theme_now.get()));

    let is = move |k: Kind| Signal::derive(move || kind.get() == k);
    let settings_active = Signal::derive(move || kind.get() == Kind::Settings);
    let page = move || match kind.get() {
        Kind::Home => view! { <pages::home::Home sources=sources/> }.into_any(),
        Kind::Files => {
            let source = Signal::derive(move || match route.get() {
                Route::Files { source, .. } => source,
                _ => String::new(),
            });
            let path = Signal::derive(move || match route.get() {
                Route::Files { path, .. } => path,
                _ => "/".into(),
            });
            view! { <pages::files::FilesPage source=source path=path/> }.into_any()
        }
        Kind::Starred => view! { <pages::files::StarredPage/> }.into_any(),
        Kind::Shares => view! { <pages::shares::SharesPage/> }.into_any(),
        Kind::Search => {
            let q = Signal::derive(move || match route.get() {
                Route::Search { q, .. } => q,
                _ => String::new(),
            });
            let kind = Signal::derive(move || match route.get() {
                Route::Search { kind, .. } => kind,
                _ => String::new(),
            });
            view! { <pages::search::SearchPage q=q kind=kind/> }.into_any()
        }
        Kind::Collections => view! { <pages::admin::CollectionsPage/> }.into_any(),
        Kind::Settings => {
            let tab = Signal::derive(move || match route.get() {
                Route::Settings { tab } => tab,
                _ => "account".into(),
            });
            view! { <pages::admin::SettingsPage tab=tab/> }.into_any()
        }
        Kind::Admin => {
            let page = Signal::derive(move || match route.get() {
                Route::Admin { page } => page,
                _ => "users".into(),
            });
            view! { <pages::admin::AdminPage page=page/> }.into_any()
        }
        Kind::NotFound => view! { <pages::NotFound/> }.into_any(),
    };

    let name = crate::app_name();
    let username = user.username.clone();
    let role = user.role().label();
    let home_href = url("/");
    let home_go = on_link(Route::Home);
    let settings_href = url("/settings/account");
    let settings_go = on_link(Route::Settings { tab: "account".into() });
    view! {
        <a class="skip-link" href="#view">"Skip to content"</a>
        <div class="shell">
            <div class="drawer-bg" class:open=drawer on:click=move |_| set_drawer.set(false) aria-hidden="true"></div>
            <aside class="sidebar" id="sidebar" class:open=drawer aria-label="Navigation">
                <div class="row between">
                    <a class="brand" href=home_href on:click=move |e| { home_go(e); set_drawer.set(false); }>
                        <img src=url("/icon.svg") alt=""/>
                        <span>{name}</span>
                    </a>
                    <button class="btn ghost icon drawer-close" title="Close menu" aria-label="Close menu" on:click=move |_| set_drawer.set(false) inner_html=icon("x")></button>
                </div>
                <nav class="nav">
                    <NavLink to=Route::Home icon_name="home" label="Overview".into() active=is(Kind::Home) on_go=close/>
                    <div class="nav-label">"Sources"</div>
                    <SourceLinks sources=sources current=in_source on_go=close/>
                    <div class="nav-label">"Library"</div>
                    <NavLink to=Route::Starred icon_name="star" label="Starred".into() active=is(Kind::Starred) on_go=close/>
                    <NavLink to=Route::Shares icon_name="share" label="Shared".into() active=is(Kind::Shares) on_go=close/>
                    <NavLink to=Route::Collections icon_name="collection" label="Collections".into() active=is(Kind::Collections) on_go=close/>
                    <Show when=move || admin>
                        <div class="nav-label">"Admin"</div>
                        <NavLink to=Route::Admin { page: "users".into() } icon_name="users" label="Users".into() active=is(Kind::Admin) on_go=close/>
                    </Show>
                </nav>
                <div class="sidebar-foot">
                    <div class="grow" style="min-width:0">
                        <div class="ellipsis" style="font-weight:650" id="me-name">{username}</div>
                        <div class="small faint">{role}</div>
                    </div>
                    <a class="btn ghost icon" href=settings_href title="Settings" aria-label="Settings" class:active=move || settings_active.get()
                        on:click=move |e| { settings_go(e); set_drawer.set(false); } inner_html=icon("settings")></a>
                    <button class="btn ghost icon" id="logout-btn" title="Sign out" aria-label="Sign out" on:click=sign_out inner_html=icon("logout")></button>
                </div>
            </aside>
            <main class="main">
                <header class="topbar">
                    <button class="btn ghost icon menu-toggle" title="Menu" aria-label="Open menu" aria-controls="sidebar" aria-expanded=move || drawer.get().to_string()
                        on:click=move |_| set_drawer.set(true) inner_html=icon("menu")></button>
                    <form class="search grow" role="search" on:submit=search>
                        <span inner_html=icon("search")></span>
                        <input class="input" type="search" name="q" placeholder="Search all files" aria-label="Search all files" autocomplete="off" node_ref=search_input/>
                    </form>
                    <button class="btn ghost icon" id="theme-btn" title=theme_title aria-label=theme_title on:click=cycle_theme inner_html=icon("palette")></button>
                </header>
                <div id="server-notice"></div>
                <div id="view" tabindex="-1">{page}</div>
            </main>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_are_picked_by_kind() {
        assert_eq!(Kind::of(&Route::Files { source: "a".into(), path: "/x".into() }), Kind::Files);
        assert_eq!(Kind::of(&Route::Files { source: "b".into(), path: "/".into() }), Kind::Files);
        assert_eq!(Kind::of(&Route::Settings { tab: "security".into() }), Kind::Settings);
        assert_eq!(Kind::of(&Route::NotFound), Kind::NotFound);
    }

    #[test]
    fn sign_out_asks_first() {
        let q = sign_out_question();
        assert_eq!((q.title.as_str(), q.ok.as_str(), q.danger), ("Sign out?", "Sign out", true));
    }
}
