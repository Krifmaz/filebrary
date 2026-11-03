//! Dead properties: arbitrary properties clients set with PROPPATCH.
//!
//! Windows Explorer stores `Win32FileAttributes` and timestamps this way,
//! macOS Finder stores a few of its own, and litmus checks that any property
//! round-trips. Values are kept as serialized XML elements (see
//! [`crate::xml::serialize_element`]) keyed by the *real* path, so every user
//! sees the same properties whatever their scope.
//!
//! [`MemoryPropStore`] keeps them in memory with hard size limits; a server
//! that wants them to survive restarts can implement [`PropStore`] on its
//! database.

use std::collections::BTreeMap;

use async_trait::async_trait;
use filebrary_storage::VPath;
use tokio::sync::Mutex;

use crate::xml::{PatchOp, PropName};

/// Most dead properties kept on one resource.
pub const MAX_PROPS_PER_RESOURCE: usize = 64;
/// Longest serialized value of one dead property, in bytes.
pub const MAX_PROP_VALUE: usize = 16 * 1024;

/// A PROPPATCH could not be stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PropError {
    /// A limit was hit (sent as 507 Insufficient Storage).
    #[error("too many or too large properties")]
    TooLarge,
}

/// Storage for dead properties.
#[async_trait]
pub trait PropStore: Send + Sync + 'static {
    /// All dead properties of one resource.
    async fn get(&self, source: &str, path: &VPath) -> Vec<(PropName, String)>;

    /// Applies set/remove operations atomically: either all are stored or
    /// none are.
    async fn patch(&self, source: &str, path: &VPath, ops: &[PatchOp]) -> Result<(), PropError>;

    /// Copies the properties of `from` and everything below it to `to`.
    async fn copy_tree(&self, source: &str, from: &VPath, to_source: &str, to: &VPath);

    /// Moves the properties of `from` and everything below it to `to`.
    async fn move_tree(&self, source: &str, from: &VPath, to_source: &str, to: &VPath);

    /// Forgets the properties of `path` and everything below it.
    async fn delete_tree(&self, source: &str, path: &VPath);
}

type Key = (String, VPath);

/// An in-memory [`PropStore`]. Properties are lost on restart.
#[derive(Debug)]
pub struct MemoryPropStore {
    inner: Mutex<Inner>,
    max_total: usize,
}

#[derive(Debug, Default)]
struct Inner {
    map: BTreeMap<Key, BTreeMap<PropName, String>>,
    bytes: usize,
}

fn cost(name: &PropName, value: &str) -> usize {
    name.ns.len() + name.name.len() + value.len() + 32
}

impl MemoryPropStore {
    /// A store holding at most `max_total` bytes in all (8 MiB is plenty for
    /// the Explorer and Finder properties of tens of thousands of files).
    pub fn new(max_total: usize) -> Self {
        Self {
            inner: Mutex::new(Inner::default()),
            max_total,
        }
    }

    fn subtree(inner: &Inner, source: &str, from: &VPath) -> Vec<Key> {
        inner
            .map
            .range((source.to_owned(), from.clone())..)
            // Keys order component by component, so a subtree is contiguous.
            .take_while(|((s, p), _)| s == source && p.starts_with(from))
            .map(|(k, _)| k.clone())
            .collect()
    }
}

impl Default for MemoryPropStore {
    fn default() -> Self {
        Self::new(8 * 1024 * 1024)
    }
}

#[async_trait]
impl PropStore for MemoryPropStore {
    async fn get(&self, source: &str, path: &VPath) -> Vec<(PropName, String)> {
        let inner = self.inner.lock().await;
        inner
            .map
            .get(&(source.to_owned(), path.clone()))
            .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default()
    }

    async fn patch(&self, source: &str, path: &VPath, ops: &[PatchOp]) -> Result<(), PropError> {
        let mut inner = self.inner.lock().await;
        let key = (source.to_owned(), path.clone());
        let mut props = inner.map.get(&key).cloned().unwrap_or_default();
        let before: usize = props.iter().map(|(k, v)| cost(k, v)).sum();
        for op in ops {
            match op {
                PatchOp::Set(name, value) => {
                    if value.len() > MAX_PROP_VALUE {
                        return Err(PropError::TooLarge);
                    }
                    props.insert(name.clone(), value.clone());
                }
                PatchOp::Remove(name) => {
                    props.remove(name);
                }
            }
        }
        if props.len() > MAX_PROPS_PER_RESOURCE {
            return Err(PropError::TooLarge);
        }
        let after: usize = props.iter().map(|(k, v)| cost(k, v)).sum();
        let total = inner.bytes - before + after;
        if total > self.max_total {
            return Err(PropError::TooLarge);
        }
        inner.bytes = total;
        if props.is_empty() {
            inner.map.remove(&key);
        } else {
            inner.map.insert(key, props);
        }
        Ok(())
    }

