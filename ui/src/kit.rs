//! Where Filebrary's UI meets the kit's features: everything here is the kit's, configured for Filebrary.
//!
//! | Feature | Where |
//! | --- | --- |
//! | toasts with Copy, the details dialog, drag and drop | [`start`] (every page) |
//! | crash logs | [`start`] catches panics and errors; [`signed_in`] sends what an earlier visit left |
//! | the connection banner | [`start`]: "Not connected" offline, "The Filebrary server isn't answering" while it's down |
//! | self-updating web app | [`start`]; it waits while an upload or an unsaved edit holds the page ([`hold`]) |
//! | notice banners with Check again | [`signed_in`] (admins), drawn in [`NoticeSlot`] |
//! | What's new | the popup once per release after signing in ([`signed_in`]), the changelog at `/whats-new` ([`WhatsNewPage`]) |
//! | bug reports | New report anywhere ([`new_report`]), the account's reports at `/bug-reports` ([`BugReportsPage`]) |
//! | Diagnostics | `/admin/diagnostics` ([`DiagnosticsPage`]) |
//! | settings from the server's schema | `/admin/server` ([`ServerSettingsPage`]) |
//! | offline mode | when the server can't be reached at start, the account this browser last saw opens with the kit's offline banner and Retry ([`offline_session`]) |
//!
//! Confirm dialogs are the kit's `modal::confirm`, called where a page asks.

use crate::api::User;
use crate::icons::icon;
use crate::route::{self, on_link, url, Route};
use kit_leptos::bug_report::{self, SheetOpts};
use kit_leptos::notice::NoticeSource;
use kit_leptos::schema_form::schema::{visible_cards, Schema};
use kit_leptos::schema_form::SettingsCards;
use kit_leptos::{api, crash, details, dropzone, notice, offline, rt, toasts, update, whats_new};
use kit_rules::notices::CheckWords;
use kit_rules::offline::mode;
use leptos::prelude::*;
use serde_json::Value;
use std::cell::Cell;
use std::rc::Rc;

/// Where the popup's Full changelog goes.
pub const CHANGELOG_PATH: &str = "/whats-new";

/// The administration pages the kit draws: `(page, title, icon)` under `/admin/<page>`.
pub const ADMIN_PAGES: [(&str, &str, &str); 2] = [("server", "Server settings", "settings"), ("diagnostics", "Diagnostics", "alert")];

/// Whether `/admin/<page>` is one of the kit's pages.
pub fn is_admin_page(page: &str) -> bool {
    ADMIN_PAGES.iter().any(|(p, _, _)| *p == page)
}

/// The kit's configuration for Filebrary: id `filebrary`, the name people see, the script stem
/// (`filebrary-web`) whose hash names the web build, and the changelog page.
fn config(app: &str) -> kit_leptos::config::AppConfig {
    let mut cfg = kit_leptos::config::AppConfig::new(kit_rules::app::App::new(crate::project(), app));
    cfg.script = crate::NAME.to_string();
    cfg.routes.changelog = CHANGELOG_PATH.into();
    cfg
}

/// Opens an app path (`/whats-new`, `/settings/account?x=1`) without reloading: the kit's links go here.
pub fn navigate(path: &str) {
    let (p, q) = path.split_once('?').unwrap_or((path, ""));
    route::go(Route::parse(p, q), false);
}

/// Starts the kit's parts that live outside any page. Call once, before anything is drawn.
pub fn start() {
    let app = crate::app_name();
    kit_leptos::config::set(config(&app));
    kit_leptos::api::set_backend(crate::api::backend());
    crash::install();
    toasts::mount();
    details::init();
    dropzone::init();
    watch_connection(&app);
    update::start(update_config());
    // the server's version names bug reports and crash logs
    rt::spawn(async {
        if let Some(h) = update::health().await {
            let mut c = (*kit_leptos::config::get()).clone();
            c.app.version = h.version;
            kit_leptos::config::set(c);
        }
    });
}

