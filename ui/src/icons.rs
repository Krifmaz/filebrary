//! Filebrary's icons, drawn the kit's way (stroked 24 × 24 outlines in the text colour), for every page. Names the
//! kit has ([`kit_leptos::icons::paths`]) come from the kit; these are the ones it does not have yet.

/// The paths of Filebrary icon `name` ("" when there is none).
fn own(name: &str) -> &'static str {
    match name {
        "home" => {
            r#"<rect x="3" y="3" width="7" height="7" rx="1.5"/><rect x="14" y="3" width="7" height="7" rx="1.5"/><rect x="3" y="14" width="7" height="7" rx="1.5"/><rect x="14" y="14" width="7" height="7" rx="1.5"/>"#
        }
        "folder" => r#"<path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/>"#,
        "drive" => r#"<rect x="2" y="13" width="20" height="8" rx="2"/><path d="M5.5 13 8 4h8l2.5 9M6 17h.01M10 17h.01"/>"#,
        "star" => r#"<path d="m12 3 2.8 5.7 6.2.9-4.5 4.4 1.1 6.2L12 17.3 6.4 20.2l1.1-6.2L3 9.6l6.2-.9z"/>"#,
        "share" => {
            r#"<circle cx="18" cy="5" r="3"/><circle cx="6" cy="12" r="3"/><circle cx="18" cy="19" r="3"/><path d="m8.6 13.5 6.8 4M15.4 6.5l-6.8 4"/>"#
        }
        "collection" => r#"<path d="M4 6h16M6 10h12M8 14h8"/><rect x="3" y="14" width="18" height="7" rx="2"/>"#,
        "users" => r#"<circle cx="9" cy="8" r="4"/><path d="M2 21a7 7 0 0 1 14 0M16 3.1a4 4 0 0 1 0 7.8M22 21a7 7 0 0 0-4-6.3"/>"#,
        "user" => r#"<circle cx="12" cy="8" r="4"/><path d="M4 21a8 8 0 0 1 16 0"/>"#,
        "lock" => r#"<rect x="4" y="11" width="16" height="10" rx="2"/><path d="M8 11V7a4 4 0 0 1 8 0v4"/>"#,
        "palette" => {
            r#"<path d="M12 3a9 9 0 1 0 0 18c1 0 1.5-.7 1.5-1.5 0-.4-.2-.8-.4-1.1-.3-.3-.4-.7-.4-1.1 0-.8.7-1.5 1.5-1.5H16a5 5 0 0 0 5-5c0-4.4-4-7.8-9-7.8z"/><circle cx="7.5" cy="11" r="1"/><circle cx="10.5" cy="7.5" r="1"/><circle cx="15" cy="7.5" r="1"/>"#
        }
        "search" => r#"<circle cx="11" cy="11" r="7"/><path d="m20 20-3.5-3.5"/>"#,
        "menu" => r#"<path d="M4 6h16M4 12h16M4 18h16"/>"#,
        "logout" => r#"<path d="M9 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h4M16 17l5-5-5-5M21 12H9"/>"#,
        "upload" => r#"<path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/><path d="m17 8-5-5-5 5M12 3v12"/>"#,
        "folder-plus" => {
            r#"<path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/><path d="M12 10v6M9 13h6"/>"#
        }
        "file" => r#"<path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><path d="M14 2v6h6"/>"#,
        "file-text" => r#"<path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><path d="M14 2v6h6M8 13h8M8 17h6"/>"#,
        "file-image" => {
            r#"<rect x="3" y="3" width="18" height="18" rx="2"/><circle cx="8.5" cy="8.5" r="1.5"/><path d="m21 15-5-5L5 21"/>"#
        }
        "file-video" => r#"<rect x="2" y="5" width="15" height="14" rx="2"/><path d="m22 8-5 4 5 4z"/>"#,
        "file-audio" => r#"<path d="M9 18V5l12-2v13"/><circle cx="6" cy="18" r="3"/><circle cx="18" cy="16" r="3"/>"#,
        "file-archive" => {
            r#"<path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><path d="M14 2v6h6M10 6h1M10 9h1M10 12h1M10 15h1v3h-1z"/>"#
        }
        "file-sheet" => r#"<rect x="3" y="3" width="18" height="18" rx="2"/><path d="M3 9h18M3 15h18M9 3v18"/>"#,
        "file-code" => {
            r#"<path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><path d="M14 2v6h6M10 13l-2 2 2 2M14 13l2 2-2 2"/>"#
        }
        "edit" => r#"<path d="M12 20h9"/><path d="M16.5 3.5a2.1 2.1 0 0 1 3 3L7 19l-4 1 1-4z"/>"#,
        "move" => r#"<path d="M5 9l-3 3 3 3M9 5l3-3 3 3M15 19l-3 3-3-3M19 9l3 3-3 3M2 12h20M12 2v20"/>"#,
        "more" => r#"<circle cx="5" cy="12" r="1"/><circle cx="12" cy="12" r="1"/><circle cx="19" cy="12" r="1"/>"#,
        "more-v" => r#"<circle cx="12" cy="5" r="1"/><circle cx="12" cy="12" r="1"/><circle cx="12" cy="19" r="1"/>"#,
        "eye" => r#"<path d="M2 12s3.6-7 10-7 10 7 10 7-3.6 7-10 7S2 12 2 12z"/><circle cx="12" cy="12" r="3"/>"#,
        "play" => r#"<path d="m6 4 14 8-14 8z"/>"#,
        "pause" => r#"<path d="M7 4h3v16H7zM14 4h3v16h-3z"/>"#,
        "arrow-up" => r#"<path d="M12 19V5M5 12l7-7 7 7"/>"#,
        "arrow-down" => r#"<path d="M12 5v14M19 12l-7 7-7-7"/>"#,
        "chevron-right" => r#"<path d="m9 18 6-6-6-6"/>"#,
        "chevron-left" => r#"<path d="m15 18-6-6 6-6"/>"#,
        "list" => r#"<path d="M8 6h13M8 12h13M8 18h13M3 6h.01M3 12h.01M3 18h.01"/>"#,
        "compact" => r#"<path d="M3 5h7M3 10h7M3 15h7M3 20h7M14 5h7M14 10h7M14 15h7M14 20h7"/>"#,
        "grid-small" => {
            r#"<rect x="3" y="3" width="4" height="4" rx="1"/><rect x="10" y="3" width="4" height="4" rx="1"/><rect x="17" y="3" width="4" height="4" rx="1"/><rect x="3" y="10" width="4" height="4" rx="1"/><rect x="10" y="10" width="4" height="4" rx="1"/><rect x="17" y="10" width="4" height="4" rx="1"/><rect x="3" y="17" width="4" height="4" rx="1"/><rect x="10" y="17" width="4" height="4" rx="1"/><rect x="17" y="17" width="4" height="4" rx="1"/>"#
        }
        "grid" => {
            r#"<rect x="3" y="3" width="7" height="7" rx="1.5"/><rect x="14" y="3" width="7" height="7" rx="1.5"/><rect x="3" y="14" width="7" height="7" rx="1.5"/><rect x="14" y="14" width="7" height="7" rx="1.5"/>"#
        }
        "grid-large" => {
            r#"<rect x="3" y="3" width="18" height="11" rx="2"/><rect x="3" y="17" width="8" height="4" rx="1"/><rect x="13" y="17" width="8" height="4" rx="1"/>"#
        }
        "filter" => r#"<path d="M22 3H2l8 9.5V19l4 2v-8.5z"/>"#,
        "tag" => r#"<path d="M20.6 13.4 13.4 20.6a2 2 0 0 1-2.8 0L2 12V2h10l8.6 8.6a2 2 0 0 1 0 2.8z"/><circle cx="7" cy="7" r="1.5"/>"#,
        "restore" => r#"<path d="M3 12a9 9 0 1 0 3-6.7L3 8"/><path d="M3 3v5h5"/>"#,
        "comment" => r#"<path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"/>"#,
        "cast" => {
            r#"<path d="M2 16.1A5 5 0 0 1 5.9 20M2 12.05A9 9 0 0 1 9.95 20M2 8V6a2 2 0 0 1 2-2h16a2 2 0 0 1 2 2v12a2 2 0 0 1-2 2h-6"/><path d="M2 20h.01"/>"#
        }
        "save" => r#"<path d="M19 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h11l5 5v11a2 2 0 0 1-2 2z"/><path d="M17 21v-8H7v8M7 3v5h8"/>"#,
        "wrap" => r#"<path d="M3 6h18M3 12h15a3 3 0 1 1 0 6h-4M3 18h7"/><path d="m16 16-2 2 2 2"/>"#,
        "star-filled" => r#"<path fill="currentColor" d="m12 3 2.8 5.7 6.2.9-4.5 4.4 1.1 6.2L12 17.3 6.4 20.2l1.1-6.2L3 9.6l6.2-.9z"/>"#,
        "check-circle" => r#"<circle cx="12" cy="12" r="10"/><path d="m8 12 3 3 5-6"/>"#,
        "link" => {
            r#"<path d="M10 13a5 5 0 0 0 7.5.5l3-3a5 5 0 0 0-7-7l-1.7 1.7"/><path d="M14 11a5 5 0 0 0-7.5-.5l-3 3a5 5 0 0 0 7 7l1.7-1.7"/>"#
        }
        "inbox" => {
            r#"<path d="M22 12h-6l-2 3h-4l-2-3H2"/><path d="M5.5 5.1 2 12v6a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2v-6l-3.5-6.9A2 2 0 0 0 16.8 4H7.2a2 2 0 0 0-1.7 1.1z"/>"#
        }
        "image" => r#"<rect x="3" y="3" width="18" height="18" rx="2"/><circle cx="9" cy="9" r="2"/><path d="m21 15-5-5L5 21"/>"#,
        "video" => r#"<rect x="2" y="5" width="14" height="14" rx="2"/><path d="m22 8-6 4 6 4z"/>"#,
        "audio" => r#"<path d="M9 18V5l12-2v13"/><circle cx="6" cy="18" r="3"/><circle cx="18" cy="16" r="3"/>"#,
        "clock" => r#"<circle cx="12" cy="12" r="9"/><path d="M12 7v5l3 2"/>"#,
        "zip" => r#"<path d="M14 3H6a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V9z"/><path d="M10 3v2M10 7v2M10 11v2M9 15h2v3H9z"/>"#,
        "document" => r#"<path d="M14 3H6a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V9z"/><path d="M14 3v6h6M8 13h8M8 17h5"/>"#,
        "code" => r#"<path d="m16 18 6-6-6-6M8 6l-6 6 6 6"/>"#,
        "archive" => r#"<rect x="3" y="3" width="18" height="5" rx="1"/><path d="M5 8v11a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8M10 12h4"/>"#,
        _ => "",
    }
}

