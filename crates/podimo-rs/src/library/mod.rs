//! Audiobook library — persistent on-disk store with an in-memory index.
//!
//! Each book lives in `LIBRARY_DIR/<audiobook_uuid>/`:
//!   - `meta.json` — book metadata + current status, written atomically.
//!   - `audio.mp3` — downloaded audio (large; written via `.partial` → rename).
//!   - `cover.jpg` — downloaded cover image.
//!
//! Hydration: on startup the library scans `LIBRARY_DIR/*/meta.json` and rebuilds
//! the in-memory map. Any entries left in `Queued`/`Downloading` are forced to
//! `Failed("interrupted")` — we don't try to resume cross-process because the
//! signed audio URL is short-lived and would be invalid by the time we got here.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::fs;
use tokio::sync::RwLock;

pub mod download;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Queued,
    Downloading,
    Done,
    Failed,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LibraryEntry {
    pub id: String,
    pub title: String,
    pub author: String,
    pub narrators: String,
    pub description: String,
    pub duration_seconds: i64,
    pub publisher: Option<String>,
    pub year: Option<i64>,
    /// RFC-3339 timestamp.
    pub added_at: String,
    pub status: Status,
    pub error: Option<String>,
    pub audio_size_bytes: Option<u64>,
    pub audio_downloaded_bytes: u64,
}

#[derive(Clone)]
pub struct Library {
    root: PathBuf,
    entries: Arc<RwLock<BTreeMap<String, LibraryEntry>>>,
}

impl std::fmt::Debug for Library {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Library").field("root", &self.root).finish()
    }
}

impl Library {
    /// Construct a Library rooted at `root`, hydrating any pre-existing
    /// `<root>/<id>/meta.json` entries. Creates `<root>` if missing.
    pub async fn new(root: impl Into<PathBuf>) -> anyhow::Result<Self> {
        let root = root.into();
        fs::create_dir_all(&root).await?;
        let entries = hydrate(&root).await?;
        Ok(Self {
            root,
            entries: Arc::new(RwLock::new(entries)),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn entry_dir(&self, id: &str) -> PathBuf {
        self.root.join(id)
    }

    pub fn audio_path(&self, id: &str) -> PathBuf {
        self.entry_dir(id).join("audio.mp3")
    }

    pub fn cover_path(&self, id: &str) -> PathBuf {
        self.entry_dir(id).join("cover.jpg")
    }

    pub fn audio_partial_path(&self, id: &str) -> PathBuf {
        self.entry_dir(id).join("audio.mp3.partial")
    }

    /// Snapshot of all entries, newest-added first.
    pub async fn list(&self) -> Vec<LibraryEntry> {
        let guard = self.entries.read().await;
        let mut v: Vec<LibraryEntry> = guard.values().cloned().collect();
        // BTreeMap iterates by id (UUID order). Sort by added_at descending so the
        // most recently added book appears at the top of the overview.
        v.sort_by(|a, b| b.added_at.cmp(&a.added_at));
        v
    }

    pub async fn get(&self, id: &str) -> Option<LibraryEntry> {
        self.entries.read().await.get(id).cloned()
    }

    pub async fn contains(&self, id: &str) -> bool {
        self.entries.read().await.contains_key(id)
    }

    /// Insert a fresh entry. Fails if `id` is already present — callers should
    /// `remove` first or check `contains`.
    pub async fn add(&self, entry: LibraryEntry) -> anyhow::Result<()> {
        let dir = self.entry_dir(&entry.id);
        fs::create_dir_all(&dir).await?;
        let mut guard = self.entries.write().await;
        if guard.contains_key(&entry.id) {
            anyhow::bail!("library already contains {}", entry.id);
        }
        write_meta(&dir, &entry).await?;
        guard.insert(entry.id.clone(), entry);
        Ok(())
    }

    /// Drop the entry from memory and remove its on-disk directory.
    pub async fn remove(&self, id: &str) -> anyhow::Result<bool> {
        let mut guard = self.entries.write().await;
        if guard.remove(id).is_none() {
            return Ok(false);
        }
        let dir = self.entry_dir(id);
        if let Err(err) = fs::remove_dir_all(&dir).await {
            if err.kind() != io::ErrorKind::NotFound {
                return Err(err.into());
            }
        }
        Ok(true)
    }

    /// Atomically read-modify-write an entry. The callback gets a `&mut` to the
    /// in-memory copy; on return the changes are persisted to `meta.json` before
    /// the lock is dropped. Returns `Ok(None)` if `id` doesn't exist.
    pub async fn update<F>(&self, id: &str, f: F) -> anyhow::Result<Option<LibraryEntry>>
    where
        F: FnOnce(&mut LibraryEntry),
    {
        let mut guard = self.entries.write().await;
        let entry = match guard.get_mut(id) {
            Some(e) => e,
            None => return Ok(None),
        };
        f(entry);
        let snapshot = entry.clone();
        let dir = self.entry_dir(id);
        write_meta(&dir, &snapshot).await?;
        Ok(Some(snapshot))
    }
}

/// Scan `<root>/<id>/meta.json` files into a fresh map. Force in-flight states
/// (`Queued` / `Downloading`) to `Failed("interrupted")` since the cross-process
/// audio URL would be expired by now.
async fn hydrate(root: &Path) -> anyhow::Result<BTreeMap<String, LibraryEntry>> {
    let mut map = BTreeMap::new();
    let mut read = match fs::read_dir(root).await {
        Ok(r) => r,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(map),
        Err(err) => return Err(err.into()),
    };
    while let Some(entry) = read.next_entry().await? {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let meta_path = path.join("meta.json");
        let raw = match fs::read(&meta_path).await {
            Ok(b) => b,
            Err(err) if err.kind() == io::ErrorKind::NotFound => continue,
            Err(err) => {
                tracing::warn!(target: "podimo::library", "skip {}: {err}", meta_path.display());
                continue;
            }
        };
        let mut book: LibraryEntry = match serde_json::from_slice(&raw) {
            Ok(b) => b,
            Err(err) => {
                tracing::warn!(target: "podimo::library", "corrupt {}: {err}", meta_path.display());
                continue;
            }
        };
        if matches!(book.status, Status::Queued | Status::Downloading) {
            book.status = Status::Failed;
            book.error = Some("interrupted by restart".into());
            // Rewrite the meta with the failed state so subsequent reads see it.
            if let Err(err) = write_meta(&path, &book).await {
                tracing::warn!(target: "podimo::library", "rewrite {}: {err}", meta_path.display());
            }
        }
        map.insert(book.id.clone(), book);
    }
    Ok(map)
}

/// Write `meta.json` atomically (`tmp` → rename). Cheap because the meta is
/// small; the heavy audio file uses the same pattern in `download.rs`.
async fn write_meta(dir: &Path, entry: &LibraryEntry) -> anyhow::Result<()> {
    let json = serde_json::to_vec_pretty(entry)?;
    let final_path = dir.join("meta.json");
    let tmp_path = dir.join("meta.json.tmp");
    fs::write(&tmp_path, json).await?;
    fs::rename(&tmp_path, &final_path).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_entry(id: &str) -> LibraryEntry {
        LibraryEntry {
            id: id.into(),
            title: "Test".into(),
            author: "Author".into(),
            narrators: "Narrator".into(),
            description: "Desc".into(),
            duration_seconds: 3600,
            publisher: Some("Pub".into()),
            year: Some(2024),
            added_at: "2026-05-14T10:00:00Z".into(),
            status: Status::Queued,
            error: None,
            audio_size_bytes: None,
            audio_downloaded_bytes: 0,
        }
    }

    #[tokio::test]
    async fn add_then_list_returns_inserted_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let lib = Library::new(tmp.path()).await.unwrap();
        lib.add(sample_entry("a1")).await.unwrap();
        let v = lib.list().await;
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].id, "a1");
    }

