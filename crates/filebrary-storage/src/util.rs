//! Small helpers shared by the backends, so that every backend applies the

// Only the remote backends use these; a build without them has nothing to share.
#![cfg_attr(
    not(any(feature = "s3", feature = "sftp", feature = "smb")),
    allow(dead_code)
)]
//! same rules to the same edge cases.

use std::{io, ops::Range};

use bytes::{Bytes, BytesMut};
use tokio::io::AsyncReadExt;

use crate::{ByteReader, StorageError, StorageResult, VPath};

/// Suffix of the temporary files used for atomic writes. Names ending in it
/// and starting with `.` are hidden from listings.
pub(crate) const TEMP_SUFFIX: &str = ".fbtmp";

/// Returns a hidden temporary name next to `name`, for write-then-rename.
pub(crate) fn temp_name(name: &str) -> String {
    // Keep the temp name short enough for 255-byte limits.
    let short: String = name.chars().take(64).collect();
    format!(".{short}.{}{TEMP_SUFFIX}", uuid::Uuid::new_v4().simple())
}

/// Returns `true` for the temporary files of in-flight atomic writes.
pub(crate) fn is_temp_name(name: &str) -> bool {
    name.starts_with('.') && name.ends_with(TEMP_SUFFIX)
}

/// Refuses moving or copying a directory into itself.
pub(crate) fn refuse_into_self(from: &VPath, to: &VPath) -> StorageResult<()> {
    if to.starts_with(from) && to != from {
        return Err(StorageError::InvalidPath(
            "cannot move or copy a folder into itself".into(),
        ));
    }
    Ok(())
}

/// Refuses an operation on the source root.
pub(crate) fn refuse_root(p: &VPath, what: &str) -> StorageResult<()> {
    if p.is_root() {
        return Err(StorageError::InvalidPath(format!(
            "cannot {what} the source root"
        )));
    }
    Ok(())
}

/// Checks a requested byte range against the file size.
pub(crate) fn check_range(range: &Range<u64>, size: u64) -> StorageResult<()> {
    if range.start > range.end || range.end > size {
        return Err(StorageError::InvalidRange { size });
    }
    Ok(())
}

/// Reads up to `max` bytes from `r`, stopping early only at end of input.
pub(crate) async fn read_chunk(r: &mut ByteReader, max: usize) -> io::Result<Bytes> {
    let mut buf = BytesMut::with_capacity(max);
    while buf.len() < max {
        // `read_buf` appends without zero-filling; cap it at `max`.
        let mut limited = (&mut *r).take((max - buf.len()) as u64);
        if limited.read_buf(&mut buf).await? == 0 {
            break;
        }
    }
    Ok(buf.freeze())
}

/// Maps an I/O error from a remote protocol to a storage error, keeping the
/// kinds the server distinguishes.
pub(crate) fn io_other(msg: impl std::fmt::Display) -> StorageError {
    StorageError::Io(io::Error::other(msg.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temp_names_are_hidden_and_short() {
        let long = "x".repeat(250);
        let t = temp_name(&long);
        assert!(is_temp_name(&t));
        assert!(t.len() < 120);
        assert!(!is_temp_name("file.fbtmp"));
        assert!(!is_temp_name(".hidden"));
    }

    #[tokio::test]
    async fn read_chunk_fills_up_to_max() {
        let data: Vec<u8> = (0..100u8).collect();
        let mut r: ByteReader = Box::pin(std::io::Cursor::new(data));
        assert_eq!(read_chunk(&mut r, 30).await.unwrap().len(), 30);
        assert_eq!(read_chunk(&mut r, 80).await.unwrap().len(), 70);
        assert!(read_chunk(&mut r, 10).await.unwrap().is_empty());
    }

    #[test]
    fn ranges() {
        assert!(check_range(&(0..10), 10).is_ok());
        assert!(check_range(&(10..10), 10).is_ok());
        assert!(check_range(&(0..11), 10).is_err());
        #[allow(clippy::reversed_empty_ranges)]
        let r = 5..4;
        assert!(check_range(&r, 10).is_err());
    }
}
