//! The pieces behind the `onus` command, shared by the binary and its tests.

pub mod daemon;
pub mod guide;
pub mod mcp;
pub mod session;

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use onus_core::{CodebaseMap, SemanticReport};
use onus_diff::{DiffInput, Intent};
use onus_map::{BuildOptions, LoadedConfig};

/// Exit code when a `--fail-on` condition is met.
pub const EXIT_FAIL_ON: i32 = 2;
/// Exit code for usage and runtime errors.
pub const EXIT_ERROR: i32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Format {
    Md,
    Json,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum FailOn {
    /// New boundary-rule violations.
    RuleViolation,
    /// Committed secrets.
    Secrets,
    /// Never fail (the default).
    None,
}

#[derive(Debug, Clone, Default)]
pub struct DiffOptions {
    /// An `onus.yaml` to use for both trees.
    pub config: Option<PathBuf>,
    pub intent: Option<Intent>,
    pub base_label: String,
    pub head_label: String,
    pub base_commit: Option<String>,
    pub head_commit: Option<String>,
    pub providers: Providers,
    /// SCIP indexes of each tree, produced elsewhere.
    pub base_scip: Vec<PathBuf>,
    pub head_scip: Vec<PathBuf>,
    /// Where to keep maps of base commits between runs (see [`MapCache`]).
    pub cache_dir: Option<PathBuf>,
}

/// Plugins and trusted mode (ADR 0006), the same for every tree.
#[derive(Debug, Clone, Default)]
pub struct Providers {
    pub plugins: onus_map::plugin::PluginsFile,
    pub trusted: bool,
    pub allow_unsandboxed: bool,
    /// Per-file facts shared between builds; see [`onus_map::FactsCache`].
    pub facts_cache: Option<std::sync::Arc<onus_map::FactsCache>>,
}

impl Providers {
    /// The plugins file from `--plugins`, else `ONUS_PLUGINS`, else none.
    pub fn load(path: Option<&Path>, trusted: bool, allow_unsandboxed: bool) -> Result<Self> {
        let from_env = std::env::var_os("ONUS_PLUGINS").map(PathBuf::from);
        let plugins = match path.map(Path::to_path_buf).or(from_env) {
            Some(p) => onus_map::plugin::load_plugins_file(&p)?,
            None => Default::default(),
        };
        Ok(Providers {
            plugins,
            trusted,
            allow_unsandboxed,
            facts_cache: None,
        })
    }
}

#[derive(Debug)]
pub struct Outcome {
    pub report: SemanticReport,
    pub base_map: CodebaseMap,
    pub head_map: CodebaseMap,
    /// Notes for stderr, such as whether the map cache was used.
    pub notes: Vec<String>,
}

impl Outcome {
    pub fn render(&self, format: Format) -> String {
        match format {
            Format::Json => onus_report::to_json(&self.report),
            Format::Md => onus_report::to_markdown(&self.report, !self.head_map.rules.is_empty()),
        }
    }
}

/// Maps of base commits kept between runs. A pull request is usually
/// reported many times against the same base commit; its map only depends
/// on the commit and on how Onus builds maps, so it is built once.
///
/// The file name is a hash of everything that shapes the map: the commit,
/// the Onus version, the config (with its packs), the plugins file and
/// trusted mode, and the bytes of every base SCIP index. A changed input
/// gives a new file; nothing is ever updated in place.
#[derive(Debug, Clone)]
pub struct MapCache {
    pub path: PathBuf,
    commit: String,
}

/// What [`MapCache::load`] found.
#[derive(Debug)]
pub enum CacheLoad {
    Hit(Box<CodebaseMap>),
    Miss,
    /// A file that is not a map of this commit; it is rebuilt.
    Unreadable,
}

impl MapCache {
    pub fn new(
        dir: &Path,
        commit: &str,
        config: Option<&LoadedConfig>,
        providers: &Providers,
        scip: &[PathBuf],
    ) -> Result<MapCache> {
        let mut key = format!(
            "onus map cache 1\nonus {}\ncommit {commit}\nconfig {}\ntrusted {} {}\nplugins {}\n",
            env!("CARGO_PKG_VERSION"),
            config.map(|c| c.hash.as_str()).unwrap_or("none"),
            providers.trusted,
            providers.allow_unsandboxed,
            serde_json::to_string(&providers.plugins)?,
        );
        for text in &providers.plugins.pack_texts {
            key.push_str(&format!(
                "pack {}\n",
                onus_core::hash::sha256_hex(text.as_bytes())
            ));
        }
        for path in scip {
            let bytes = std::fs::read(path)
                .with_context(|| format!("cannot read SCIP index {}", path.display()))?;
            key.push_str(&format!("scip {}\n", onus_core::hash::sha256_hex(&bytes)));
        }
        let name = format!(
            "map-{}.json",
            &onus_core::hash::sha256_hex(key.as_bytes())[..32]
        );
        Ok(MapCache {
            path: dir.join(name),
            commit: commit.to_string(),
        })
    }

    /// The cached map of this commit, if there is one.
    pub fn load(&self) -> CacheLoad {
        let Ok(text) = std::fs::read_to_string(&self.path) else {
            return CacheLoad::Miss;
        };
        match serde_json::from_str::<CodebaseMap>(&text) {
            Ok(map) if map.commit == self.commit => CacheLoad::Hit(Box::new(map)),
            _ => CacheLoad::Unreadable,
        }
    }

    /// Saves `map`. A cache that cannot be written only costs time later,
    /// so callers report the error and carry on.
    pub fn store(&self, map: &CodebaseMap) -> Result<()> {
        let dir = self.path.parent().context("cache path has no folder")?;
        std::fs::create_dir_all(dir)?;
        // Write a sibling file, then rename it, so a reader never sees half
        // a map.
        let tmp = tempfile::NamedTempFile::new_in(dir)?;
        serde_json::to_writer(std::io::BufWriter::new(tmp.as_file()), map)?;
        tmp.persist(&self.path)?;
        Ok(())
    }
}

/// The changes in meaning between `base` and the worktree at `root` as it
/// is now, given the worktree's current map. The base map comes from the
/// repository's map cache, or is built with the shared facts cache, which
/// already holds every file the two share.
pub fn check_worktree(
    root: &Path,
    base: &str,
    head_map: &CodebaseMap,
    facts: std::sync::Arc<onus_map::FactsCache>,
    bases: &BaseTrees,
) -> Result<serde_json::Value> {
    let sha = resolve_commit(root, base)?;
    let b = bases.get(root, &sha)?;
    let config = diff_config(b.dir.path(), None)?;
    let providers = Providers {
        facts_cache: Some(facts),
        ..Providers::default()
    };
    let cache = match daemon::git_common_dir(root) {
        Some(dir) => Some(MapCache::new(
            &dir.join("onus").join("maps"),
            &b.sha,
            config.as_ref(),
            &providers,
            &[],
        )?),
        None => None,
    };
    let base_map = match b.map.get().cloned() {
        Some(map) => map,
        None => {
            let map = match cache.as_ref().map(MapCache::load) {
                Some(CacheLoad::Hit(map)) => *map,
                _ => {
                    let map = build_with(
                        b.dir.path(),
                        config.clone(),
                        Some(b.sha.clone()),
                        &providers,
                        &[],
                    )?;
                    if let Some(c) = &cache {
                        // A cache that cannot be written only costs time later.
                        let _ = c.store(&map);
                    }
                    map
                }
            };
            let map = std::sync::Arc::new(map);
            let _ = b.map.set(map.clone());
            map
        }
    };
    let report = onus_diff::diff(&DiffInput {
        base_root: b.dir.path(),
        head_root: root,
        base_map: &base_map,
        head_map,
        config: config.as_ref().map(|c| &c.config),
        intent: None,
        base_label: &ref_label(base, &sha),
        head_label: "worktree",
    });
    Ok(serde_json::json!({
        "base": report.base,
        "summary": report.summary,
        "markdown": onus_report::to_markdown(&report, !head_map.rules.is_empty()),
    }))
}

/// Extracted base commits and their maps, kept by a long-running server so
/// that checks against the same commit skip `git archive` and mapping.
#[derive(Debug, Default)]
pub struct BaseTrees {
    trees: std::sync::Mutex<Vec<std::sync::Arc<BaseTree>>>,
}

/// One extracted commit.
#[derive(Debug)]
pub struct BaseTree {
    pub sha: String,
    pub dir: tempfile::TempDir,
    pub map: std::sync::OnceLock<std::sync::Arc<CodebaseMap>>,
}

/// How many base commits a server keeps extracted.
const BASE_TREES: usize = 4;

impl BaseTrees {
    /// The extracted tree of `sha`, extracting it on first use. The most
    /// recently used commits are kept.
    pub fn get(&self, repo: &Path, sha: &str) -> Result<std::sync::Arc<BaseTree>> {
        let mut trees = self
            .trees
            .lock()
            .map_err(|_| anyhow::anyhow!("base trees poisoned"))?;
        if let Some(i) = trees.iter().position(|t| t.sha == sha) {
            let t = trees.remove(i);
            trees.push(t.clone());
            return Ok(t);
        }
        let m = materialize(repo, sha)?;
        let tree = std::sync::Arc::new(BaseTree {
            sha: m.sha,
            dir: m.dir,
            map: std::sync::OnceLock::new(),
        });
        if trees.len() >= BASE_TREES {
            trees.remove(0);
        }
        trees.push(tree.clone());
        Ok(tree)
    }
}

/// The commit `reference` names in `repo`.
pub fn resolve_commit(repo: &Path, reference: &str) -> Result<String> {
    let spec = format!("{reference}^{{commit}}");
    Ok(String::from_utf8(
        git(repo, &["rev-parse", "--verify", "--quiet", &spec])
            .with_context(|| format!("unknown ref `{reference}`"))?,
    )?
    .trim()
    .to_string())
}

/// The config both maps are built with: `--config` if given, else the base
/// tree's `onus.yaml`. A pull request never gets to relax its own checks;
/// a changed `onus.yaml` is reported as rules of the game instead.
pub fn diff_config(base: &Path, explicit: Option<&Path>) -> Result<Option<LoadedConfig>> {
    match explicit {
        Some(p) => Ok(Some(onus_map::config::load(p)?)),
        None => Ok(onus_map::config::load_from_tree(base)?),
    }
}

pub fn build(
    root: &Path,
    config: Option<LoadedConfig>,
    commit: Option<String>,
) -> Result<CodebaseMap> {
    build_with(root, config, commit, &Providers::default(), &[])
}

pub fn build_with(
    root: &Path,
    config: Option<LoadedConfig>,
    commit: Option<String>,
    providers: &Providers,
    scip: &[PathBuf],
) -> Result<CodebaseMap> {
    Ok(onus_map::build_map(
        root,
        &BuildOptions {
            config,
            ignore_tree_config: true,
            commit,
            plugins: providers.plugins.clone(),
            trusted: providers.trusted,
            allow_unsandboxed: providers.allow_unsandboxed,
            scip_indexes: scip.to_vec(),
            facts_cache: providers.facts_cache.clone(),
        },
    )?)
}

/// Maps both trees and diffs them.
pub fn diff_dirs(base: &Path, head: &Path, opts: &DiffOptions) -> Result<Outcome> {
    if !base.is_dir() {
        bail!("base directory {} does not exist", base.display());
    }
    if !head.is_dir() {
        bail!("head directory {} does not exist", head.display());
    }
    let config = diff_config(base, opts.config.as_deref())?;
    let cache = match (&opts.cache_dir, &opts.base_commit) {
        (Some(dir), Some(commit)) => Some(MapCache::new(
            dir,
            commit,
            config.as_ref(),
            &opts.providers,
            &opts.base_scip,
        )?),
        _ => None,
    };
    let mut notes = Vec::new();
    let cached = match &cache {
        Some(c) => match c.load() {
            CacheLoad::Hit(map) => {
                notes.push(format!(
                    "using the cached map of {}",
                    ref_label(&c.commit, &c.commit)
                ));
                Some(*map)
            }
            CacheLoad::Miss => None,
            CacheLoad::Unreadable => {
                notes.push(format!(
                    "ignoring unreadable cached map {}",
                    c.path.display()
                ));
                None
            }
        },
        None => None,
    };
    let base_map = match cached {
        Some(map) => map,
        None => {
            let map = build_with(
                base,
                config.clone(),
                opts.base_commit.clone(),
                &opts.providers,
                &opts.base_scip,
            )?;
            if let Some(c) = &cache
                && let Err(e) = c.store(&map)
            {
                notes.push(format!(
                    "cannot write the map cache {}: {e:#}",
                    c.path.display()
                ));
            }
            map
        }
    };
    let head_map = build_with(
        head,
        config.clone(),
        opts.head_commit.clone(),
        &opts.providers,
        &opts.head_scip,
    )?;
    let report = onus_diff::diff(&DiffInput {
        base_root: base,
        head_root: head,
        base_map: &base_map,
        head_map: &head_map,
        config: config.as_ref().map(|c| &c.config),
        intent: opts.intent.as_ref(),
        base_label: &opts.base_label,
        head_label: &opts.head_label,
    });
    Ok(Outcome {
        report,
        base_map,
        head_map,
        notes,
    })
}

pub fn read_intent(path: &Path) -> Result<Option<Intent>> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("cannot read intent file {}", path.display()))?;
    let markdown = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "md" | "markdown" | "txt"));
    if markdown {
        Ok(onus_diff::intent::parse_markdown(&text)?)
    } else {
        Ok(onus_diff::intent::parse(&text)?)
    }
}

