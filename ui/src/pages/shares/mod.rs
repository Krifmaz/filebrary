//! Sharing: the account's links (`/shares`), the dialog that makes and changes a link, and the page a visitor
//! of a link sees (`/s/<token>/<path…>`), without an account.
//!
//! | File | What |
//! | --- | --- |
//! | `api.rs` | the share API as the server answers it, and drop uploads with progress |
//! | `format.rs` | words, sizes, dates, expiry choices, paths inside a share |
//! | `list.rs` | the Shared page |
//! | `dialog.rs` | the share dialog ([`share_dialog`], [`share_collection_dialog`]) and the change dialog |
//! | `public.rs` | the visitor's page: password, folder browsing, previews, downloads, the drop box |
//! | `style.rs` | this folder's styles, put in the page once |
//!
//! Other pages open the dialog with [`share_dialog`] (files and folders of a source) or
//! [`share_collection_dialog`] (a whole collection).

mod api;
mod dialog;
mod format;
mod list;
mod public;
mod style;

#[allow(unused_imports)]
pub use dialog::{share_collection_dialog, share_dialog};
pub use list::SharesPage;
pub use public::PublicSharePage;

/// The icons these pages use that neither the kit nor `crate::icons` has yet.
fn own_icon(name: &str) -> &'static str {
    match name {
        "link" => {
            r#"<path d="M10 13a5 5 0 0 0 7.5.5l3-3a5 5 0 0 0-7-7l-1.7 1.7"/><path d="M14 11a5 5 0 0 0-7.5-.5l-3 3a5 5 0 0 0 7 7l1.7-1.7"/>"#
        }
        "upload" => r#"<path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4M17 8l-5-5-5 5M12 3v12"/>"#,
        "inbox" => {
            r#"<path d="M22 12h-6l-2 3h-4l-2-3H2"/><path d="M5.5 5.1 2 12v6a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2v-6l-3.5-6.9A2 2 0 0 0 16.8 4H7.2a2 2 0 0 0-1.7 1.1z"/>"#
        }
        "edit" => r#"<path d="M12 20h9"/><path d="M16.5 3.5a2.1 2.1 0 0 1 3 3L7 19l-4 1 1-4z"/>"#,
        "eye" => r#"<path d="M2 12s3.5-7 10-7 10 7 10 7-3.5 7-10 7S2 12 2 12z"/><circle cx="12" cy="12" r="3"/>"#,
        "file" => r#"<path d="M14 3H6a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V9z"/><path d="M14 3v6h6"/>"#,
        "image" => r#"<rect x="3" y="3" width="18" height="18" rx="2"/><circle cx="9" cy="9" r="2"/><path d="m21 15-5-5L5 21"/>"#,
        "video" => r#"<rect x="2" y="5" width="14" height="14" rx="2"/><path d="m22 8-6 4 6 4z"/>"#,
        "audio" => r#"<path d="M9 18V5l12-2v13"/><circle cx="6" cy="18" r="3"/><circle cx="18" cy="16" r="3"/>"#,
        "clock" => r#"<circle cx="12" cy="12" r="9"/><path d="M12 7v5l3 2"/>"#,
        "zip" => r#"<path d="M14 3H6a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V9z"/><path d="M10 3v2M10 7v2M10 11v2M9 15h2v3H9z"/>"#,
        _ => "",
    }
}

/// The `<svg>` of icon `name`: these pages' own, else Filebrary's, else the kit's.
pub(crate) fn icon(name: &str) -> String {
    let p = own_icon(name);
    if p.is_empty() {
        return crate::icons::icon(name);
    }
    format!(
        r#"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">{p}</svg>"#
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn every_icon_draws() {
        for n in [
            "link", "upload", "inbox", "edit", "eye", "file", "image", "video", "audio", "clock", "zip", "folder", "lock", "share", "copy",
            "trash", "download", "external", "check", "x", "alert", "refresh",
        ] {
            let svg = super::icon(n);
            assert!(svg.starts_with("<svg") && svg.len() > 150, "{n}");
        }
    }
}
