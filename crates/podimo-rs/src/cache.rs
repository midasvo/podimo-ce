//! Caches with TTL semantics.
//!
//! Three caches:
//!   - `tokens`   — login token by `sha256(username~password)`. Optionally persisted to disk.
//!   - `podcasts` — full episode list per podcast id, JSON value. Persisted.
//!   - `head`     — `(content_length, content_type)` per episode id. Persisted.
//!
//! Disk format: one bincode file per entry under `<cache_dir>/<name>/<key>.bin`.
//! Each file contains `(expiry_unix_seconds, value_bytes)`. Wipe `<cache_dir>` to reset.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use moka::future::Cache;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tokio::fs;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeadInfo {
    pub content_length: String,
    pub content_type: String,
}

#[derive(Debug, Clone)]
pub struct Caches {
    pub(crate) tokens: TtlCache<String>,
    pub(crate) podcasts: TtlCache<Arc<serde_json::Value>>,
    pub head: TtlCache<HeadInfo>,
}

impl Caches {
    pub(crate) async fn init(
        cache_dir: &str,
        store_tokens_on_disk: bool,
        token_ttl: u64,
        podcast_ttl: u64,
        head_ttl: u64,
    ) -> Self {
        let root = PathBuf::from(cache_dir);
        let tokens = TtlCache::new(
            "tokens",
            if store_tokens_on_disk {
                Some(root.join("tokens_cache"))
            } else {
                None
            },
            Duration::from_secs(token_ttl),
        )
        .await;
        let podcasts = TtlCache::new(
            "podcasts",
            Some(root.join("podcast_cache")),
            Duration::from_secs(podcast_ttl),
        )
        .await;
        let head = TtlCache::new(
            "head",
            Some(root.join("head_cache")),
            Duration::from_secs(head_ttl),
        )
        .await;

        Self {
            tokens,
            podcasts,
            head,
        }
    }
}

/// In-memory TTL cache with optional persistence to disk. Disk writes happen
/// in the background; reads first check moka, then fall back to disk.
#[derive(Clone)]
pub struct TtlCache<V>
where
    V: Clone + Send + Sync + Serialize + DeserializeOwned + 'static,
{
    name: &'static str,
    inner: Cache<String, Entry<V>>,
    dir: Option<PathBuf>,
    default_ttl: Duration,
}

impl<V> std::fmt::Debug for TtlCache<V>
where
    V: Clone + Send + Sync + Serialize + DeserializeOwned + 'static,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TtlCache")
            .field("name", &self.name)
            .finish()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Entry<V> {
    expiry: u64,
    value: V,
}

