//! A behaviour suite every [`Storage`] backend must pass.
//!
//! The suite pins down the semantics the rest of the server relies on (error
//! kinds, overwrite rules, recursive copy and delete, byte ranges), so that a
//! local folder, a bucket, an SFTP server and an SMB share all behave the same
//! from the API's point of view. Backend integration tests call [`run`] with a
//! storage rooted at an empty, throw-away location.
//!
//! Enabled with the `conformance` feature; it is test support code, not part
//! of the server.

use std::ops::Range;

use bytes::Bytes;
use futures::TryStreamExt;

use crate::{ByteReader, Storage, StorageError, StorageResult, VPath};

fn vp(s: &str) -> VPath {
    VPath::parse(s).expect("test path is valid")
}

fn reader(data: Vec<u8>) -> ByteReader {
    Box::pin(std::io::Cursor::new(data))
}

/// Reads a whole file (or `range` of it) into memory.
///
/// # Errors
///
/// Whatever [`Storage::read`] or the returned stream reports.
pub async fn read_all(
    s: &dyn Storage,
    path: &str,
    range: Option<Range<u64>>,
) -> StorageResult<Vec<u8>> {
    let stream = s.read(&vp(path), range).await?;
    let chunks: Vec<Bytes> = stream.try_collect().await?;
    Ok(chunks.concat())
}

async fn names(s: &dyn Storage, path: &str) -> Vec<String> {
    let mut v: Vec<String> = s
        .list(&vp(path))
        .await
        .unwrap_or_else(|e| panic!("list {path}: {e}"))
        .into_iter()
        .map(|e| e.name)
        .collect();
    v.sort();
    v
}

macro_rules! assert_err {
    ($e:expr, $pat:pat) => {
        match $e {
            Err($pat) => {}
            other => panic!(
                "{} returned {:?}, expected {}",
                stringify!($e),
                other.map(|_| ()),
                stringify!($pat)
            ),
        }
    };
}

/// Runs every check against `s`, which must be rooted at an empty location.
///
/// # Panics
///
/// On the first behaviour that differs from the contract.
pub async fn run(s: &dyn Storage) {
    empty_root(s).await;
    write_read_stat(s).await;
    ranges(s).await;
    overwrite_rules(s).await;
    mkdir_rules(s).await;
    rename_rules(s).await;
    copy_rules(s).await;
    delete_rules(s).await;
    large_file(s).await;
    odd_names(s).await;
}

async fn empty_root(s: &dyn Storage) {
    assert!(s.list(&VPath::root()).await.unwrap().is_empty());
    let root = s.stat(&VPath::root()).await.unwrap();
    assert!(root.is_dir());
    assert_err!(s.stat(&vp("missing")).await, StorageError::NotFound);
    assert_err!(s.list(&vp("missing")).await, StorageError::NotFound);
}

