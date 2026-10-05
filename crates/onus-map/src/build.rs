//! Assembling the map.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use globset::{Glob, GlobSet, GlobSetBuilder};
use onus_core::ids::external_id;
use onus_core::{
    BoundaryRule, BuiltWith, CodebaseMap, Confidence, EdgeKind, ExternalService, ExternalSource,
    LanguageAdapter, MapDiagnostic, ONUS_VERSION, OnusConfig, RuleEndpoints, SCHEMA_VERSION,
    Workspace, WorkspaceFile, WorkspacePackage,
};
use onus_lang_ts::{TypeScriptAdapter, component_dir};

use crate::config::{self, LoadedConfig};
use crate::discover::{self, ComponentMatcher};
use crate::registry::resolve_extractors;
use crate::{MapError, walk};

/// Globs of test files when `onus.yaml` declares none.
pub const DEFAULT_TEST_GLOBS: &[&str] = &[
    "**/*.test.*",
    "**/*.spec.*",
    "**/__tests__/**",
    "**/test/**",
    "**/tests/**",
    "**/e2e/**",
];

#[derive(Debug, Clone, Default)]
pub struct BuildOptions {
    /// Use this config instead of the tree's own `onus.yaml`.
    pub config: Option<LoadedConfig>,
    /// Do not read the tree's own `onus.yaml` when `config` is `None`.
    pub ignore_tree_config: bool,
    /// Recorded as `commit` in the map.
    pub commit: Option<String>,
}

