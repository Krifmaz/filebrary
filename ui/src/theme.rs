//! The kit theme on `<html data-theme>`: `encore` (the default, charcoal and cream), `dark`, `oled` or `light`,
//! or `system` (light or dark from the device). Kept per browser so it applies before signing in; the
//! Appearance settings page changes it.

/// The themes people can pick, with their names, in order.
pub const THEMES: [(&str, &str); 5] =
    [("encore", "Encore"), ("dark", "Dark"), ("oled", "Black (OLED)"), ("light", "Light"), ("system", "Follow the device")];

/// Where the choice is kept in `localStorage`.
pub const KEY: &str = "filebrary.theme";

/// A known theme id, else the default `encore` (the old `custom` included).
pub fn normalize(id: &str) -> &'static str {
    THEMES.iter().map(|(id, _)| *id).find(|t| *t == id).unwrap_or("encore")
}

/// The theme `id` draws with: `system` follows the device's light or dark setting.
pub fn resolve(id: &str, prefers_light: bool) -> &'static str {
    match normalize(id) {
        "system" if prefers_light => "light",
        "system" => "dark",
        t => t,
    }
}

fn storage() -> Option<web_sys::Storage> {
    kit_leptos::rt::window().and_then(|w| w.local_storage().ok().flatten())
}

/// The saved choice (`encore` when nothing is saved).
pub fn saved() -> String {
    normalize(&storage().and_then(|s| s.get_item(KEY).ok().flatten()).unwrap_or_default()).to_string()
}

/// The theme after `id` in [`THEMES`] (the top bar's theme button steps through them).
pub fn next(id: &str) -> &'static str {
    let i = THEMES.iter().position(|(t, _)| *t == normalize(id)).unwrap_or(0);
    THEMES[(i + 1) % THEMES.len()].0
}

/// The name people read for theme `id`.
pub fn label(id: &str) -> &'static str {
    THEMES.iter().find(|(t, _)| *t == normalize(id)).map(|(_, l)| *l).unwrap_or("Encore")
}

/// Draws with `id` and remembers it.
pub fn set(id: &str) {
    if let Some(s) = storage() {
        let _ = s.set_item(KEY, normalize(id));
    }
    apply(id.to_string());
}

/// Draws with `id` without saving it.
pub fn apply(id: String) {
    let light =
        kit_leptos::rt::window().and_then(|w| w.match_media("(prefers-color-scheme: light)").ok().flatten()).is_some_and(|m| m.matches());
    if let Some(root) = kit_leptos::rt::document().and_then(|d| d.document_element()) {
        let _ = root.set_attribute("data-theme", resolve(&id, light));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_themes_fall_back_to_encore() {
        assert_eq!(normalize("custom"), "encore");
        assert_eq!(normalize(""), "encore");
        assert_eq!(normalize("oled"), "oled");
    }

    #[test]
    fn the_button_steps_through_every_theme() {
        assert_eq!(next("encore"), "dark");
        assert_eq!(next("system"), "encore");
        assert_eq!(label("oled"), "Black (OLED)");
    }

    #[test]
    fn system_follows_the_device() {
        assert_eq!(resolve("system", true), "light");
        assert_eq!(resolve("system", false), "dark");
        assert_eq!(resolve("encore", true), "encore");
    }
}