/// Starts the kit's parts that need a signed-in account: the notices (administrators), What's new and the
/// crash logs an earlier visit left; and remembers the account for offline mode.
pub fn signed_in(user: &User) {
    remember(user);
    notice::start(notices(), user.role() == crate::api::Role::Admin);
    whats_new::start(Rc::new(navigate));
    rt::spawn(crash::flush());
}

// ---- self-update -------------------------------------------------------------------------------------------

/// What can hold a new build back: the page reloads into it only once none of these is going on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hold {
    /// Files are uploading.
    Upload,
    /// A text file has changes that are not saved.
    Edit,
}

impl Hold {
    /// The update toast's second line while this holds the page.
    pub fn words(self) -> &'static str {
        match self {
            Hold::Upload => "It reloads when the uploads are done",
            Hold::Edit => "It reloads when your changes are saved",
        }
    }

    fn index(self) -> usize {
        match self {
            Hold::Upload => 0,
            Hold::Edit => 1,
        }
    }
}

thread_local! {
    static HOLDS: [Cell<u32>; 2] = const { [Cell::new(0), Cell::new(0)] };
}

/// Says that `what` started (`on`) or ended holding the page. Calls nest: each start needs its end.
#[allow(dead_code)] // the files pages call it (uploads, the text editor)
pub fn hold(what: Hold, on: bool) {
    HOLDS.with(|h| {
        let c = &h[what.index()];
        c.set(if on { c.get() + 1 } else { c.get().saturating_sub(1) });
    });
}

/// Whether `what` holds the page now.
pub fn held(what: Hold) -> bool {
    HOLDS.with(|h| h[what.index()].get() > 0)
}

/// The self-update: the configured app, held by uploads and unsaved edits (an open dialog holds it too).
fn update_config() -> update::UpdateConfig {
    update::UpdateConfig::from_config().hold(Hold::Upload.words(), || held(Hold::Upload)).hold(Hold::Edit.words(), || held(Hold::Edit))
}

// ---- banners -----------------------------------------------------------------------------------------------

/// The connection banner: "Not connected" while offline, "The Filebrary server isn't answering" while the
/// server is down (the kit backend's `filebrary:server-down` and `filebrary:server-up`).
fn watch_connection(app: &str) {
    use kit_rules::offline::mode::{server_down, NOT_CONNECTED};
    let down = server_down(app);
    let p = crate::project();
    rt::on_window("offline", |_| offline::conn_banner(true, NOT_CONNECTED));
    rt::on_window("online", |_| offline::conn_banner(false, ""));
    rt::on_window(&format!("{p}:server-down"), move |_| offline::conn_banner(true, &down));
    rt::on_window(&format!("{p}:server-up"), |_| offline::conn_banner(false, ""));
}

/// The server's notices (`GET /api/notices`), with Check again's words for the data folder and the sources
/// (the server's `kit/checks.rs`).
pub fn notices() -> NoticeSource {
    NoticeSource::default()
        .words_for(
            "data",
            CheckWords {
                failing: "The data folder still can't be written",
                checking: "Still checking the data folder",
                fixed: "The data folder can be written again",
            },
        )
        .words_for(
            "sources",
            CheckWords {
                failing: "Some sources still can't be reached",
                checking: "Still checking the sources",
                fixed: "Every source answers again",
            },
        )
}

/// `<div id="server-notice">`, where the notice banners are drawn (the slot is empty while there is none).
#[component]
pub fn NoticeSlot() -> impl IntoView {
    let slot = NodeRef::<leptos::html::Div>::new();
    Effect::new(move |_| {
        if slot.get().is_some() {
            notice::mount();
        }
    });
    view! { <div id="server-notice" node_ref=slot></div> }
}

// ---- offline mode ------------------------------------------------------------------------------------------

fn local() -> Option<web_sys::Storage> {
    rt::window().and_then(|w| w.local_storage().ok().flatten())
}

fn now_s() -> f64 {
    rt::now_ms() as f64 / 1000.0
}

/// Remembers the signed-in account in this browser for offline mode (without its email address).
fn remember(user: &User) {
    let (Some(s), Ok(v)) = (local(), serde_json::to_value(user)) else { return };
    let _ = s.set_item(&mode::me_key(crate::project()), &mode::me_doc(&v, now_s()).to_string());
}

