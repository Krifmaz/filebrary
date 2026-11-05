//! The account's own settings, one component per tab: Account, Security, Preferences and Appearance.

use super::client::{self, ApiKey, Quota, Totp, TotpSetup, KEY_SCOPES, VIEW_MODES};
use super::{done, fail, failed, ic, loading};
use crate::api::{Role, User};
use crate::route::{on_link, url, Route};
use kit_leptos::clipboard::copy_then;
use leptos::prelude::*;

/// A card's heading: icon, title, a line under it, and something on the right (a badge).
#[component]
fn CardHead(
    ic: &'static str,
    #[prop(into)] title: String,
    #[prop(into)] line: String,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    view! {
        <div class="adm-head">
            <span class="adm-icon" inner_html=self::ic(ic)></span>
            <div class="grow">
                <h2>{title}</h2>
                <p class="muted small">{line}</p>
            </div>
            {children.map(|c| c())}
        </div>
    }
}

/// The storage bar: used of the limit, or "no limit".
#[component]
pub fn QuotaBar(q: Quota) -> impl IntoView {
    match q.limit {
        None => view! { <p class="muted small">{format!("{} used · no storage limit", client::format_size(q.used))}</p> }.into_any(),
        Some(limit) => {
            let pct = if limit > 0 { ((q.used as f64 / limit as f64) * 100.0).clamp(0.0, 100.0).round() as i64 } else { 100 };
            let meter_class = if pct >= 90 { "adm-meter warn" } else { "adm-meter" };
            view! {
                <div class="adm-quota">
                    <div class="row small"><span class="grow">"Storage"</span>
                        <span class="muted">{format!("{} of {}", client::format_size(q.used), client::format_size(limit))}</span></div>
                    <div class=meter_class role="meter" aria-valuemin="0" aria-valuemax="100" aria-valuenow=pct.to_string() aria-label="Storage used">
                        <span style=format!("width:{pct}%")></span>
                    </div>
                </div>
            }
            .into_any()
        }
    }
}

/// The badge of a role.
pub fn role_badge(r: Role) -> impl IntoView {
    let tone = match r {
        Role::Admin => "badge ok",
        Role::ReadOnly => "badge info",
        Role::User => "badge",
    };
    view! { <span class=tone>{r.label()}</span> }
}

/// The first letter of a name, for the round badge.
pub fn initial(name: &str) -> String {
    name.chars().next().map(|c| c.to_uppercase().collect()).unwrap_or_else(|| "?".into())
}

/// Account: who is signed in, their storage, and changing the password.
#[component]
pub fn AccountTab(user: User) -> impl IntoView {
    let quota = LocalResource::new(|| async { client::my_quota().await.map_err(|e| e.message) });
    let current = RwSignal::new(String::new());
    let next = RwSignal::new(String::new());
    let confirm = RwSignal::new(String::new());
    let problem = RwSignal::new(None::<String>);
    let busy = RwSignal::new(false);
    let submit = move |e: leptos::ev::SubmitEvent| {
        e.prevent_default();
        let p = crate::api::password_problem(&next.get_untracked(), Some(&confirm.get_untracked()));
        problem.set(p.map(str::to_string));
        if p.is_some() || busy.get_untracked() {
            return;
        }
        busy.set(true);
        kit_leptos::rt::spawn(async move {
            match client::change_password(&current.get_untracked(), &next.get_untracked()).await {
                Ok(()) => {
                    current.set(String::new());
                    next.set(String::new());
                    confirm.set(String::new());
                    done("Password changed. Your other devices were signed out.");
                }
                Err(e) if e.status == 401 || e.status == 403 => problem.set(Some("Your current password is not right.".into())),
                Err(e) => problem.set(Some(e.message)),
            }
            busy.set(false);
        });
    };
    let role = user.role();
    view! {
        <div class="card adm-who">
            <span class="adm-avatar big">{initial(&user.username)}</span>
            <div class="grow">
                <div class="adm-name">{user.username.clone()}</div>
                {role_badge(role)}
            </div>
            <div class="adm-who-quota">
                {move || match quota.get() {
                    None => loading(),
                    Some(Ok(q)) => view! { <QuotaBar q=q/> }.into_any(),
                    Some(Err(e)) => failed(e, move || quota.refetch()),
                }}
            </div>
        </div>
        <form class="card" on:submit=submit aria-labelledby="pw-title">
            <h2 id="pw-title">"Change password"</h2>
            <p class="muted small">"Changing it signs you out everywhere else."</p>
            <div class="form-grid mt">
                <label class="field full"><span>"Current password"</span>
                    <input class="input" type="password" autocomplete="current-password" required
                        prop:value=move || current.get() on:input=move |e| current.set(event_target_value(&e))/>
                </label>
                <label class="field"><span>"New password"</span>
                    <input class="input" type="password" autocomplete="new-password" required minlength="8"
                        prop:value=move || next.get() on:input=move |e| next.set(event_target_value(&e))/>
                    <small>"At least 8 characters. A long phrase is easy to remember and hard to guess."</small>
                </label>
                <label class="field"><span>"Confirm new password"</span>
                    <input class="input" type="password" autocomplete="new-password" required minlength="8"
                        prop:value=move || confirm.get() on:input=move |e| confirm.set(event_target_value(&e))/>
                </label>
            </div>
            <p class="field-err small mt" role="alert" class:hidden=move || problem.get().is_none()>{move || problem.get()}</p>
            <div class="adm-actions">
                <button class="btn primary" type="submit" disabled=move || busy.get()>
                    {move || if busy.get() { "Changing…" } else { "Change password" }}
                </button>
            </div>
        </form>
    }
}

