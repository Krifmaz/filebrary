//! Optional FTP and FTPS server for Filebrary.
//!
//! The server is off by default. When an administrator turns it on, users
//! log in with the same accounts and passwords as the web UI and see the same
//! sources with the same permissions:
//!
//! ```text
//! /                  one folder per source the user may view
//! /files/...         the "files" source, inside the user's scope
//! /media/...
//! ```
//!
//! This crate does not know about users or the database. The server crate
//! implements [`FtpAccess`] (check a password, load an account) and
//! [`FtpAccount`] (map an operation on a source path to a storage backend
//! and the real path, or refuse it). Every path a client sends is normalised
//! in [`backend`] and then validated by [`filebrary_storage::VPath::parse`]
//! before any storage call, so `..`, absolute paths and odd separators cannot
//! leave the user's scope. Nothing is ever run through a shell.
//!
//! Protocol handling is done by [libunftp](https://docs.rs/libunftp), built
//! with rustls and the ring provider (no OpenSSL).

#![warn(missing_docs)]

mod access;
pub mod backend;
mod config;
mod server;

pub use access::{Denied, FtpAccess, FtpAccount, Op};
pub use config::{FtpConfig, FtpsConfig, FtpsMode};
pub use server::{FtpError, FtpServer};