/// Forgets the remembered account (after signing out).
pub fn forget() {
    if let Some(s) = local() {
        let _ = s.remove_item(&mode::me_key(crate::project()));
    }
}

/// The account offline mode opens with when asking the server who is signed in failed with `status`: the
/// one this browser remembered (`None`: sign in as usual). Offline mode's banner, with Retry, is shown at once;
/// the grace of [`mode::GRACE_DAYS`] days decides what it says.
pub fn offline_session(status: u16) -> Option<User> {
    let text = local()?.get_item(&mode::me_key(crate::project())).ok().flatten()?;
    let (v, at) = mode::parse_me(&text)?;
    let user: User = serde_json::from_value(v).ok()?;
    let fresh = mode::grace_left(at, now_s()) > 0;
    let (user, ended) = mode::decide(status, fresh.then(|| user.clone()), Some(user), 1)?;
    offline::show_banner(ended, Rc::new(retry));
    retry_while_offline();
    Some(user)
}

/// Offline mode's Retry: back online reloads (into the sign-in page when the session has gone), still offline
/// says so.
fn retry() {
    offline::set_trying(true);
    rt::spawn(async {
        let status = match crate::api::me().await {
            Ok(_) => 200,
            Err(e) => e.status,
        };
        offline::set_trying(false);
        match mode::after_retry(status) {
            mode::RetryOutcome::BackOnline => {
                toasts::toast(mode::BACK_ONLINE, "ok");
                rt::after(mode::BACK_ONLINE_RELOAD_MS, rt::reload);
            }
            mode::RetryOutcome::SignIn => rt::reload(),
            mode::RetryOutcome::StillOffline => toasts::toast(mode::STILL_OFFLINE, "warn"),
        }
    });
}

/// Retries on its own while offline mode is on (every [`mode::RETRY_MS`], and when the browser is back online).
fn retry_while_offline() {
    rt::every(mode::RETRY_MS, retry);
    rt::on_window("online", |_| retry());
}

// ---- bug reports -------------------------------------------------------------------------------------------

/// Opens the kit's New report sheet for the page at `from` (an app path); a sent report opens Bug reports.
pub fn new_report(from: &str) {
    let mut o = SheetOpts::from_config(from);
    o.on_sent = Some(Rc::new(|_| route::go(Route::BugReports, false)));
    bug_report::open(o);
}

/// A string field of a JSON object, "" when missing.
fn s(v: &Value, k: &str) -> String {
    v.get(k).and_then(Value::as_str).unwrap_or("").to_string()
}

/// "No replies", "1 reply", "3 replies".
pub fn replies(n: i64) -> String {
    match n {
        0 => "No replies".into(),
        1 => "1 reply".into(),
        n => format!("{n} replies"),
    }
}

/// A report's status as people read it.
pub fn status_label(status: &str) -> &'static str {
    match status {
        "looking" => "Looking into it",
        "fixed" => "Fixed",
        "wontfix" => "Won't fix",
        "answered" => "Answered",
        _ => "New",
    }
}

/// One report of the list: title, author, status and replies.
fn report_row(r: &Value) -> impl IntoView {
    let n = r.get("message_count").and_then(Value::as_i64).unwrap_or(0);
    let unread = r.get("unread").and_then(Value::as_bool).unwrap_or(false);
    view! {
        <div class="list-item bug-row" style="flex-wrap:wrap;gap:10px">
            <div class="grow" style="min-width:200px">
                <span class="bug-title" style="font-weight:650">{s(r, "title")}</span>
                <div class="small faint">{format!("by {}", s(r, "username"))}</div>
            </div>
            <span class="badge">{status_label(&s(r, "status"))}</span>
            <span class="small muted bug-count" class:accent=unread>{replies(n)}</span>
        </div>
    }
}