/// Security: two-factor sign-in and API keys.
#[component]
pub fn SecurityTab(user: User) -> impl IntoView {
    view! { <TotpCard/><ApiKeysCard admin=user.role() == Role::Admin/> }
}

/// The recovery codes or key just handed out: a warning line, the text, and Copy.
#[component]
fn Secret(#[prop(into)] line: String, text: String, #[prop(into)] copy_label: String, #[prop(into)] copied: String) -> impl IntoView {
    let t = text.clone();
    view! {
        <div class="adm-secret" role="status">
            <p><strong>{line}</strong></p>
            <pre class="code">{text}</pre>
            <button class="btn sm" type="button" on:click=move |_| copy_then(&t, &copied)>
                <span inner_html=ic("copy")></span>{copy_label}
            </button>
        </div>
    }
}

/// Two-factor sign-in: set up (key, code), recovery codes, turn off.
#[component]
fn TotpCard() -> impl IntoView {
    let status = LocalResource::new(|| async { client::totp().await.map_err(|e| e.message) });
    let pending = RwSignal::new(None::<TotpSetup>);
    let codes = RwSignal::new(None::<Vec<String>>);
    let code = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let reload = move || status.refetch();

    let start = move |_| {
        busy.set(true);
        error.set(None);
        kit_leptos::rt::spawn(async move {
            match client::totp_setup().await {
                Ok(p) => pending.set(Some(p)),
                Err(e) => error.set(Some(e.message)),
            }
            busy.set(false);
        });
    };
    let confirm = move |e: leptos::ev::SubmitEvent| {
        e.prevent_default();
        busy.set(true);
        error.set(None);
        kit_leptos::rt::spawn(async move {
            match client::totp_confirm(code.get_untracked().trim()).await {
                Ok(c) => {
                    codes.set(Some(c));
                    pending.set(None);
                    code.set(String::new());
                    reload();
                    done("Two-factor sign-in is on.");
                }
                Err(e) => error.set(Some(if e.status == 400 || e.status == 401 {
                    "That code did not work. Check the time on your phone and try the next one.".into()
                } else {
                    e.message
                })),
            }
            busy.set(false);
        });
    };
    let new_codes = move |_| {
        busy.set(true);
        error.set(None);
        kit_leptos::rt::spawn(async move {
            match client::totp_recovery(code.get_untracked().trim()).await {
                Ok(c) => {
                    codes.set(Some(c));
                    code.set(String::new());
                    reload();
                }
                Err(e) => error.set(Some(e.message)),
            }
            busy.set(false);
        });
    };
    let turn_off = move |_| {
        kit_leptos::rt::spawn(async move {
            let q = kit_rules::dialogs::Confirm::new("Turn off two-factor sign-in?")
                .text("Signing in will only need your password again.")
                .ok("Turn off")
                .danger(true);
            if !kit_leptos::modal::confirm(q).await {
                return;
            }
            busy.set(true);
            error.set(None);
            match client::totp_disable(code.get_untracked().trim()).await {
                Ok(()) => {
                    code.set(String::new());
                    codes.set(None);
                    reload();
                    done("Two-factor sign-in is off.");
                }
                Err(e) => error.set(Some(e.message)),
            }
            busy.set(false);
        });
    };

    let enabled = move || matches!(status.get(), Some(Ok(Totp { enabled: true, .. })));
    let badge = move || match status.get() {
        Some(Ok(t)) => {
            Some(view! { <span class=if t.enabled { "badge ok" } else { "badge" }>{if t.enabled { "On" } else { "Off" }}</span> })
        }
        _ => None,
    };
    let left = move || match status.get() {
        Some(Ok(t)) => format!("{} recovery codes left", t.recovery_codes_left),
        _ => String::new(),
    };
    let code_input = move || {
        view! {
            <input class="input mono" inputmode="numeric" autocomplete="one-time-code" placeholder="123456"
                prop:value=move || code.get() on:input=move |e| code.set(event_target_value(&e))/>
        }
    };
    view! {
        <section class="card" aria-labelledby="totp-title" id="totp-card">
            <CardHead ic="lock" title="Two-factor sign-in" line="Ask for a code from an authenticator app (such as Aegis, 2FAS or 1Password) after your password.">
                {badge}
            </CardHead>
            {move || match status.get() {
                None => Some(loading()),
                Some(Err(e)) => Some(failed(e, move || status.refetch())),
                _ => None,
            }}
            {move || codes.get().map(|c| view! {
                <Secret line="Your recovery codes. Each works once if you lose your phone. Save them now: they are not shown again." text=c.join("\n") copy_label="Copy codes" copied="Codes copied"/>
            })}
            <Show when=move || matches!(status.get(), Some(Ok(Totp { enabled: false, .. }))) && pending.get().is_none()>
                <div class="adm-actions start">
                    <button class="btn primary" disabled=move || busy.get() on:click=start>"Set up two-factor sign-in"</button>
                </div>
            </Show>
            {move || pending.get().map(|p| {
                let secret = client::grouped(&p.secret);
                view! {
                    <form class="adm-steps" on:submit=confirm>
                        <ol>
                            <li>
                                <p>"In your authenticator app, add an account with this key, or "<a href=p.uri.clone()>"open it in the app"</a>"."</p>
                                <code class="adm-key mono" data-details="">{secret}</code>
                            </li>
                            <li>
                                <label class="field"><span>"Enter the 6-digit code it shows"</span>{code_input()}</label>
                            </li>
                        </ol>
                        <div class="adm-actions">
                            <button class="btn ghost" type="button" on:click=move |_| pending.set(None)>"Cancel"</button>
                            <button class="btn primary" type="submit" disabled=move || busy.get()>"Turn on"</button>
                        </div>
                    </form>
                }
            })}
            <Show when=enabled>
                <div class="adm-inline">
                    <label class="field grow"><span>"Current code"</span>{code_input()}<small>{left}</small></label>
                    <div class="row wrap">
                        <button class="btn" disabled=move || busy.get() || code.get().is_empty() on:click=new_codes>"New recovery codes"</button>
                        <button class="btn danger" disabled=move || busy.get() || code.get().is_empty() on:click=turn_off>"Turn off"</button>
                    </div>
                </div>
            </Show>
            <p class="field-err small" role="alert" class:hidden=move || error.get().is_none()>{move || error.get()}</p>
        </section>
    }
}