/// Whether any `--fail-on` condition is met.
pub fn should_fail(report: &SemanticReport, fail_on: &[FailOn]) -> bool {
    fail_on.iter().any(|f| match f {
        FailOn::RuleViolation => report.summary.new_rule_violations > 0,
        FailOn::Secrets => report.summary.secrets > 0,
        FailOn::None => false,
    })
}

/// A git ref checked out into a temporary directory with `git archive`.
/// The directory is removed when this value is dropped.
#[derive(Debug)]
pub struct Materialized {
    pub dir: tempfile::TempDir,
    pub sha: String,
}

fn git(repo: &Path, args: &[&str]) -> Result<Vec<u8>> {
    // Committed bytes, whatever the machine's line-ending settings.
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["-c", "core.autocrlf=false"])
        .args(args)
        .output()
        .context("cannot run git; is it installed?")?;
    if !out.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(out.stdout)
}

/// Resolves `reference` in `repo` and extracts its tree. Only reads the
/// repository; nothing in the tree is executed.
pub fn materialize(repo: &Path, reference: &str) -> Result<Materialized> {
    let spec = format!("{reference}^{{commit}}");
    let sha = String::from_utf8(
        git(repo, &["rev-parse", "--verify", "--quiet", &spec])
            .with_context(|| format!("unknown ref `{reference}`"))?,
    )?
    .trim()
    .to_string();
    let archive = git(repo, &["archive", "--format=tar", &sha])?;
    let dir = tempfile::Builder::new().prefix("onus-").tempdir()?;
    tar::Archive::new(archive.as_slice())
        .unpack(dir.path())
        .context("cannot extract git archive")?;
    Ok(Materialized { dir, sha })
}

/// `main (abc1234)`
pub fn ref_label(reference: &str, sha: &str) -> String {
    let short = &sha[..sha.len().min(7)];
    if reference == sha || sha.starts_with(reference) {
        short.to_string()
    } else {
        format!("{reference} ({short})")
    }
}