/// Builds the map of the tree at `root`. Reads files only.
pub fn build_map(root: &Path, opts: &BuildOptions) -> Result<CodebaseMap, MapError> {
    let loaded = match &opts.config {
        Some(c) => Some(c.clone()),
        None if opts.ignore_tree_config => None,
        None => config::load_from_tree(root)?,
    };
    let cfg: Option<&OnusConfig> = loaded.as_ref().map(|l| &l.config);
    let files = walk::list_files(root);
    let discovered = discover::discover(root, &files, cfg);
    let mut components = discovered.components;
    let matcher = ComponentMatcher::new(&components);
    let tests = test_globs(cfg);

    let adapter = TypeScriptAdapter;
    let ws_files: Vec<WorkspaceFile> = files
        .iter()
        .map(|f| WorkspaceFile {
            path: f.clone(),
            component: matcher.component_of(f),
            is_test: adapter.handles(f) && tests.is_match(f),
        })
        .collect();
    let packages: BTreeMap<String, WorkspacePackage> = components
        .iter()
        .filter_map(|c| {
            let name = c.package_name.clone()?;
            Some((
                name.clone(),
                WorkspacePackage {
                    name,
                    component: c.id.clone(),
                    dir: component_dir(&c.roots),
                    entrypoints: c.public_entrypoints.clone(),
                },
            ))
        })
        .collect();
    let extractors = resolve_extractors(cfg);
    let workspace = Workspace {
        root: root.to_path_buf(),
        files: ws_files,
        components: components.clone(),
        packages,
        extractors: extractors.clone(),
    };
    let partial = adapter.build(&workspace);

    // Externals actually called.
    let mut used: BTreeSet<String> = BTreeSet::new();
    for e in &partial.edges {
        if e.kind == EdgeKind::CallsExternal {
            used.insert(e.to.clone());
        }
    }
    let mut externals = Vec::new();
    for id in used {
        let slug = id.strip_prefix("external:").unwrap_or(&id);
        if let Some(host) = slug.strip_prefix("host:") {
            externals.push(ExternalService {
                id: id.clone(),
                vendor: host.to_string(),
                category: "http".into(),
                egress: vec![],
                packages: vec![],
                hosts: vec![host.to_string()],
                source: ExternalSource::Observed,
                confidence: Confidence::Static,
            });
            continue;
        }
        let entries: Vec<(&String, &onus_core::ResolvedExternal)> = extractors
            .externals
            .iter()
            .filter(|(_, e)| e.slug == slug)
            .collect();
        let Some((_, first)) = entries.first() else {
            continue;
        };
        let declared = entries.iter().any(|(_, e)| e.declared);
        let mut hosts: Vec<String> = entries
            .iter()
            .flat_map(|(_, e)| e.spec.hosts.clone())
            .collect();
        hosts.sort();
        hosts.dedup();
        externals.push(ExternalService {
            id: external_id(slug),
            vendor: first.spec.vendor.clone(),
            category: first.spec.category.clone(),
            egress: first.spec.egress.clone(),
            packages: entries.iter().map(|(p, _)| (*p).clone()).collect(),
            hosts,
            source: if declared {
                ExternalSource::Declared
            } else {
                ExternalSource::BuiltIn
            },
            confidence: if declared {
                Confidence::Declared
            } else {
                Confidence::Static
            },
        });
    }

    let package_deps = discover::package_deps(root, &files, &components, &matcher);

    let rules: Vec<BoundaryRule> = cfg
        .map(|c| {
            c.rules
                .iter()
                .map(|r| BoundaryRule {
                    id: format!("deny:{}->{}", r.deny.from, r.deny.to),
                    deny: RuleEndpoints {
                        from: r.deny.from.clone(),
                        to: r.deny.to.clone(),
                    },
                    confidence: Confidence::Declared,
                })
                .collect()
        })
        .unwrap_or_default();

    let mut symbols = partial.symbols;
    if let Some(cfg) = cfg {
        for contract in cfg.contracts.values() {
            if let Some(s) = symbols.iter_mut().find(|s| s.id == contract.symbol) {
                s.invariants = contract.invariants.clone();
            }
        }
    }

    let mut diagnostics = partial.diagnostics;
    for c in &mut components {
        c.owners.sort();
        if c.public_entrypoints.is_empty() {
            diagnostics.push(MapDiagnostic {
                kind: "no-entrypoint".into(),
                file: component_dir(&c.roots),
                line: 0,
                message: format!(
                    "component `{}` has no entrypoint; all of its symbols are treated as internal",
                    c.id
                ),
                confidence: Confidence::Low,
            });
        }
    }
    if let Some(cfg) = cfg {
        for (name, contract) in &cfg.contracts {
            if !symbols.iter().any(|s| s.id == contract.symbol) {
                diagnostics.push(MapDiagnostic {
                    kind: "unknown-contract".into(),
                    file: config::CONFIG_FILE.into(),
                    line: 0,
                    message: format!(
                        "contract `{name}` names `{}`, which is not in the map",
                        contract.symbol
                    ),
                    confidence: Confidence::Low,
                });
            }
        }
    }
    diagnostics.sort();
    diagnostics.dedup();

    let mut files_out = partial.files;
    files_out.sort_by(|a, b| a.path.cmp(&b.path));
    let mut tests_out = partial.tests;
    tests_out.sort_by(|a, b| a.id.cmp(&b.id));
    let mut adapters = BTreeMap::new();
    adapters.insert(adapter.id().to_string(), adapter.version());

    Ok(CodebaseMap {
        schema_version: SCHEMA_VERSION,
        commit: opts.commit.clone().unwrap_or_else(|| "worktree".into()),
        built_with: BuiltWith {
            onus: ONUS_VERSION.to_string(),
            adapters,
            config_hash: loaded
                .as_ref()
                .map(|l| l.hash.clone())
                .unwrap_or_else(|| "inferred".into()),
        },
        components,
        files: files_out,
        symbols,
        edges: partial.edges,
        externals,
        packages: package_deps,
        tests: tests_out,
        rules,
        diagnostics,
    })
}

pub fn test_globs(cfg: Option<&OnusConfig>) -> GlobSet {
    let globs: Vec<String> = match cfg {
        Some(c) if !c.tests.globs.is_empty() => c.tests.globs.clone(),
        _ => DEFAULT_TEST_GLOBS.iter().map(|s| s.to_string()).collect(),
    };
    let mut b = GlobSetBuilder::new();
    for g in globs {
        if let Ok(glob) = Glob::new(&g) {
            b.add(glob);
        }
    }
    b.build().unwrap_or_else(|_| GlobSet::empty())
}