/// API keys: the list with Revoke, and a form for a new one.
#[component]
fn ApiKeysCard(admin: bool) -> impl IntoView {
    let keys = LocalResource::new(|| async { client::api_keys().await.map_err(|e| e.message) });
    let name = RwSignal::new(String::new());
    let days = RwSignal::new(String::new());
    let scopes = RwSignal::new(vec!["files:read".to_string()]);
    let created = RwSignal::new(None::<client::NewApiKey>);
    let error = RwSignal::new(None::<String>);
    let busy = RwSignal::new(false);

    let create = move |e: leptos::ev::SubmitEvent| {
        e.prevent_default();
        let d = days.get_untracked();
        let d = d.trim();
        let days_n = if d.is_empty() {
            None
        } else {
            match d.parse::<u32>() {
                Ok(n) if n > 0 => Some(n),
                _ => {
                    error.set(Some("Use a whole number of days, or leave it empty for a key that does not expire.".into()));
                    return;
                }
            }
        };
        busy.set(true);
        error.set(None);
        kit_leptos::rt::spawn(async move {
            match client::create_api_key(name.get_untracked().trim(), &scopes.get_untracked(), days_n).await {
                Ok(k) => {
                    created.set(Some(k));
                    name.set(String::new());
                    days.set(String::new());
                    keys.refetch();
                }
                Err(e) => error.set(Some(e.message)),
            }
            busy.set(false);
        });
    };
    let revoke = move |k: ApiKey| {
        kit_leptos::rt::spawn(async move {
            let q = kit_rules::dialogs::Confirm::new(format!("Revoke “{}”?", k.name))
                .text("Anything using this key stops working.")
                .ok("Revoke")
                .danger(true);
            if !kit_leptos::modal::confirm(q).await {
                return;
            }
            match client::revoke_api_key(&k.id).await {
                Ok(()) => {
                    keys.refetch();
                    done("Key revoked.");
                }
                Err(e) => fail(e),
            }
        });
    };
    let list = move || {
        match keys.get() {
        None => loading(),
        Some(Err(e)) => failed(e, move || keys.refetch()),
        Some(Ok(list)) if list.is_empty() => view! { <p class="muted small">"No API keys yet. Create one below for a script or app."</p> }.into_any(),
        Some(Ok(list)) => view! {
            <ul class="list adm-list">
                {list.into_iter().map(|k| {
                    let k2 = k.clone();
                    view! {
                        <li class="list-item">
                            <div class="grow">
                                <strong>{k.name.clone()}</strong>
                                <div class="row wrap adm-chips">{k.scopes.iter().map(|s| view! { <span class="badge">{s.clone()}</span> }).collect_view()}</div>
                                <span class="faint small">{format!("Created {} · last used {} · expires {}", client::date(Some(k.created_at)), client::date(k.last_used_at), client::date(k.expires_at))}</span>
                            </div>
                            <button class="btn sm danger" on:click=move |_| revoke(k2.clone())>"Revoke"</button>
                        </li>
                    }
                }).collect_view()}
            </ul>
        }
        .into_any(),
    }
    };
    view! {
        <section class="card" aria-labelledby="keys-title" id="keys-card">
            <CardHead ic="key" title="API keys" line="For scripts and apps such as rclone. Send as Authorization: Bearer …"/>
            {move || created.get().map(|k| view! {
                <Secret line=format!("Copy your new key “{}” now. It is not shown again.", k.name) text=k.token copy_label="Copy key" copied="Key copied"/>
            })}
            {list}
            <form class="adm-new" on:submit=create>
                <h3>"New key"</h3>
                <div class="form-grid">
                    <label class="field"><span>"Name"</span>
                        <input class="input" required maxlength="64" placeholder="rclone on my laptop"
                            prop:value=move || name.get() on:input=move |e| name.set(event_target_value(&e))/>
                    </label>
                    <label class="field"><span>"Expires after (days)"</span>
                        <input class="input" inputmode="numeric" placeholder="never"
                            prop:value=move || days.get() on:input=move |e| days.set(event_target_value(&e))/>
                        <small>"Leave empty for a key that does not expire."</small>
                    </label>
                </div>
                <fieldset class="adm-checks">
                    <legend class="small muted">"Allowed"</legend>
                    {KEY_SCOPES.iter().filter(|(id, _)| admin || *id != "admin").map(|(id, words)| {
                        let id = id.to_string();
                        let id2 = id.clone();
                        view! {
                            <label class="switch">
                                <input type="checkbox" prop:checked=move || scopes.get().contains(&id)
                                    on:change=move |e| {
                                        let on = event_target_checked(&e);
                                        scopes.update(|s| { s.retain(|x| *x != id2); if on { s.push(id2.clone()); } });
                                    }/>
                                <i></i><span>{*words}</span>
                            </label>
                        }
                    }).collect_view()}
                </fieldset>
                <p class="field-err small" role="alert" class:hidden=move || error.get().is_none()>{move || error.get()}</p>
                <div class="adm-actions">
                    <button class="btn primary" type="submit" disabled=move || busy.get() || name.get().trim().is_empty() || scopes.get().is_empty()>
                        <span inner_html=ic("plus")></span>"Create key"
                    </button>
                </div>
            </form>
        </section>
    }
}

