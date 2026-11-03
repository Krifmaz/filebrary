//! Full-text content search for Filebrary.
//!
//! This crate turns documents into searchable text and keeps a
//! [tantivy](https://docs.rs/tantivy) index of it:
//!
//! * [`extract`] pulls plain text out of text/code files, PDF, Word,
//!   PowerPoint, Excel and OpenDocument files, in pure Rust, with size limits
//!   and panic containment so hostile files cannot hurt the server.
//! * [`ContentIndex`] stores the text per `(source, path)`, answers queries
//!   with ranked [`Hit`]s and HTML-escaped snippets, and handles removal and
//!   renames of whole directory trees.
//! * [`worker`] keeps the index in step with the file-name indexer: it
//!   consumes [`worker::ContentEvent`]s, reads changed files through a
//!   [`worker::ContentFetcher`] (the server's storage layer) and commits in
//!   batches. Content search is switched per source by the fetcher's
//!   [`worker::ContentFetcher::enabled`].
//!
//! No external programs are run and no input goes through a shell.

#![warn(missing_docs)]

pub mod extract;
pub mod index;
pub mod worker;

pub use extract::{DocKind, ExtractError, detect_kind, extract_text};
pub use index::{ContentIndex, FulltextConfig, Hit, IndexError, IndexResult};