async fn write_read_stat(s: &dyn Storage) {
    s.mkdir(&vp("wr/sub")).await.unwrap();
    let n = s
        .write(
            &vp("wr/sub/hello.txt"),
            reader(b"hello world".to_vec()),
            false,
        )
        .await
        .unwrap();
    assert_eq!(n, 11);
    assert_eq!(
        read_all(s, "wr/sub/hello.txt", None).await.unwrap(),
        b"hello world"
    );

    let st = s.stat(&vp("wr/sub/hello.txt")).await.unwrap();
    assert!(!st.is_dir());
    assert_eq!(st.size, 11);
    assert_eq!(st.name, "hello.txt");
    assert_eq!(st.path, vp("wr/sub/hello.txt"));
    assert_eq!(st.mime.as_deref(), Some("text/plain"));
    assert!(st.modified.is_some(), "files carry a modification time");

    assert!(s.stat(&vp("wr")).await.unwrap().is_dir());
    assert!(s.stat(&vp("wr/sub")).await.unwrap().is_dir());
    assert_eq!(names(s, "").await, ["wr"]);
    assert_eq!(names(s, "wr").await, ["sub"]);
    let listed = s.list(&vp("wr/sub")).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].name, "hello.txt");
    assert_eq!(listed[0].path, vp("wr/sub/hello.txt"));
    assert_eq!(listed[0].size, 11);
    assert!(!listed[0].is_dir());

    // Directories are not files and files are not directories.
    assert_err!(s.read(&vp("wr"), None).await, StorageError::IsADirectory);
    assert_err!(
        s.list(&vp("wr/sub/hello.txt")).await,
        StorageError::NotADirectory
    );
    assert_err!(s.read(&vp("wr/nope"), None).await, StorageError::NotFound);
    // Writing over a directory or the root is refused.
    assert_err!(
        s.write(&vp("wr/sub"), reader(b"x".to_vec()), true).await,
        StorageError::IsADirectory
    );
    assert_err!(
        s.write(&VPath::root(), reader(b"x".to_vec()), true).await,
        StorageError::IsADirectory
    );
    // The parent must exist.
    assert_err!(
        s.write(&vp("wr/none/f"), reader(b"x".to_vec()), false)
            .await,
        StorageError::NotFound
    );

    // Empty files work.
    s.write(&vp("wr/empty"), reader(Vec::new()), false)
        .await
        .unwrap();
    assert_eq!(s.stat(&vp("wr/empty")).await.unwrap().size, 0);
    assert!(read_all(s, "wr/empty", None).await.unwrap().is_empty());

    s.delete(&vp("wr")).await.unwrap();
}

async fn ranges(s: &dyn Storage) {
    s.write(&vp("r.bin"), reader(b"0123456789".to_vec()), false)
        .await
        .unwrap();
    assert_eq!(read_all(s, "r.bin", Some(2..5)).await.unwrap(), b"234");
    assert_eq!(read_all(s, "r.bin", Some(9..10)).await.unwrap(), b"9");
    assert_eq!(
        read_all(s, "r.bin", Some(0..10)).await.unwrap(),
        b"0123456789"
    );
    assert!(read_all(s, "r.bin", Some(4..4)).await.unwrap().is_empty());
    assert_err!(
        read_all(s, "r.bin", Some(5..11)).await,
        StorageError::InvalidRange { size: 10 }
    );
    #[allow(clippy::reversed_empty_ranges)]
    let backwards = 6..5;
    assert_err!(
        read_all(s, "r.bin", Some(backwards)).await,
        StorageError::InvalidRange { size: 10 }
    );
    s.delete(&vp("r.bin")).await.unwrap();
}

async fn overwrite_rules(s: &dyn Storage) {
    s.write(&vp("o"), reader(b"one".to_vec()), false)
        .await
        .unwrap();
    assert_err!(
        s.write(&vp("o"), reader(b"two".to_vec()), false).await,
        StorageError::AlreadyExists
    );
    assert_eq!(read_all(s, "o", None).await.unwrap(), b"one");
    s.write(&vp("o"), reader(b"three".to_vec()), true)
        .await
        .unwrap();
    assert_eq!(read_all(s, "o", None).await.unwrap(), b"three");
    // No temporary files are left behind.
    assert_eq!(names(s, "").await, ["o"]);
    s.delete(&vp("o")).await.unwrap();
}

async fn mkdir_rules(s: &dyn Storage) {
    s.mkdir(&vp("m/a/b")).await.unwrap();
    // Creating an existing directory is fine.
    s.mkdir(&vp("m/a/b")).await.unwrap();
    s.mkdir(&vp("m/a")).await.unwrap();
    assert!(s.stat(&vp("m/a/b")).await.unwrap().is_dir());
    // Empty directories are listed.
    assert_eq!(names(s, "m/a").await, ["b"]);
    assert!(s.list(&vp("m/a/b")).await.unwrap().is_empty());
    // A file in the way is an error.
    s.write(&vp("m/f"), reader(b"f".to_vec()), false)
        .await
        .unwrap();
    assert_err!(s.mkdir(&vp("m/f/x")).await, StorageError::NotADirectory);
    assert_err!(s.mkdir(&vp("m/f")).await, StorageError::NotADirectory);
    s.delete(&vp("m")).await.unwrap();
}