/// Preferences: how folders open.
#[component]
pub fn PreferencesTab() -> impl IntoView {
    let saved = LocalResource::new(|| async { client::view_mode().await.map_err(|e| e.message) });
    let chosen = RwSignal::new(None::<String>);
    let shown = move || chosen.get().or_else(|| saved.get().and_then(Result::ok));
    let pick = move |m: &'static str, words: &'static str| {
        let before = shown();
        chosen.set(Some(m.to_string()));
        kit_leptos::rt::spawn(async move {
            match client::set_view_mode(m).await {
                Ok(()) => done(&format!("Folders now open as {}.", words.to_lowercase())),
                Err(e) => {
                    chosen.set(before);
                    fail(e);
                }
            }
        });
    };
    view! {
        <section class="card" id="view-card">
            <CardHead ic="settings" title="Default view" line="How folders open. You can still switch per folder; folders you changed keep their own view."/>
            {move || match saved.get() {
                None => Some(loading()),
                Some(Err(e)) => Some(failed(e, move || saved.refetch())),
                Some(Ok(_)) => None,
            }}
            <div class="adm-choices" role="radiogroup" aria-label="Default view" class:hidden=move || shown().is_none()>
                {VIEW_MODES.iter().map(|(id, words)| {
                    let (id, words) = (*id, *words);
                    view! {
                        <label class="adm-choice" class:on=move || shown().as_deref() == Some(id)>
                            <input type="radio" name="view-mode" class="adm-sr" prop:checked=move || shown().as_deref() == Some(id) on:change=move |_| pick(id, words)/>
                            <span class="adm-preview" inner_html=view_icon(id)></span>
                            <strong>{words}</strong>
                        </label>
                    }
                }).collect_view()}
            </div>
        </section>
    }
}