impl<V> TtlCache<V>
where
    V: Clone + Send + Sync + Serialize + DeserializeOwned + 'static,
{
    pub async fn new(name: &'static str, dir: Option<PathBuf>, default_ttl: Duration) -> Self {
        let cache = Cache::builder()
            .max_capacity(10_000)
            .time_to_live(default_ttl.saturating_mul(2))
            .build();
        let this = Self {
            name,
            inner: cache,
            dir: dir.clone(),
            default_ttl,
        };

        if let Some(dir) = &dir {
            if let Err(err) = fs::create_dir_all(dir).await {
                tracing::warn!(target: "podimo::cache", "create_dir_all({}): {err}", dir.display());
            }
        }
        this
    }

    pub async fn get(&self, key: &str) -> Option<V> {
        if let Some(entry) = self.inner.get(key).await {
            if entry.expiry > now_secs() {
                return Some(entry.value);
            } else {
                self.inner.invalidate(key).await;
            }
        }
        if let Some(dir) = &self.dir {
            if let Some(entry) = read_entry::<V>(&entry_path(dir, key)).await {
                if entry.expiry > now_secs() {
                    self.inner.insert(key.to_string(), entry.clone()).await;
                    return Some(entry.value);
                }
            }
        }
        None
    }

    /// Like [`Self::get`] but never deletes an expired on-disk entry: expired
    /// entries return `None`, but the underlying record (in moka and on disk)
    /// is kept. Used for the HEAD cache so the historical record survives TTL.
    pub async fn get_no_expire(&self, key: &str) -> Option<V> {
        if let Some(entry) = self.inner.get(key).await {
            return if entry.expiry > now_secs() {
                Some(entry.value)
            } else {
                None
            };
        }
        if let Some(dir) = &self.dir {
            if let Some(entry) = read_entry::<V>(&entry_path(dir, key)).await {
                self.inner.insert(key.to_string(), entry.clone()).await;
                return if entry.expiry > now_secs() {
                    Some(entry.value)
                } else {
                    None
                };
            }
        }
        None
    }

    pub async fn insert(&self, key: String, value: V) {
        self.insert_with_ttl(key, value, self.default_ttl).await
    }

    pub async fn insert_with_ttl(&self, key: String, value: V, ttl: Duration) {
        let entry = Entry {
            expiry: now_secs() + ttl.as_secs(),
            value,
        };
        if let Some(dir) = &self.dir {
            if let Err(err) = write_entry(&entry_path(dir, &key), &entry).await {
                tracing::warn!(target: "podimo::cache", "persist {} key {}: {err}", self.name, key);
            }
        }
        self.inner.insert(key, entry).await;
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn entry_path(dir: &Path, key: &str) -> PathBuf {
    // Hash isn't required for safety here (cache keys are already opaque hashes
    // or podcast ids), but we still sanitize: replace any path separator.
    let safe: String = key
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    dir.join(format!("{safe}.bin"))
}

async fn read_entry<V>(path: &Path) -> Option<Entry<V>>
where
    V: DeserializeOwned,
{
    let bytes = fs::read(path).await.ok()?;
    bincode::deserialize::<Entry<V>>(&bytes).ok()
}

async fn write_entry<V>(path: &Path, entry: &Entry<V>) -> std::io::Result<()>
where
    V: Serialize,
{
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).await?;
    }
    let bytes = bincode::serialize(entry).map_err(std::io::Error::other)?;
    fs::write(path, bytes).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn insert_then_retrieve_before_expiry() {
        let cache: TtlCache<String> = TtlCache::new("test", None, Duration::from_secs(60)).await;
        cache.insert("k".into(), "v".into()).await;
        assert_eq!(cache.get("k").await, Some("v".into()));
        // Hit doesn't evict the entry.
        assert_eq!(cache.get("k").await, Some("v".into()));
    }

    #[tokio::test]
    async fn missing_key_returns_none() {
        let cache: TtlCache<String> = TtlCache::new("test", None, Duration::from_secs(60)).await;
        assert_eq!(cache.get("missing").await, None);
    }

    #[tokio::test]
    async fn expired_entry_returns_none_and_is_evicted_from_memory() {
        let cache: TtlCache<String> = TtlCache::new("test", None, Duration::from_millis(50)).await;
        cache
            .insert_with_ttl("k".into(), "v".into(), Duration::from_millis(1))
            .await;
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(cache.get("k").await, None);
        // After expiry, a subsequent direct check sees an evicted moka entry.
        assert!(
            cache.inner.get("k").await.is_none(),
            "key should be invalidated"
        );
    }

    #[tokio::test]
    async fn get_no_expire_returns_none_when_expired_but_keeps_key() {
        // get_no_expire returns None on expiry but must not remove the
        // underlying record (in-memory or on-disk).
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        let cache: TtlCache<String> =
            TtlCache::new("test", Some(dir.clone()), Duration::from_secs(60)).await;
        cache
            .insert_with_ttl("k".into(), "v".into(), Duration::from_millis(1))
            .await;
        tokio::time::sleep(Duration::from_millis(20)).await;

        assert_eq!(cache.get_no_expire("k").await, None);
        // Disk record is still present (not deleted on expiry-via-get_no_expire).
        let on_disk = entry_path(&dir, "k");
        assert!(on_disk.exists(), "on-disk record must survive expiry");
    }

    #[tokio::test]
    async fn get_no_expire_returns_value_when_not_expired() {
        let cache: TtlCache<String> = TtlCache::new("test", None, Duration::from_secs(60)).await;
        cache.insert("k".into(), "v".into()).await;
        assert_eq!(cache.get_no_expire("k").await, Some("v".into()));
    }

    #[tokio::test]
    async fn disk_persistence_survives_restart() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();

        {
            let c: TtlCache<String> =
                TtlCache::new("test", Some(dir.clone()), Duration::from_secs(60)).await;
            c.insert("key1".into(), "val1".into()).await;
        }
        let c2: TtlCache<String> =
            TtlCache::new("test", Some(dir.clone()), Duration::from_secs(60)).await;
        assert_eq!(c2.get("key1").await, Some("val1".into()));
    }
}
