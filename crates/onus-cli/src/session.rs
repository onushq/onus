//! A live map of one worktree, for queries while agents edit it.
//!
//! The session keeps the worktree's map in memory as a [`MapIndex`]. A file
//! watcher marks the map stale when a file changes; the next query rebuilds
//! it once, however many queries are waiting, and the rebuild reuses the
//! per-file facts of every file that did not change (a [`FactsCache`]
//! shared by all worktrees of the repository). Queries between changes are
//! answered from memory.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use onus_index::MapIndex;
use onus_map::{BuildOptions, FactsCache};

/// A worktree's map, kept current.
pub struct Session {
    root: PathBuf,
    cache: Arc<FactsCache>,
    /// Bumped by the watcher on every relevant file change.
    changes: Arc<AtomicU64>,
    /// When the watcher last saw a change.
    last_change: Arc<Mutex<Instant>>,
    state: Mutex<State>,
    /// Kept alive for the session's lifetime; `None` when watching failed,
    /// in which case every query checks the files instead.
    _watcher: Option<notify::RecommendedWatcher>,
}

struct State {
    index: Option<Arc<MapIndex>>,
    /// `changes` when the current index was built.
    built_at_change: u64,
    version: u64,
    build_ms: u64,
    /// Without a watcher: the file list and sizes the index was built from.
    fingerprint: Option<u64>,
}

/// About the map an answer came from.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MapMeta {
    /// Increases each time the worktree's map is rebuilt.
    pub version: u64,
    /// Whether this query rebuilt the map.
    pub rebuilt: bool,
    /// How long the last rebuild took.
    pub build_ms: u64,
    pub watching: bool,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("root", &self.root)
            .field("watching", &self._watcher.is_some())
            .finish_non_exhaustive()
    }
}

/// Changes closer together than this are waited out, so a query right
/// after a burst of writes sees all of them.
const SETTLE: Duration = Duration::from_millis(30);

impl Session {
    pub fn new(root: &Path, cache: Arc<FactsCache>) -> Result<Session> {
        let root = crate::daemon::canonical(root)
            .with_context(|| format!("cannot open {}", root.display()))?;
        let changes = Arc::new(AtomicU64::new(0));
        let last_change = Arc::new(Mutex::new(Instant::now()));
        let watcher = watch(&root, changes.clone(), last_change.clone()).ok();
        Ok(Session {
            root,
            cache,
            changes,
            last_change,
            state: Mutex::new(State {
                index: None,
                built_at_change: 0,
                version: 0,
                build_ms: 0,
                fingerprint: None,
            }),
            _watcher: watcher,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Marks the map stale, so the next question rebuilds it even if the
    /// watcher has not reported the change yet.
    pub fn invalidate(&self) {
        if let Ok(mut t) = self.last_change.lock() {
            *t = Instant::now();
        }
        self.changes.fetch_add(1, Ordering::SeqCst);
        if let Ok(mut state) = self.state.lock() {
            state.fingerprint = None;
        }
    }

    /// The current index, rebuilt first if a file changed since it was
    /// built. Concurrent callers wait for one rebuild and share it.
    pub fn index(&self) -> Result<(Arc<MapIndex>, MapMeta)> {
        // Let a burst of writes finish before deciding.
        if let Ok(t) = self.last_change.lock() {
            let since = t.elapsed();
            if since < SETTLE {
                std::thread::sleep(SETTLE - since);
            }
        }
        let mut state = self
            .state
            .lock()
            .map_err(|_| anyhow::anyhow!("session state poisoned"))?;
        let now = self.changes.load(Ordering::SeqCst);
        let watching = self._watcher.is_some();
        let stale = match &state.index {
            None => true,
            Some(_) if watching => now != state.built_at_change,
            Some(_) => Some(fingerprint(&self.root)) != state.fingerprint,
        };
        if !stale && let Some(index) = &state.index {
            return Ok((
                index.clone(),
                MapMeta {
                    version: state.version,
                    rebuilt: false,
                    build_ms: state.build_ms,
                    watching,
                },
            ));
        }
        let started = Instant::now();
        let fp = (!watching).then(|| fingerprint(&self.root));
        let map = onus_map::build_map(
            &self.root,
            &BuildOptions {
                commit: Some("worktree".into()),
                facts_cache: Some(self.cache.clone()),
                ..BuildOptions::default()
            },
        )?;
        let index = Arc::new(MapIndex::new(map).with_root(&self.root));
        state.index = Some(index.clone());
        state.built_at_change = now;
        state.version += 1;
        state.build_ms = started.elapsed().as_millis() as u64;
        state.fingerprint = fp;
        Ok((
            index,
            MapMeta {
                version: state.version,
                rebuilt: true,
                build_ms: state.build_ms,
                watching,
            },
        ))
    }
}

/// Whether a changed path can affect the map: not inside `.git`,
/// `node_modules` or another folder the map never reads.
fn relevant(root: &Path, path: &Path) -> bool {
    let Ok(rel) = path.strip_prefix(root) else {
        return false;
    };
    !rel.components().any(|c| {
        let name = c.as_os_str().to_string_lossy();
        onus_map::walk::SKIPPED_DIRS.contains(&name.as_ref())
    })
}

fn watch(
    root: &Path,
    changes: Arc<AtomicU64>,
    last_change: Arc<Mutex<Instant>>,
) -> notify::Result<notify::RecommendedWatcher> {
    use notify::{EventKind, RecursiveMode, Watcher};
    let base = root.to_path_buf();
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        let Ok(event) = event else {
            // Overflow or an error: assume anything changed.
            changes.fetch_add(1, Ordering::SeqCst);
            return;
        };
        if matches!(event.kind, EventKind::Access(_)) {
            return;
        }
        if event.paths.iter().any(|p| relevant(&base, p)) {
            if let Ok(mut t) = last_change.lock() {
                *t = Instant::now();
            }
            changes.fetch_add(1, Ordering::SeqCst);
        }
    })?;
    watcher.watch(root, RecursiveMode::Recursive)?;
    Ok(watcher)
}

/// A cheap summary of the tree, for sessions without a watcher: the paths,
/// sizes and modification times of every file the map reads.
fn fingerprint(root: &Path) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for f in onus_map::walk::list_files(root) {
        f.hash(&mut h);
        if let Ok(m) = std::fs::metadata(onus_core::paths::native(root, &f)) {
            m.len().hash(&mut h);
            if let Ok(t) = m.modified() {
                t.hash(&mut h);
            }
        }
    }
    h.finish()
}
