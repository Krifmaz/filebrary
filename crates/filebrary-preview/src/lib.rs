//! Thumbnails and previews for Filebrary.
//!
//! # What gets a preview, and how
//!
//! | Kind | Thumbnail | Large preview | Needs |
//! |------|-----------|---------------|-------|
//! | JPEG, PNG, GIF, WebP, BMP, ICO | resized in Rust | the original | nothing |
//! | TIFF | resized in Rust | converted in Rust | nothing |
//! | Camera RAW | embedded JPEG, in Rust | same, larger | nothing |
//! | HEIC, HEIF, AVIF | decoded, resized | decoded, resized | `heif-dec` or `ffmpeg` |
//! | Video | poster frame at 10% | poster frame | `ffmpeg` (+ `ffprobe`) |
//! | Audio | embedded cover art | cover art | `ffmpeg`, `ffprobe` |
//! | PDF | first page | the original (browser viewer) | `pdftoppm` |
//! | Office, OpenDocument | first page of the PDF | PDF via `variant=pdf` | `soffice`, `pdftoppm` |
//!
//! The slim Docker image ships none of the external tools, so it previews
//! images and RAW files only; the `-full` image adds the rest. Tools are
//! found on `PATH` at startup ([`Tools::detect`]).
//!
//! Video and audio the browser cannot play natively (HEVC in MKV, WMA, ...)
//! can be streamed through [`PreviewService::transcode`]; everything else is
//! played straight from the download route, where HTTP range requests give
//! seeking for free.
//!
//! # Safety
//!
//! * External programs run with an argument vector, never through a shell,
//!   with a time limit, no stdin, and are killed when the request goes away.
//! * Inputs are absolute paths that the storage layer has already resolved
//!   and checked against the source root; `ffmpeg` inputs carry a `file:`
//!   prefix so file names cannot select another protocol.
//! * In-process decoding has dimension and memory limits against image bombs,
//!   and per-kind file size limits.
//! * LibreOffice runs with a fresh, throwaway profile for each conversion.

#![warn(missing_docs)]

pub mod cache;
pub mod document;
pub mod error;
pub mod kind;
pub mod media;
pub mod raster;
pub mod service;
pub mod tools;

pub use cache::{CacheKey, CachedFile, PreviewCache};
pub use error::{PreviewError, Result};
pub use kind::FileKind;
pub use media::{MediaInfo, TranscodeStream, TranscodeTarget};
pub use service::{PreviewConfig, PreviewInput, PreviewOutput, PreviewService, Variant};
pub use tools::Tools;
