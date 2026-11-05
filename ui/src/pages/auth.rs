//! The pages before an account is signed in: Sign in (with a second-factor step and single sign-on), first-run
//! Setup, and the page shown when the server can't be asked at all.

use crate::api::{self, Error};
use crate::route::{self, url, Route};
use crate::Session;
use leptos::prelude::*;

/// The sentence a failed sign-in shows (`with_code`: the second-factor step is open).
pub fn sign_in_error(e: &Error, with_code: bool) -> String {
    match (e.status, api::code(e)) {
        (429, _) => "Too many attempts. Wait a moment and try again.".into(),
        (401, _) if with_code => "That code did not work. Check your authenticator app.".into(),
        (401, _) => "Wrong username or password.".into(),
        _ => e.message.clone(),
    }
}

/// The sentence a failed setup shows.
pub fn setup_error(e: &Error) -> String {
    match e.status {
        429 => "Too many attempts. Wait a moment and try again.".into(),
        409 => "Setup is already complete. Reload the page to sign in.".into(),
        _ => e.message.clone(),
    }
}

/// The card every page here draws in: the logo and name, a title, a line under it, and the form.
#[component]
fn AuthCard(#[prop(into)] title: String, #[prop(into)] lead: String, children: Children) -> impl IntoView {
    view! {
        <div class="auth-wrap">
            <div class="auth-card">
                <div class="brand"><img src=url("/icon.svg") alt=""/><span>{crate::app_name()}</span></div>
                <h1 class="auth-title">{title}</h1>
                <p class="muted auth-lead">{lead}</p>
                {children()}
            </div>
        </div>
    }
}

/// A switch with its words.
#[component]
fn Switch(#[prop(into)] label: String, checked: RwSignal<bool>) -> impl IntoView {
    view! {
        <label class="switch">
            <input type="checkbox" prop:checked=move || checked.get() on:change=move |e| checked.set(event_target_checked(&e))/>
            <i></i>
            <span>{label}</span>
        </label>
    }
}

/// The sign-in page; `next` is where to go once signed in.
#[component]
pub fn SignIn(next: String) -> impl IntoView {
    let session = crate::use_session();
    let username = RwSignal::new(String::new());
    let password = RwSignal::new(String::new());
    let code = RwSignal::new(String::new());
    let remember = RwSignal::new(false);
    let need_code = RwSignal::new(false);
    let busy = RwSignal::new(false);
    let sso_failed =
        route::param(&kit_leptos::rt::window().and_then(|w| w.location().search().ok()).unwrap_or_default(), "error") == "sso_failed";
    let error = RwSignal::new(sso_failed.then(|| "Single sign-on failed. Try again or use your password.".to_string()));
    let methods = LocalResource::new(|| async { api::auth_methods().await.unwrap_or_default() });
    let sso_href = api::oidc_login_url(&url(&next));
    let next_route = {
        let (p, q) = next.split_once('?').unwrap_or((&next, ""));
        Route::parse(p, q)
    };

    let submit = move |e: leptos::ev::SubmitEvent| {
        e.prevent_default();
        if busy.get_untracked() {
            return;
        }
        busy.set(true);
        error.set(None);
        let with_code = need_code.get_untracked();
        let next_route = next_route.clone();
        kit_leptos::rt::spawn(async move {
            let c = code.get_untracked();
            match api::login(
                &username.get_untracked(),
                &password.get_untracked(),
                with_code.then_some(c.as_str()),
                remember.get_untracked(),
            )
            .await
            {
                Ok(user) => {
                    crate::kit::signed_in(&user);
                    route::go(next_route, true);
                    session.set.set(Session::SignedIn(user));
                }
                Err(e) if api::code(&e) == "totp_required" && !with_code => {
                    busy.set(false);
                    need_code.set(true);
                }
                Err(e) => {
                    busy.set(false);
                    error.set(Some(sign_in_error(&e, with_code)));
                }
            }
        });
    };

    let sso = move || {
        methods.get().and_then(|m| m.oidc).map(|o| {
            view! {
                <div class="auth-or"><span>"or"</span></div>
                <a class="btn auth-sso" href=sso_href.clone() rel="external">{o.label}</a>
            }
        })
    };
    view! {
        <AuthCard title="Sign in" lead="Welcome back. Sign in to reach your files.">
            <form autocomplete="on" on:submit=submit id="sign-in">
                <label class="field"><span>"Username"</span>
                    <input class="input" name="username" autocomplete="username" required autofocus
                        prop:value=move || username.get() on:input=move |e| username.set(event_target_value(&e))/>
                </label>
                <label class="field"><span>"Password"</span>
                    <input class="input" type="password" name="password" autocomplete="current-password" required
                        prop:value=move || password.get() on:input=move |e| password.set(event_target_value(&e))/>
                </label>
                <Show when=move || need_code.get()>
                    <label class="field"><span>"Authentication code"</span>
                        <input class="input mono" name="totp" inputmode="numeric" autocomplete="one-time-code" required autofocus
                            prop:value=move || code.get() on:input=move |e| code.set(event_target_value(&e))/>
                        <small>"The 6-digit code from your authenticator app, or a recovery code."</small>
                    </label>
                </Show>
                <Switch label="Remember me on this device" checked=remember/>
                <div class="error-box" role="alert" id="auth-err" class:hidden=move || error.get().is_none()>{move || error.get()}</div>
                <button class="btn primary auth-submit" type="submit" disabled=move || busy.get()>
                    {move || if busy.get() { "Signing in…" } else { "Sign in" }}
                </button>
                {sso}
            </form>
        </AuthCard>
    }
}

