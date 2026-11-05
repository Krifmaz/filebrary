//! Build script: embeds the release notes (`docs/releases/<MAJOR.MINOR.PATCH>.md`
//! at the repository root) for the kit's What's new and changelog routes.

fn main() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/releases");
    kit_server_changelog::build::embed(&dir, "changelog_gen.rs");
    println!("cargo:rerun-if-env-changed=APP_VERSION");
}
