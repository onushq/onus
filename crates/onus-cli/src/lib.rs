//! The pieces behind the `onus` command, shared by the binary and its tests.

pub mod guide;

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
}

/// Plugins and trusted mode (ADR 0006), the same for every tree.
#[derive(Debug, Clone, Default)]
pub struct Providers {
    pub plugins: onus_map::plugin::PluginsFile,
    pub trusted: bool,
    pub allow_unsandboxed: bool,
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
        })
    }
}

#[derive(Debug)]
pub struct Outcome {
    pub report: SemanticReport,
    pub base_map: CodebaseMap,
    pub head_map: CodebaseMap,
}

impl Outcome {
    pub fn render(&self, format: Format) -> String {
        match format {
            Format::Json => onus_report::to_json(&self.report),
            Format::Md => onus_report::to_markdown(&self.report, !self.head_map.rules.is_empty()),
        }
    }
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
    let base_map = build_with(
        base,
        config.clone(),
        opts.base_commit.clone(),
        &opts.providers,
        &opts.base_scip,
    )?;
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
