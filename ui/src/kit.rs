//! Where Filebrary's UI meets the kit's features.
//!
//! This scaffold starts only what every page needs from the first commit: the kit's configuration (app id and
//! name), the API backend, toasts with Copy, the details dialog (cut-off text opens in full), drag and drop, and
//! the connection banner. What's new, self-update, notice banners, bug reports and crash logs, Diagnostics,
//! schema settings and offline mode land here in their own change, once the server mounts the kit's endpoints.

use kit_leptos::{details, dropzone, offline, rt, toasts};

/// The kit's configuration for Filebrary: id `filebrary`, the name people see, and the script stem
/// (`filebrary-web`) whose hash names the web build.
fn config(app: &str) -> kit_leptos::config::AppConfig {
    let mut cfg = kit_leptos::config::AppConfig::new(kit_rules::app::App::new(crate::project(), app));
    cfg.script = crate::NAME.to_string();
    cfg
}

/// Starts the kit's parts that live outside any page. Call once, before anything is drawn.
pub fn start() {
    let app = crate::app_name();
    kit_leptos::config::set(config(&app));
    kit_leptos::api::set_backend(crate::api::backend());
    toasts::mount();
    details::init();
    dropzone::init();
    watch_connection(&app);
}

/// Starts the kit's parts that need a signed-in account (`admin`: the account administers the server).
pub fn signed_in(_admin: bool) {}

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