/// A small drawing of a view mode.
fn view_icon(id: &str) -> String {
    let p = match id {
        "details" => r#"<path d="M4 6h16M4 12h16M4 18h16"/>"#,
        "compact" => r#"<path d="M3 6h7M3 12h7M3 18h7M14 6h7M14 12h7M14 18h7"/>"#,
        "small" => {
            r#"<rect x="3" y="3" width="4" height="4" rx="1"/><rect x="10" y="3" width="4" height="4" rx="1"/><rect x="17" y="3" width="4" height="4" rx="1"/><rect x="3" y="10" width="4" height="4" rx="1"/><rect x="10" y="10" width="4" height="4" rx="1"/><rect x="17" y="10" width="4" height="4" rx="1"/>"#
        }
        "medium" => {
            r#"<rect x="3" y="3" width="7" height="7" rx="1.5"/><rect x="14" y="3" width="7" height="7" rx="1.5"/><rect x="3" y="14" width="7" height="7" rx="1.5"/><rect x="14" y="14" width="7" height="7" rx="1.5"/>"#
        }
        _ => r#"<rect x="3" y="3" width="18" height="18" rx="2"/>"#,
    };
    format!(
        r#"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">{p}</svg>"#
    )
}

/// Appearance: the kit theme for this browser, and for administrators a link to the branding.
#[component]
pub fn AppearanceTab(admin: bool) -> impl IntoView {
    let theme = RwSignal::new(crate::theme::saved());
    let pick = move |id: &'static str| {
        crate::theme::set(id);
        theme.set(id.to_string());
    };
    let to = Route::Admin { page: "branding".into() };
    let branding_href = url(&to.href());
    let branding_go = on_link(to);
    view! {
        <section class="card" id="theme-card">
            <CardHead ic="palette" title="Theme" line="How Filebrary looks in this browser. The theme button in the top bar steps through them too."/>
            <div class="adm-choices" role="radiogroup" aria-label="Theme">
                {crate::theme::THEMES.iter().map(|(id, words)| {
                    let (id, words) = (*id, *words);
                    view! {
                        <label class="adm-choice" class:on=move || theme.get() == id>
                            <input type="radio" name="theme" class="adm-sr" prop:checked=move || theme.get() == id on:change=move |_| pick(id)/>
                            {swatch(id)}
                            <strong>{words}</strong>
                        </label>
                    }
                }).collect_view()}
            </div>
            <Show when=move || admin>
                <p class="muted small mt">"The server's name, logo and default theme are on the "
                    <a href=branding_href.clone() on:click=branding_go.clone()>"Branding"</a>" page."</p>
            </Show>
        </section>
    }
}