/// The `<svg>` of icon `name`: Filebrary's own, else the kit's.
pub fn icon(name: &str) -> String {
    let p = own(name);
    if p.is_empty() {
        return kit_leptos::icons::icon(name);
    }
    format!(
        r#"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">{p}</svg>"#
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn every_icon_the_pages_use_draws() {
        for n in [
            "home",
            "folder",
            "drive",
            "star",
            "share",
            "collection",
            "users",
            "user",
            "lock",
            "palette",
            "search",
            "menu",
            "logout",
            "settings",
            "bug",
            "upload",
            "folder-plus",
            "file",
            "file-text",
            "file-image",
            "file-video",
            "file-audio",
            "file-archive",
            "file-sheet",
            "file-code",
            "edit",
            "move",
            "more",
            "more-v",
            "eye",
            "play",
            "pause",
            "arrow-up",
            "arrow-down",
            "chevron-right",
            "chevron-left",
            "list",
            "compact",
            "grid-small",
            "grid",
            "grid-large",
            "filter",
            "tag",
            "restore",
            "comment",
            "cast",
            "save",
            "wrap",
            "star-filled",
            "check-circle",
            "x",
            "trash",
            "download",
            "copy",
            "refresh",
            "check",
            "alert",
            "external",
            "plus",
            "link",
            "inbox",
            "image",
            "video",
            "audio",
            "clock",
            "zip",
            "document",
            "code",
            "archive",
        ] {
            let svg = super::icon(n);
            assert!(svg.starts_with("<svg") && svg.len() > 120, "{n}");
        }
    }
}
