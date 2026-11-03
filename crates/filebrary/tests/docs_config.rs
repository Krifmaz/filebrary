//! Keeps the settings reference in `docs/src/configuration/reference.md` in sync with
//! [`filebrary::config::Config`].
//!
//! The reference documents each setting under a heading whose text is the
//! setting's key in backticks, for example:
//!
//! ```markdown
//! ### `listen`
//! ### `sources[].name`
//! ### `search.content.enabled`
//! ```
//!
//! Nested tables use dots, and arrays of tables (`[[sources]]`) use `[]`.
//! These tests fail when:
//!
//! * a field of `Config` (or of any struct it contains) has no heading,
//! * a heading names a key that `Config` does not have (a heading for a
//!   table itself, like ``### `sources` ``, is allowed),
//! * a `FILEBRARY_*` variable read by the server is not mentioned in the docs,
//! * `docs/src/configuration/filebrary.example.toml` no longer parses, or
//!   leaves out a setting (it may be commented out).
//!
//! The field list comes from tracing `Config`'s `Deserialize` impl with
//! `serde-reflection`, so serde renames are respected. That tracer cannot see
//! through `deserialize_any` (used by `#[serde(untagged)]`,
//! `#[serde(tag = "...")]` and `#[serde(flatten)]`). Keep config types plain
//! structs and unit-variant enums, or the trace below reports an error.

use std::collections::BTreeSet;

use filebrary::config::Config;
use serde_reflection::{ContainerFormat, Format, Registry, Tracer, TracerConfig};

/// The settings reference, relative to this crate.
const REFERENCE: &str = include_str!("../../../docs/src/configuration/reference.md");
/// The annotated example config shipped with the docs.
const EXAMPLE: &str = include_str!("../../../docs/src/configuration/filebrary.example.toml");
/// Every docs page that may mention environment variables.
const ENV_DOCS: &[&str] = &[
    REFERENCE,
    include_str!("../../../docs/src/configuration/overview.md"),
    include_str!("../../../docs/src/install/docker.md"),
    include_str!("../../../docs/src/operations/monitoring.md"),
];
/// Source files that read `FILEBRARY_*` variables.
const ENV_SOURCES: &[&str] = &[
    include_str!("../src/config.rs"),
    include_str!("../src/main.rs"),
    include_str!("../src/jobs/config.rs"),
];

/// Returns every setting key of `Config`, in the docs' dotted notation.
fn config_keys() -> BTreeSet<String> {
    let mut tracer = Tracer::new(TracerConfig::default());
    let (root, _) = tracer
        .trace_simple_type::<Config>()
        .expect("tracing Config failed; see the module docs for supported serde attributes");
    // Enum variants are not needed, only struct fields, so a partly traced
    // enum is fine here.
    let registry = tracer.registry_unchecked();
    let mut keys = BTreeSet::new();
    collect(&registry, &root, "", &mut keys);
    keys
}

/// Adds the keys reachable from `format` under `prefix` to `out`.
fn collect(registry: &Registry, format: &Format, prefix: &str, out: &mut BTreeSet<String>) {
    match format {
        Format::Option(inner) => collect(registry, inner, prefix, out),
        Format::Seq(inner) => collect(registry, inner, &format!("{prefix}[]"), out),
        Format::Map { value, .. } => collect(registry, value, &format!("{prefix}.<name>"), out),
        Format::TypeName(name) => match registry.get(name) {
            Some(ContainerFormat::Struct(fields)) => {
                for field in fields {
                    let key = if prefix.is_empty() {
                        field.name.clone()
                    } else {
                        format!("{prefix}.{}", field.name)
                    };
                    let before = out.len();
                    collect(registry, &field.value, &key, out);
                    // A field with no nested settings is a setting itself.
                    // A table (struct) field documents its own children.
                    if out.len() == before {
                        out.insert(key);
                    }
                }
            }
            Some(ContainerFormat::NewTypeStruct(inner)) => collect(registry, inner, prefix, out),
            _ => {}
        },
        _ => {}
    }
}

/// Returns every key documented with a ``### `key` `` heading.
fn documented_keys() -> BTreeSet<String> {
    REFERENCE
        .lines()
        .filter_map(|line| {
            let rest = line.trim_start_matches('#');
            if rest.len() == line.len() || !rest.starts_with(' ') {
                return None;
            }
            let rest = rest.trim();
            let key = rest.strip_prefix('`')?.strip_suffix('`')?;
            Some(key.to_owned())
        })
        .collect()
}

#[test]
fn every_setting_is_documented() {
    let keys = config_keys();
    assert!(keys.contains("listen"), "trace looks wrong: {keys:?}");
    let documented = documented_keys();
    let missing: Vec<_> = keys.difference(&documented).collect();
    assert!(
        missing.is_empty(),
        "settings missing from docs/src/configuration/reference.md (add a ### `key` heading for each): {missing:?}"
    );
}

#[test]
fn every_documented_setting_exists() {
    let mut known = config_keys();
    // A table may also have its own heading (`### `sources``) introducing
    // its keys, so every prefix of a key is allowed too.
    for key in known.clone() {
        for (i, c) in key.char_indices() {
            if c == '.' || c == '[' {
                known.insert(key[..i].to_owned());
            }
        }
    }
    let stale: Vec<_> = documented_keys().difference(&known).cloned().collect();
    assert!(
        stale.is_empty(),
        "docs/src/configuration/reference.md documents settings that Config does not have: {stale:?}"
    );
}

#[test]
fn every_env_var_is_documented() {
    let mut vars = BTreeSet::new();
    for src in ENV_SOURCES {
        let mut rest: &str = src;
        while let Some(i) = rest.find("\"FILEBRARY_") {
            let tail = &rest[i + 1..];
            let end = tail
                .find(|c: char| !(c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_'))
                .unwrap_or(tail.len());
            vars.insert(&tail[..end]);
            rest = &tail[end..];
        }
    }
    // `clap`'s `env = "..."` attributes are string literals too, so
    // `FILEBRARY_CONFIG` is picked up the same way.
    assert!(
        vars.contains("FILEBRARY_LISTEN"),
        "scan looks wrong: {vars:?}"
    );
    let missing: Vec<_> = vars
        .iter()
        .filter(|v| !ENV_DOCS.iter().any(|d| d.contains(&format!("`{v}`"))))
        .collect();
    assert!(
        missing.is_empty(),
        "environment variables not documented (mention each in backticks): {missing:?}"
    );
}

#[test]
fn example_config_mentions_every_setting() {
    let missing: Vec<_> = config_keys()
        .into_iter()
        .filter(|key| {
            let leaf = key.rsplit(['.', ']']).next().unwrap_or(key);
            !EXAMPLE.lines().any(|l| {
                let l = l.trim_start_matches(['#', ' ']);
                l.strip_prefix(leaf)
                    .is_some_and(|r| r.trim_start().starts_with('='))
            })
        })
        .collect();
    assert!(
        missing.is_empty(),
        "settings missing from docs/src/configuration/filebrary.example.toml \
         (add `key = value`, commented out if it has no default): {missing:?}"
    );
}

#[test]
fn example_config_parses() {
    let cfg: Config =
        toml::from_str(EXAMPLE).expect("docs/src/configuration/filebrary.example.toml is invalid");
    cfg.validate()
        .expect("docs/src/configuration/filebrary.example.toml fails validation");
}