/// `/bug-reports`: the account's reports (an administrator's: everyone's), New report and, for
/// administrators, the export and the crash logs the apps sent.
#[component]
pub fn BugReportsPage(
    /// The account administers the server.
    admin: bool,
) -> impl IntoView {
    let (changed, set_changed) = signal(0u32);
    rt::on_window(&format!("{}:bugs-changed", crate::project()), move |_| set_changed.update(|n| *n += 1));
    let list = LocalResource::new(move || {
        changed.track();
        async { api::get("/bug-reports").await.map_err(|e| e.message) }
    });
    let rows = move || match list.get() {
        None => view! { <div class="empty"><span class="spinner"></span></div> }.into_any(),
        Some(Err(e)) => view! { <div class="error-box">{e}</div> }.into_any(),
        Some(Ok(v)) => {
            let reports = v.get("reports").and_then(Value::as_array).cloned().unwrap_or_default();
            if reports.is_empty() {
                view! { <div class="empty" id="bugs-empty" inner_html=format!(r#"{}<p class="muted">No reports yet. New report writes the first one.</p>"#, icon("bug"))></div> }.into_any()
            } else {
                view! { <div class="list">{reports.iter().map(report_row).collect_view()}</div> }.into_any()
            }
        }
    };
    let export = format!("{}/bug-reports/export?format=txt", kit_leptos::config::get().paths.api);
    view! {
        <div class="row wrap between" style="margin-bottom:18px;gap:10px">
            <h1 style="margin:0">"Bug reports"</h1>
            <div class="row wrap" style="gap:8px">
                <Show when=move || admin>
                    <a class="btn" id="bugs-export" href=export.clone() download="bug-reports.txt" inner_html=format!("{}Export as text", icon("download"))></a>
                </Show>
                <button class="btn primary" id="bugs-new" on:click=|_| new_report("/bug-reports") inner_html=format!("{}New report", icon("plus"))></button>
            </div>
        </div>
        <div class="card flat mt" id="bugs-list">{rows}</div>
        <Show when=move || admin>
            <CrashLogs/>
        </Show>
    }
}

/// The crash logs the apps sent (administrators), each opening in full with Copy.
#[component]
fn CrashLogs() -> impl IntoView {
    let list = LocalResource::new(|| async { api::get("/crash-reports").await.map_err(|e| e.message) });
    let rows = move || match list.get() {
        None => view! { <div class="empty"><span class="spinner"></span></div> }.into_any(),
        Some(Err(e)) => view! { <div class="error-box">{e}</div> }.into_any(),
        Some(Ok(v)) => {
            let crashes = v.get("crashes").and_then(Value::as_array).cloned().unwrap_or_default();
            if crashes.is_empty() {
                return view! { <div class="empty"><p class="muted">"No crash logs. Filebrary sends one here by itself when the web app breaks."</p></div> }.into_any();
            }
            crashes
                .iter()
                .map(|c| {
                    let (title, text) = (s(c, "first_line"), s(c, "text"));
                    let line = format!("{} · {} · {}", s(c, "user"), s(c, "device"), s(c, "app_version"));
                    view! {
                        <div class="list-item" style="flex-wrap:wrap;gap:10px">
                            <div class="grow ellipsis" style="min-width:200px" data-details="" title=text.clone()>
                                <span style="font-weight:650">{title}</span>
                                <div class="small faint">{line}</div>
                            </div>
                            <button class="btn sm" on:click=move |_| { details::show("Crash log", &text); }>"Open"</button>
                        </div>
                    }
                })
                .collect_view()
                .into_any()
        }
    };
    view! {
        <h2 class="mt">"Crash logs"</h2>
        <div class="card flat mt list" id="crash-list">{rows}</div>
    }
}

// ---- pages -------------------------------------------------------------------------------------------------

/// `/whats-new`: every release's notes, and the switch for the popup after an update.
#[component]
pub fn WhatsNewPage() -> impl IntoView {
    // the kit's page has its own heading
    view! { <div id="whats-new"><whats_new::ChangelogPage/></div> }
}

/// `/admin/diagnostics`: the server's checks with what's wrong and how to fix it, and Copy report.
#[component]
pub fn DiagnosticsPage() -> impl IntoView {
    view! { <div id="diagnostics"><kit_leptos::diagnostics::DiagnosticsPage/></div> }
}

/// `/admin/server`: the server's run-time settings, drawn from its schema (`/api/settings/schema`).
#[component]
pub fn ServerSettingsPage() -> impl IntoView {
    let data = LocalResource::new(|| async {
        let schema = api::get("/settings/schema").await.map_err(|e| e.message)?;
        let values = api::get("/settings").await.map_err(|e| e.message)?;
        Ok::<_, String>((Schema::from_json(&schema), values))
    });
    let body = move || match data.get() {
        None => view! { <div class="empty"><span class="spinner"></span></div> }.into_any(),
        Some(Err(e)) => view! { <div class="error-box">{e}</div> }.into_any(),
        Some(Ok((schema, values))) => schema
            .sections
            .iter()
            .map(|sec| {
                let cards = visible_cards(&sec.groups, &values);
                view! {
                    <section class="mt" data-section=sec.id.clone()>
                        <h2>{sec.title.clone()}</h2>
                        <SettingsCards cards=cards values=values.clone() reveal=true/>
                    </section>
                }
            })
            .collect_view()
            .into_any(),
    };
    view! {
        <h1>"Server settings"</h1>
        <p class="muted">"What the server's administrators can change while it runs. A setting locked by the server's environment says so."</p>
        <div id="server-settings">{body}</div>
    }
}

/// The kit's page for a route, if it is one ([`Kind`](crate::shell::Kind) `Kit`).
#[component]
pub fn KitPage(
    /// The route.
    #[prop(into)]
    route: Signal<Route>,
    /// The account administers the server.
    admin: bool,
) -> impl IntoView {
    move || match route.get() {
        Route::WhatsNew => view! { <WhatsNewPage/> }.into_any(),
        Route::BugReports => view! { <BugReportsPage admin=admin/> }.into_any(),
        Route::Admin { page } if !admin => {
            let _ = page;
            view! { <crate::pages::NotFound/> }.into_any()
        }
        Route::Admin { page } if page == "server" => view! { <ServerSettingsPage/> }.into_any(),
        Route::Admin { .. } => view! { <DiagnosticsPage/> }.into_any(),
        _ => view! { <crate::pages::NotFound/> }.into_any(),
    }
}

/// The help links under the sidebar: What's new and Bug reports.
#[component]
pub fn HelpLinks(
    /// The current route.
    #[prop(into)]
    route: Signal<Route>,
    /// Closes the phone drawer after a click.
    on_go: Callback<()>,
) -> impl IntoView {
    let link = move |to: Route, icon_name: &'static str, label: &'static str| {
        let href = url(&to.href());
        let go = on_link(to.clone());
        let active = move || route.get() == to;
        view! {
            <a href=href class:active=active.clone() aria-current=move || active().then_some("page") on:click=move |e| { go(e); on_go.run(()); }>
                <span inner_html=icon(icon_name)></span>
                <span class="ellipsis">{label}</span>
            </a>
        }
    };
    view! {
        <div class="nav-label">"Help"</div>
        {link(Route::WhatsNew, "refresh", "What's new")}
        {link(Route::BugReports, "bug", "Bug reports")}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn holds_nest() {
        assert!(!held(Hold::Upload));
        hold(Hold::Upload, true);
        hold(Hold::Upload, true);
        hold(Hold::Upload, false);
        assert!(held(Hold::Upload) && !held(Hold::Edit));
        hold(Hold::Upload, false);
        hold(Hold::Upload, false);
        assert!(!held(Hold::Upload));
    }

    #[test]
    fn the_kit_admin_pages() {
        assert!(is_admin_page("server") && is_admin_page("diagnostics"));
        assert!(!is_admin_page("users"));
    }

    #[test]
    fn report_words() {
        assert_eq!((replies(0).as_str(), replies(1).as_str(), replies(4).as_str()), ("No replies", "1 reply", "4 replies"));
        assert_eq!(status_label("wontfix"), "Won't fix");
        assert_eq!(status_label("anything"), "New");
    }

    #[test]
    fn notices_have_their_own_words() {
        let n = notices();
        assert_eq!(n.words_of("data").fixed, "The data folder can be written again");
        assert_eq!(n.words_of("sources").failing, "Some sources still can't be reached");
    }
}
