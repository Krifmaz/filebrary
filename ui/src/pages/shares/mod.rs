//! Sharing: the account's links (`/shares`), the dialog that makes and changes a link, and the page a visitor
//! of a link sees (`/s/<token>/<path…>`), without an account.
//!
//! | File | What |
//! | --- | --- |
//! | `api.rs` | the share API as the server answers it, and the raw request drop uploads use |
//! | `format.rs` | words, sizes, dates, expiry choices, paths inside a share |
//! | `list.rs` | the Shared page |
//! | `dialog.rs` | the share dialog ([`share_dialog`], [`share_collection_dialog`]) and the change dialog |
//! | `public.rs` | the visitor's page: password, folder browsing, previews, downloads, the drop box |
//! | `upload.rs` | drop-link uploads: chunked, resumable after a dropped connection or a reload |
//!
//! Other pages open the dialog with [`share_dialog`] (files and folders of a source) or
//! [`share_collection_dialog`] (a whole collection). The files browser asks for it with the window event
//! [`crate::pages::files::SHARE_EVENT`], which [`answer_share_requests`] answers.

mod api;
mod dialog;
mod format;
mod list;
mod public;
mod upload;

#[allow(unused_imports)]
pub use dialog::{share_collection_dialog, share_dialog};
pub use list::SharesPage;
pub use public::PublicSharePage;

use serde::Deserialize;
use wasm_bindgen::JsCast;

/// What the files browser asks to share (the detail of [`crate::pages::files::SHARE_EVENT`]).
#[derive(Debug, PartialEq, Deserialize)]
struct ShareRequest {
    source: String,
    paths: Vec<String>,
    #[serde(default)]
    single_folder: bool,
}

/// Reads a share request from the event's detail, as JSON.
fn share_request(detail: &str) -> Option<ShareRequest> {
    serde_json::from_str::<ShareRequest>(detail).ok().filter(|r| !r.source.is_empty() && !r.paths.is_empty())
}

thread_local! {
    static ANSWERING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Opens the share dialog whenever the files browser asks for it (once per page; later calls do nothing).
pub fn answer_share_requests() {
    if ANSWERING.with(|a| a.replace(true)) {
        return;
    }
    kit_leptos::rt::on_window(crate::pages::files::SHARE_EVENT, |e| {
        let Some(ev) = e.dyn_ref::<web_sys::CustomEvent>() else { return };
        let Some(detail) = js_sys::JSON::stringify(&ev.detail()).ok().and_then(|s| s.as_string()) else { return };
        if let Some(r) = share_request(&detail) {
            share_dialog(&r.source, r.paths, r.single_folder);
        }
    });
}

/// The icons, for the share pages and the dialog.
pub(crate) use crate::icons::icon;

#[cfg(test)]
mod tests {
    #[test]
    fn share_requests_are_read() {
        let r = super::share_request(r#"{"source":"home","paths":["/Photos"],"single_folder":true}"#).unwrap();
        assert_eq!((r.source.as_str(), r.paths.as_slice(), r.single_folder), ("home", &["/Photos".to_string()][..], true));
        assert!(super::share_request(r#"{"source":"home","paths":[]}"#).is_none());
        assert!(super::share_request("null").is_none());
    }
}