    async fn copy_tree(&self, source: &str, from: &VPath, to_source: &str, to: &VPath) {
        let mut inner = self.inner.lock().await;
        for key in Self::subtree(&inner, source, from) {
            let Some(dest) = key.1.strip_prefix(from).map(|rel| to.join_path(&rel)) else {
                continue;
            };
            let props = inner.map[&key].clone();
            let added: usize = props.iter().map(|(k, v)| cost(k, v)).sum();
            if inner.bytes + added > self.max_total {
                tracing::warn!("dead property store full; not copying properties");
                return;
            }
            inner.bytes += added;
            if let Some(old) = inner.map.insert((to_source.to_owned(), dest), props) {
                inner.bytes -= old.iter().map(|(k, v)| cost(k, v)).sum::<usize>();
            }
        }
    }

    async fn move_tree(&self, source: &str, from: &VPath, to_source: &str, to: &VPath) {
        let mut inner = self.inner.lock().await;
        for key in Self::subtree(&inner, source, from) {
            let Some(props) = inner.map.remove(&key) else {
                continue;
            };
            let Some(dest) = key.1.strip_prefix(from).map(|rel| to.join_path(&rel)) else {
                continue;
            };
            if let Some(old) = inner.map.insert((to_source.to_owned(), dest), props) {
                inner.bytes -= old.iter().map(|(k, v)| cost(k, v)).sum::<usize>();
            }
        }
    }

    async fn delete_tree(&self, source: &str, path: &VPath) {
        let mut inner = self.inner.lock().await;
        for key in Self::subtree(&inner, source, path) {
            if let Some(old) = inner.map.remove(&key) {
                inner.bytes -= old.iter().map(|(k, v)| cost(k, v)).sum::<usize>();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> VPath {
        VPath::parse(s).unwrap()
    }

    fn set(n: &str, v: &str) -> PatchOp {
        PatchOp::Set(PropName::new("urn:t", n), v.into())
    }

    #[tokio::test]
    async fn patch_copy_move_delete() {
        let s = MemoryPropStore::default();
        s.patch("a", &p("d/f"), &[set("x", "<x/>")]).await.unwrap();
        s.patch("a", &p("d"), &[set("y", "<y/>")]).await.unwrap();
        s.patch("a", &p("dz"), &[set("z", "<z/>")]).await.unwrap();
        s.copy_tree("a", &p("d"), "b", &p("e")).await;
        assert_eq!(s.get("b", &p("e/f")).await.len(), 1);
        assert_eq!(s.get("b", &p("e")).await.len(), 1);
        assert!(s.get("b", &p("ez")).await.is_empty());
        s.move_tree("a", &p("d"), "a", &p("m")).await;
        assert!(s.get("a", &p("d/f")).await.is_empty());
        assert_eq!(s.get("a", &p("m/f")).await.len(), 1);
        // A sibling that merely shares a prefix string is untouched.
        assert_eq!(s.get("a", &p("dz")).await.len(), 1);
        s.delete_tree("a", &p("m")).await;
        assert!(s.get("a", &p("m")).await.is_empty());
        s.patch(
            "a",
            &p("dz"),
            &[PatchOp::Remove(PropName::new("urn:t", "z"))],
        )
        .await
        .unwrap();
        s.delete_tree("b", &VPath::root()).await;
        assert_eq!(s.inner.lock().await.bytes, 0);
    }

    #[tokio::test]
    async fn limits_are_atomic() {
        let s = MemoryPropStore::new(200);
        let big = "x".repeat(300);
        let r = s
            .patch("a", &p("f"), &[set("ok", "<ok/>"), set("big", &big)])
            .await;
        assert_eq!(r, Err(PropError::TooLarge));
        assert!(s.get("a", &p("f")).await.is_empty());
    }
}
