//! Per-file facts, kept between map builds.
//!
//! Extracting a file's facts depends only on its path, its bytes, whether
//! it is a test, the grammar and the compiled patterns. The cache key is a
//! hash of exactly those, so a hit is always the same facts a fresh
//! extraction would give. Worktrees of one repository share almost all of
//! their files, so a shared cache lets a second worktree, or a second
//! process, skip parsing what another already parsed.
//!
//! Entries live in memory and, optionally, in a folder. On disk each entry
//! is one file named by its key, written to a temporary name and renamed,
//! so concurrent processes never see half an entry and never conflict:
//! two writers of one key write the same bytes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::extract::FileFacts;

/// Bumped when the facts format or extraction changes in a way the crate
/// version does not capture.
const FORMAT: &str = "2";

/// Entries kept in memory before the oldest generation is dropped.
const MEMORY_ENTRIES: usize = 400_000;

#[derive(Debug, Default)]
pub struct FactsCache {
    memory: Mutex<HashMap<String, Arc<FileFacts>>>,
    dir: Option<PathBuf>,
    hits: AtomicU64,
    disk_hits: AtomicU64,
    misses: AtomicU64,
}

/// How a cache has been used, for status reports and benchmarks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheStats {
    pub entries: usize,
    pub memory_hits: u64,
    pub disk_hits: u64,
    pub misses: u64,
}

impl FactsCache {
    /// A cache that lives as long as the process.
    pub fn in_memory() -> FactsCache {
        FactsCache::default()
    }

    /// A cache that also keeps entries in `dir`, shared with every process
    /// that uses the same folder.
    pub fn on_disk(dir: &Path) -> FactsCache {
        FactsCache {
            dir: Some(dir.to_path_buf()),
            ..FactsCache::default()
        }
    }

    /// The key for one file's facts.
    pub fn key(path: &str, is_test: bool, tsx: bool, patterns: &str, content: &[u8]) -> String {
        let mut h = Vec::with_capacity(content.len() + path.len() + 160);
        h.extend_from_slice(
            format!(
                "onus-lang-ts {} facts {FORMAT}\0{path}\0{is_test}\0{tsx}\0{patterns}\0",
                env!("CARGO_PKG_VERSION")
            )
            .as_bytes(),
        );
        h.extend_from_slice(content);
        onus_core::hash::sha256_hex(&h)
    }

    pub fn get(&self, key: &str) -> Option<Arc<FileFacts>> {
        if let Some(f) = self.memory.lock().ok()?.get(key).cloned() {
            self.hits.fetch_add(1, Ordering::Relaxed);
            return Some(f);
        }
        let facts: FileFacts = self
            .dir
            .as_ref()
            .and_then(|d| std::fs::read(entry_path(d, key)).ok())
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())?;
        let facts = Arc::new(facts);
        self.disk_hits.fetch_add(1, Ordering::Relaxed);
        self.remember(key, &facts);
        Some(facts)
    }

    pub fn put(&self, key: &str, facts: &Arc<FileFacts>) {
        self.misses.fetch_add(1, Ordering::Relaxed);
        self.remember(key, facts);
        if let Some(dir) = &self.dir {
            // A cache that cannot be written only costs time later.
            let _ = write_entry(dir, key, facts);
        }
    }

    fn remember(&self, key: &str, facts: &Arc<FileFacts>) {
        if let Ok(mut m) = self.memory.lock() {
            if m.len() >= MEMORY_ENTRIES {
                m.clear();
            }
            m.insert(key.to_string(), facts.clone());
        }
    }

    pub fn stats(&self) -> CacheStats {
        CacheStats {
            entries: self.memory.lock().map(|m| m.len()).unwrap_or(0),
            memory_hits: self.hits.load(Ordering::Relaxed),
            disk_hits: self.disk_hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
        }
    }
}

fn entry_path(dir: &Path, key: &str) -> PathBuf {
    dir.join(&key[..2]).join(format!("{key}.json"))
}

fn write_entry(dir: &Path, key: &str, facts: &FileFacts) -> std::io::Result<()> {
    let path = entry_path(dir, key);
    if path.exists() {
        return Ok(());
    }
    let parent = path.parent().unwrap_or(dir);
    std::fs::create_dir_all(parent)?;
    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    {
        let mut w = std::io::BufWriter::new(tmp.as_file_mut());
        serde_json::to_writer(&mut w, facts)?;
        std::io::Write::flush(&mut w)?;
    }
    tmp.persist(&path).map_err(|e| e.error)?;
    Ok(())
}