    #[tokio::test]
    async fn add_twice_same_id_fails() {
        let tmp = tempfile::tempdir().unwrap();
        let lib = Library::new(tmp.path()).await.unwrap();
        lib.add(sample_entry("a1")).await.unwrap();
        let err = lib.add(sample_entry("a1")).await.unwrap_err();
        assert!(err.to_string().contains("already contains"));
    }

    #[tokio::test]
    async fn remove_drops_entry_and_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let lib = Library::new(tmp.path()).await.unwrap();
        lib.add(sample_entry("a1")).await.unwrap();
        let dir = lib.entry_dir("a1");
        assert!(dir.exists());
        assert!(lib.remove("a1").await.unwrap());
        assert!(!dir.exists());
        assert!(lib.get("a1").await.is_none());
    }

    #[tokio::test]
    async fn remove_unknown_returns_false() {
        let tmp = tempfile::tempdir().unwrap();
        let lib = Library::new(tmp.path()).await.unwrap();
        assert!(!lib.remove("nope").await.unwrap());
    }

    #[tokio::test]
    async fn update_persists_to_disk() {
        let tmp = tempfile::tempdir().unwrap();
        let lib = Library::new(tmp.path()).await.unwrap();
        lib.add(sample_entry("a1")).await.unwrap();
        lib.update("a1", |e| {
            e.status = Status::Done;
            e.audio_size_bytes = Some(42);
        })
        .await
        .unwrap();

        // Rehydrate from disk to confirm persistence.
        let lib2 = Library::new(tmp.path()).await.unwrap();
        let e = lib2.get("a1").await.unwrap();
        assert_eq!(e.status, Status::Done);
        assert_eq!(e.audio_size_bytes, Some(42));
    }

    #[tokio::test]
    async fn hydrate_marks_interrupted_downloads_as_failed() {
        let tmp = tempfile::tempdir().unwrap();
        let lib = Library::new(tmp.path()).await.unwrap();
        let mut e = sample_entry("a1");
        e.status = Status::Downloading;
        lib.add(e).await.unwrap();

        let lib2 = Library::new(tmp.path()).await.unwrap();
        let e = lib2.get("a1").await.unwrap();
        assert_eq!(e.status, Status::Failed);
        assert_eq!(e.error.as_deref(), Some("interrupted by restart"));
    }
}
