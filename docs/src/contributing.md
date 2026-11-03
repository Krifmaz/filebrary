# Contributing

Pull requests are welcome. Before opening one:

- Run the checks listed in [Building from source](install/source.md#running-the-checks).
- Add tests for new behaviour (unit tests, plus integration tests against a
  temporary directory, and against MinIO for S3).
- Document every public item with doc comments, and every new setting in
  these docs (see below).
- Never pass user input through a shell.
- Ask before adding a dependency that needs a C library outside the
  conversion (`-full`) image.

CI runs formatting, clippy, tests and the web build on Linux, macOS and
Windows, builds the Docker image and checks its size, and builds these docs.

Contributions are accepted under the Apache License, version 2.0.

## Keeping the settings reference in sync

The [settings reference](configuration/reference.md) is checked against the
`Config` struct in `crates/filebrary/src/config.rs` by
`crates/filebrary/tests/docs_config.rs`, which runs with `cargo test`. It
fails when:

* a field of `Config`, or of any struct inside it, has no heading in the
  reference;
* the reference has a heading for a key that no longer exists;
* a `FILEBRARY_*` variable read in `config.rs` or `main.rs` is not mentioned
  (in backticks) in the reference, the configuration overview, the Docker
  page or the monitoring page;
* `docs/src/configuration/filebrary.example.toml` does not parse or fails
  validation.

When you add a setting:

1. Add the field with a doc comment in `config.rs` (and its default in
   `impl Default`).
2. Add a heading for it in `docs/src/configuration/reference.md`, under the
   right group, using the key in backticks. Nested tables use dots and
   arrays of tables use `[]`:

   ````markdown
   ### `search.rescan_minutes`

   How often network sources are rescanned, in minutes.

   |             |                                 |
   |-------------|---------------------------------|
   | Type        | integer                         |
   | Default     | `60`                            |
   | Environment | `FILEBRARY_SEARCH_RESCAN_MINUTES` |
   ````

   A heading for the table itself (`### `search``) is allowed and is a good
   place to introduce the section.
3. Add it, with its default, to `filebrary.example.toml`.
4. If it has an environment variable, list it in the table.

The key list comes from tracing `Config`'s `Deserialize` implementation, so
`#[serde(rename)]` is respected. The tracer cannot follow `deserialize_any`,
which `#[serde(untagged)]`, internally tagged enums (`#[serde(tag = "...")]`)
and `#[serde(flatten)]` use. Keep configuration types to plain structs and
enums of unit variants (for a per-backend table, use an optional
sub-table per backend, such as `sources[].s3`).

## Building these docs

```sh
cargo install mdbook --locked
mdbook serve docs --open    # live preview on http://localhost:3000
mdbook build docs           # output in docs/book
```

Pull requests build the book in CI. Pushes to `main` that touch `docs/`
publish it to GitHub Pages through `.github/workflows/docs.yml`.