/// First-run setup: the administrator's name and password, and the one-time setup token from the server log
/// that proves the person runs the server.
#[component]
pub fn Setup() -> impl IntoView {
    let session = crate::use_session();
    let token = RwSignal::new(String::new());
    let username = RwSignal::new("admin".to_string());
    let password = RwSignal::new(String::new());
    let confirm = RwSignal::new(String::new());
    let remember = RwSignal::new(true);
    let busy = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let pw_error = RwSignal::new(None::<&'static str>);

    let submit = move |e: leptos::ev::SubmitEvent| {
        e.prevent_default();
        let problem = api::password_problem(&password.get_untracked(), Some(&confirm.get_untracked()));
        pw_error.set(problem);
        if problem.is_some() || busy.get_untracked() {
            return;
        }
        busy.set(true);
        error.set(None);
        kit_leptos::rt::spawn(async move {
            match api::setup(
                token.get_untracked().trim(),
                username.get_untracked().trim(),
                &password.get_untracked(),
                remember.get_untracked(),
            )
            .await
            {
                Ok(user) => {
                    crate::kit::signed_in(&user);
                    route::go(Route::Home, true);
                    session.set.set(Session::SignedIn(user));
                }
                Err(e) => {
                    busy.set(false);
                    error.set(Some(setup_error(&e)));
                }
            }
        });
    };
    view! {
        <AuthCard title="Set up your server" lead="Create the administrator account. There is no default password: you choose it now.">
            <form on:submit=submit id="setup">
                <label class="field"><span>"Setup token"</span>
                    <input class="input mono" name="token" autocomplete="off" spellcheck="false" required autofocus
                        prop:value=move || token.get() on:input=move |e| token.set(event_target_value(&e))/>
                    <small>"From the server log. With Docker: " <code>"docker logs filebrary"</code></small>
                </label>
                <label class="field"><span>"Administrator name"</span>
                    <input class="input" name="username" autocomplete="username" required maxlength="64"
                        prop:value=move || username.get() on:input=move |e| username.set(event_target_value(&e))/>
                </label>
                <label class="field"><span>"Password"</span>
                    <input class="input" type="password" name="password" autocomplete="new-password" required minlength="8"
                        prop:value=move || password.get() on:input=move |e| password.set(event_target_value(&e))/>
                    <small>"At least 8 characters."</small>
                </label>
                <label class="field"><span>"Confirm password"</span>
                    <input class="input" type="password" name="confirm" autocomplete="new-password" required minlength="8"
                        prop:value=move || confirm.get() on:input=move |e| confirm.set(event_target_value(&e))/>
                    <small class="field-err" class:hidden=move || pw_error.get().is_none()>{move || pw_error.get()}</small>
                </label>
                <Switch label="Keep me signed in on this device" checked=remember/>
                <div class="error-box" role="alert" class:hidden=move || error.get().is_none()>{move || error.get()}</div>
                <button class="btn primary auth-submit" type="submit" disabled=move || busy.get()>"Create administrator"</button>
            </form>
            <p class="small faint auth-foot">"The setup token is printed once in the server log each time it starts while no account exists."</p>
        </AuthCard>
    }
}

/// The page when the server could not be asked who is signed in, with Try again.
#[component]
pub fn Unreachable(message: String, on_retry: Callback<()>) -> impl IntoView {
    view! {
        <AuthCard title="Can't reach the server" lead="Filebrary didn't answer. Check that it is running, then try again.">
            <div class="error-box mt" data-details="" title=message.clone()>{message.clone()}</div>
            <button class="btn primary auth-submit mt" style="width:100%" on:click=move |_| on_retry.run(())>"Try again"</button>
        </AuthCard>
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn err(status: u16, code: &str) -> Error {
        Error { status, message: "server words".into(), data: json!({ "error": code }) }
    }

    #[test]
    fn sign_in_errors_read_plainly() {
        assert_eq!(sign_in_error(&err(401, "invalid_credentials"), false), "Wrong username or password.");
        assert_eq!(sign_in_error(&err(401, "invalid_credentials"), true), "That code did not work. Check your authenticator app.");
        assert_eq!(sign_in_error(&err(429, "too_many_requests"), false), "Too many attempts. Wait a moment and try again.");
        assert_eq!(sign_in_error(&err(500, "x"), false), "server words");
    }

    #[test]
    fn setup_errors_read_plainly() {
        assert_eq!(setup_error(&err(409, "conflict")), "Setup is already complete. Reload the page to sign in.");
        assert_eq!(setup_error(&err(403, "forbidden")), "server words");
    }
}
