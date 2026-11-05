//! Filebrary's web UI: Leptos (client-side, WebAssembly) on the kit's components and Encore theme.
//!
//! | Module | What |
//! | --- | --- |
//! | [`api`] | typed client for the REST API, with the CSRF header |
//! | [`route`] | the app's addresses (real paths under a base, `/next` until cutover) |
//! | [`shell`] | the app frame: sidebar, top bar, phone drawer |
//! | [`pages`] | one module per page; files, search, shares and admin each have their own folder |
//! | [`kit`] | the kit's features (toasts, banners, What's new, …) |
//! | [`theme`] | the kit theme on `<html data-theme>`, remembered per browser |
//!
//! Start-up: the kit first, then the page for the address. A public share link draws without an account;
//! otherwise the server is asked whether first-run setup is waiting, then who is signed in.

mod api;
mod icons;
mod kit;
mod pages;
mod route;
mod shell;
mod theme;

use leptos::prelude::*;
use route::Route;
use wasm_bindgen::JsCast;

/// The package name, `filebrary-web`: trunk names the script `filebrary-web-<hash>.js`.
pub const NAME: &str = env!("CARGO_PKG_NAME");

/// The app's name as people see it, from `<meta name="application-name">`.
pub fn app_name() -> String {
    kit_leptos::rt::qs(r#"meta[name="application-name"]"#).and_then(|m| m.get_attribute("content")).unwrap_or_else(|| "Filebrary".into())
}

/// The project id (`NAME` without `-web`): the `X-Requested-With` value and the prefix of the kit's events.
pub fn project() -> &'static str {
    NAME.strip_suffix("-web").unwrap_or(NAME)
}

/// Who is using the app: still asking, nobody (sign in), setup waiting, or a signed-in account.
#[derive(Clone, Debug, PartialEq)]
pub enum Session {
    /// Asking the server.
    Loading,
    /// First-run setup is waiting.
    Setup,
    /// Nobody is signed in.
    SignedOut,
    /// The server could not be asked (the sentence to show).
    Failed(String),
    /// Signed in.
    SignedIn(api::User),
}

/// The session signal every page can read ([`use_session`]).
#[derive(Clone, Copy)]
pub struct SessionCtx {
    /// Who is signed in.
    pub get: ReadSignal<Session>,
    /// Changes it (after signing in, out or setting up).
    pub set: WriteSignal<Session>,
}

/// The session from context (inside the app).
pub fn use_session() -> SessionCtx {
    expect_context::<SessionCtx>()
}

/// Asks the server who is signed in and whether setup is waiting.
async fn load_session() -> Session {
    match api::setup_needed().await {
        Ok(true) => return Session::Setup,
        Ok(false) => {}
        Err(e) if e.status == 0 => return Session::Failed(e.message),
        Err(_) => {}
    }
    match api::me().await {
        Ok(u) => Session::SignedIn(u),
        Err(e) if e.status == 401 || e.status == 403 => Session::SignedOut,
        Err(e) => Session::Failed(e.message),
    }
}

/// The whole app: the page the address and the session call for.
#[component]
fn App() -> impl IntoView {
    let (session, set_session) = signal(Session::Loading);
    provide_context(SessionCtx { get: session, set: set_session });
    let route = route::current();
    let reload = move || {
        kit_leptos::rt::spawn(async move {
            let s = load_session().await;
            if let Session::SignedIn(u) = &s {
                kit::signed_in(u.role() == api::Role::Admin);
            }
            set_session.set(s);
        })
    };
    // a call answered 401: the session ended somewhere else
    kit_leptos::rt::on_window(&format!("{}:unauthorized", project()), move |_| {
        if matches!(session.get_untracked(), Session::SignedIn(_)) {
            set_session.set(Session::SignedOut);
        }
    });

    // the sign-in and setup addresses are done with once signed in
    Effect::new(move |_| {
        if let Session::SignedIn(_) = session.get() {
            match route.get() {
                Route::Login { next } => {
                    let (p, q) = next.split_once('?').unwrap_or((&next, ""));
                    route::go(Route::parse(p, q), true);
                }
                Route::Setup => route::go(Route::Home, true),
                _ => {}
            }
        }
    });

    // only a change between a public share and the app redraws everything; pages follow the route themselves
    let public = Memo::new(move |_| match route.get() {
        Route::Public { token, path } => Some((token, path)),
        _ => None,
    });
    // ask the server once the app (not a public share) is shown for the first time
    let asked = StoredValue::new(false);
    Effect::new(move |_| {
        if public.get().is_none() && !asked.get_value() {
            asked.set_value(true);
            reload();
        }
    });
    move || {
        if let Some((token, path)) = public.get() {
            return view! { <pages::shares::PublicSharePage token=token path=path/> }.into_any();
        }
        match session.get() {
            Session::Loading => view! { <div class="auth-wrap"><span class="spinner"></span></div> }.into_any(),
            Session::Failed(msg) => view! { <pages::auth::Unreachable message=msg on_retry=Callback::new(move |_| { set_session.set(Session::Loading); reload(); })/> }.into_any(),
            Session::Setup => view! { <pages::auth::Setup/> }.into_any(),
            Session::SignedOut => {
                let next = match &route.get_untracked() {
                    Route::Login { next } => next.clone(),
                    Route::Setup | Route::NotFound => "/".into(),
                    other => other.href(),
                };
                view! { <pages::auth::SignIn next=next/> }.into_any()
            }
            Session::SignedIn(user) => view! { <shell::Shell user=user/> }.into_any(),
        }
    }
}

/// Starts the kit, applies the saved theme and draws the app into `#app`.
fn main() {
    std::panic::set_hook(Box::new(|p| web_sys::console::error_1(&p.to_string().into())));
    theme::apply(theme::saved());
    kit::start();
    if let Some(icon) = kit_leptos::rt::by_id("favicon") {
        let _ = icon.set_attribute("href", &route::url("/icon.svg"));
    }
    let Some(root) = kit_leptos::rt::by_id("app").and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok()) else { return };
    leptos::mount::mount_to(root, App).forget();
}

#[cfg(test)]
mod tests {
    /// The stylesheet copies stay equal to the pinned kit's (`scripts/sync-kit.sh` copies them again).
    #[test]
    fn kit_stylesheet_matches_the_pinned_kit() {
        assert_eq!(include_str!("../static/css/kit.css"), kit_leptos::CSS, "run ui/scripts/sync-kit.sh");
    }
}
