//! Filebrary's icons, drawn the kit's way (stroked 24 × 24 outlines in the text colour). Names the kit has
//! ([`kit_leptos::icons::paths`]) come from the kit; these are the ones it does not have yet.

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
    fn every_nav_icon_draws() {
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
        ] {
            let svg = super::icon(n);
            assert!(svg.starts_with("<svg") && svg.len() > 150, "{n}");
        }
    }
}