/// The kit's colour tokens, read for the theme tiles (the tokens live on `:root[data-theme]`, so a tile can't
/// borrow them by attribute).
const TOKENS: &str = include_str!("../../../static/css/tokens.css");

/// The `bg`, `panel`, `text` and `accent` colours of kit theme `id` from tokens.css (`encore` is the bare
/// `:root` block, which also fills what a theme leaves out; `system` shows `light`).
pub fn theme_colours(id: &str) -> [String; 4] {
    let id = if id == "system" { "light" } else { id };
    let block = if id == "encore" {
        TOKENS.split(":root[data-theme=").next().unwrap_or_default()
    } else {
        TOKENS.split(&format!(":root[data-theme=\"{id}\"]")).nth(1).and_then(|b| b.split('}').next()).unwrap_or_default()
    };
    let base = TOKENS.split(":root[data-theme=").next().unwrap_or_default();
    // a theme that leaves a token out keeps the bare `:root` one (as the browser does)
    let get = |name: &str| {
        let find = |b: &str| {
            b.lines()
                .map(str::trim)
                .find_map(|l| l.strip_prefix(&format!("--{name}:")))
                .map(|v| v.trim().trim_end_matches(';').trim().to_string())
        };
        find(block).or_else(|| find(base)).unwrap_or_else(|| format!("var(--{name})"))
    };
    [get("bg"), get("panel"), get("text"), get("accent")]
}

/// A theme's tile drawing: its background with a panel, text and accent stripe.
fn swatch(id: &str) -> impl IntoView {
    let [bg, panel, text, accent] = theme_colours(id);
    view! {
        <span class="adm-swatch" style=format!("background:{bg}")>
            <i style=format!("background:{panel}")></i><i style=format!("background:{text}")></i><i style=format!("background:{accent}")></i>
        </span>
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn theme_tiles_use_each_themes_own_colours() {
        let all: Vec<[String; 4]> = crate::theme::THEMES.iter().map(|(id, _)| super::theme_colours(id)).collect();
        for (c, (id, _)) in all.iter().zip(crate::theme::THEMES) {
            assert!(c.iter().all(|x| x.starts_with('#')), "{id}: {c:?}");
        }
        assert_ne!(all[0], all[1], "encore and dark differ");
        assert_eq!(super::theme_colours("system"), super::theme_colours("light"));
    }

    #[test]
    fn initials_are_one_capital() {
        assert_eq!(super::initial("ada"), "A");
        assert_eq!(super::initial("élise"), "É");
        assert_eq!(super::initial(""), "?");
    }

    #[test]
    fn every_view_mode_draws() {
        for (id, _) in super::VIEW_MODES {
            assert!(super::view_icon(id).contains("<path") || super::view_icon(id).contains("<rect"), "{id}");
        }
    }
}