async fn rename_rules(s: &dyn Storage) {
    s.mkdir(&vp("rn/d/sub")).await.unwrap();
    s.write(&vp("rn/d/sub/x"), reader(b"x".to_vec()), false)
        .await
        .unwrap();
    s.write(&vp("rn/f"), reader(b"f".to_vec()), false)
        .await
        .unwrap();
    s.write(&vp("rn/g"), reader(b"g".to_vec()), false)
        .await
        .unwrap();

    // Files.
    assert_err!(
        s.rename(&vp("rn/f"), &vp("rn/g"), false).await,
        StorageError::AlreadyExists
    );
    s.rename(&vp("rn/f"), &vp("rn/g"), true).await.unwrap();
    assert_eq!(read_all(s, "rn/g", None).await.unwrap(), b"f");
    assert_err!(s.stat(&vp("rn/f")).await, StorageError::NotFound);
    s.rename(&vp("rn/g"), &vp("rn/d/moved"), false)
        .await
        .unwrap();
    assert_eq!(read_all(s, "rn/d/moved", None).await.unwrap(), b"f");
    // Renaming onto itself is a no-op.
    s.rename(&vp("rn/d/moved"), &vp("rn/d/moved"), false)
        .await
        .unwrap();
    assert_err!(
        s.rename(&vp("rn/nope"), &vp("rn/x"), false).await,
        StorageError::NotFound
    );

    // Directories.
    s.rename(&vp("rn/d"), &vp("rn/e"), false).await.unwrap();
    assert_err!(s.stat(&vp("rn/d")).await, StorageError::NotFound);
    assert_eq!(read_all(s, "rn/e/sub/x", None).await.unwrap(), b"x");
    assert_eq!(names(s, "rn/e").await, ["moved", "sub"]);
    assert_err!(
        s.rename(&vp("rn/e"), &vp("rn/e/sub/in"), false).await,
        StorageError::InvalidPath(_)
    );
    s.mkdir(&vp("rn/other")).await.unwrap();
    assert_err!(
        s.rename(&vp("rn/e"), &vp("rn/other"), true).await,
        StorageError::AlreadyExists
    );
    assert_err!(
        s.rename(&VPath::root(), &vp("rn/x"), false).await,
        StorageError::InvalidPath(_)
    );
    s.delete(&vp("rn")).await.unwrap();
}

async fn copy_rules(s: &dyn Storage) {
    s.mkdir(&vp("cp/d/sub")).await.unwrap();
    s.mkdir(&vp("cp/d/empty")).await.unwrap();
    s.write(&vp("cp/d/sub/x"), reader(b"x".to_vec()), false)
        .await
        .unwrap();
    s.write(&vp("cp/d/top"), reader(b"top".to_vec()), false)
        .await
        .unwrap();

    s.copy(&vp("cp/d/top"), &vp("cp/top2"), false)
        .await
        .unwrap();
    assert_eq!(read_all(s, "cp/top2", None).await.unwrap(), b"top");
    assert_eq!(read_all(s, "cp/d/top", None).await.unwrap(), b"top");
    assert_err!(
        s.copy(&vp("cp/d/top"), &vp("cp/top2"), false).await,
        StorageError::AlreadyExists
    );
    s.write(&vp("cp/new"), reader(b"new".to_vec()), false)
        .await
        .unwrap();
    s.copy(&vp("cp/new"), &vp("cp/top2"), true).await.unwrap();
    assert_eq!(read_all(s, "cp/top2", None).await.unwrap(), b"new");

    s.copy(&vp("cp/d"), &vp("cp/d2"), false).await.unwrap();
    assert_eq!(read_all(s, "cp/d2/sub/x", None).await.unwrap(), b"x");
    assert_eq!(names(s, "cp/d2").await, ["empty", "sub", "top"]);
    // The source is untouched.
    assert_eq!(names(s, "cp/d").await, ["empty", "sub", "top"]);
    assert_err!(
        s.copy(&vp("cp/d"), &vp("cp/d2"), true).await,
        StorageError::AlreadyExists
    );
    assert_err!(
        s.copy(&vp("cp/d"), &vp("cp/d/sub/in"), false).await,
        StorageError::InvalidPath(_)
    );
    assert_err!(
        s.copy(&vp("cp/nope"), &vp("cp/x"), false).await,
        StorageError::NotFound
    );
    s.delete(&vp("cp")).await.unwrap();
}

async fn delete_rules(s: &dyn Storage) {
    s.mkdir(&vp("del/a/b/c")).await.unwrap();
    s.write(&vp("del/a/b/c/f"), reader(b"f".to_vec()), false)
        .await
        .unwrap();
    s.write(&vp("del/a/g"), reader(b"g".to_vec()), false)
        .await
        .unwrap();
    // A sibling whose name shares a prefix must survive.
    s.write(&vp("del/ab"), reader(b"ab".to_vec()), false)
        .await
        .unwrap();
    s.delete(&vp("del/a/g")).await.unwrap();
    assert_err!(s.stat(&vp("del/a/g")).await, StorageError::NotFound);
    s.delete(&vp("del/a")).await.unwrap();
    assert_err!(s.stat(&vp("del/a")).await, StorageError::NotFound);
    assert_err!(s.stat(&vp("del/a/b/c/f")).await, StorageError::NotFound);
    assert_eq!(read_all(s, "del/ab", None).await.unwrap(), b"ab");
    assert_err!(s.delete(&vp("del/a")).await, StorageError::NotFound);
    assert_err!(s.delete(&VPath::root()).await, StorageError::InvalidPath(_));
    s.delete(&vp("del")).await.unwrap();
    assert!(s.list(&VPath::root()).await.unwrap().is_empty());
}

async fn large_file(s: &dyn Storage) {
    // Bigger than one S3 multipart part and many SFTP/SMB packets.
    let data: Vec<u8> = (0..(12 * 1024 * 1024 + 123u32))
        .map(|i| (i % 251) as u8)
        .collect();
    let n = s
        .write(&vp("big.bin"), reader(data.clone()), false)
        .await
        .unwrap();
    assert_eq!(n, data.len() as u64);
    assert_eq!(
        s.stat(&vp("big.bin")).await.unwrap().size,
        data.len() as u64
    );
    let got = read_all(s, "big.bin", None).await.unwrap();
    assert!(got == data, "large file round trip differs");
    let r = 5_000_000..9_000_001u64;
    let got = read_all(s, "big.bin", Some(r.clone())).await.unwrap();
    assert!(
        got == data[r.start as usize..r.end as usize],
        "large range differs"
    );
    s.copy(&vp("big.bin"), &vp("big2.bin"), false)
        .await
        .unwrap();
    assert_eq!(
        s.stat(&vp("big2.bin")).await.unwrap().size,
        data.len() as u64
    );
    s.delete(&vp("big.bin")).await.unwrap();
    s.delete(&vp("big2.bin")).await.unwrap();
}

async fn odd_names(s: &dyn Storage) {
    // Characters that need escaping somewhere along the way.
    for name in [
        "space name.txt",
        "ümlaut-日本語.txt",
        "brackets [1] (2) {3}.txt",
        "percent %41 & plus+#.txt",
        "%2e%2e",
    ] {
        let p = VPath::root().join(name).unwrap();
        s.write(&p, reader(name.as_bytes().to_vec()), false)
            .await
            .unwrap_or_else(|e| panic!("write {name}: {e}"));
        let got = read_all(s, &p.to_string(), None).await.unwrap();
        assert_eq!(got, name.as_bytes());
        let listed = names(s, "").await;
        assert_eq!(listed, [name], "listing shows the exact name");
        s.delete(&p).await.unwrap();
    }
}
