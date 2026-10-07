//! Assembling the map.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use globset::{Glob, GlobSet, GlobSetBuilder};
use onus_core::ids::external_id;
use onus_core::{
    BoundaryRule, BuiltWith, CodebaseMap, Confidence, EdgeKind, ExternalService, ExternalSource,
    FactProvider, LanguageAdapter, MapDiagnostic, ONUS_VERSION, OnusConfig, PartialMap,
    ProviderError, RuleEndpoints, SCHEMA_VERSION, Workspace, WorkspaceFile, WorkspacePackage,
};
use onus_lang_ts::{TypeScriptAdapter, component_dir};

use crate::config::{self, LoadedConfig};
use crate::discover::{self, ComponentMatcher};
use crate::merge;
use crate::plugin::{ExternalPlugin, PluginsFile, Runner, SpecKind, run_scip_indexer};
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
    /// Plugins and their sandbox, from whoever runs Onus (never from the
    /// analyzed tree).
    pub plugins: PluginsFile,
    /// Allow providers that may run repository code (in a sandbox).
    pub trusted: bool,
    /// Allow them without a sandbox.
    pub allow_unsandboxed: bool,
    /// SCIP indexes produced elsewhere; importing them runs nothing.
    pub scip_indexes: Vec<PathBuf>,
    /// Per-file facts kept between builds (a long-running server, or a
    /// folder shared by worktrees). The map is the same with or without it.
    pub facts_cache: Option<std::sync::Arc<onus_lang_ts::FactsCache>>,
}

fn provider_note(id: &str, err: &ProviderError) -> MapDiagnostic {
    let (kind, confidence) = match err {
        ProviderError::Skipped(_) => ("provider-skipped", Confidence::Inferred),
        ProviderError::Failed(_) => ("provider-failed", Confidence::Low),
    };
    MapDiagnostic {
        kind: kind.into(),
        file: String::new(),
        line: 0,
        message: format!("provider `{id}` {err}"),
        confidence,
    }
}

/// Builds the map of the tree at `root`. Built-in providers only read
/// files; plugins run only as `opts` allows (ADR 0006).
pub fn build_map(root: &Path, opts: &BuildOptions) -> Result<CodebaseMap, MapError> {
    // Providers and plugins get an absolute root: plugins run inside it, and
    // language servers need file URIs. Nothing absolute reaches the map.
    let absolute =
        std::path::absolute(root).map_err(|e| MapError::Io(format!("{}: {e}", root.display())))?;
    let root = absolute.as_path();
    let loaded = match &opts.config {
        Some(c) => Some(c.clone()),
        None if opts.ignore_tree_config => None,
        None => config::load_from_tree(root)?,
    };
    let cfg: Option<&OnusConfig> = loaded.as_ref().map(|l| &l.config);
    let test_data = test_data_globs(cfg);
    let files: Vec<String> = walk::list_files(root)
        .into_iter()
        .filter(|f| !test_data.is_match(f))
        .collect();
    let file_set: BTreeSet<String> = files.iter().cloned().collect();
    let runner = Runner {
        sandbox: opts.plugins.sandbox.clone(),
        trusted: opts.trusted,
        allow_unsandboxed: opts.allow_unsandboxed,
    };
    let mut providers: BTreeMap<String, String> = BTreeMap::new();
    let mut notes: Vec<MapDiagnostic> = Vec::new();

    // Discovery: onus.yaml, else Nx, workspaces and folders, plus plugins.
    let discovered = discover::discover(root, &files, cfg);
    providers.insert(
        "discovery".into(),
        format!("onus {ONUS_VERSION} ({})", discovered.source),
    );
    let mut components = discovered.components;
    let declared = cfg.is_some_and(|c| !c.components.is_empty());
    for spec in opts
        .plugins
        .plugins
        .iter()
        .filter(|p| p.kind == SpecKind::Discovery)
    {
        if declared {
            continue;
        }
        let plugin = ExternalPlugin::new(spec.clone(), runner.clone())?;
        match onus_core::DiscoveryProvider::discover(&plugin, root, &files) {
            Ok(found) => {
                let taken: BTreeSet<String> = components.iter().map(|c| c.id.clone()).collect();
                for c in found {
                    let valid = !c.id.is_empty()
                        && !c.id.contains([':', '#', '/', ' '])
                        && !c.roots.is_empty()
                        && c.roots
                            .iter()
                            .all(|r| Glob::new(r).is_ok() && !r.starts_with('/'));
                    if valid && !taken.contains(&c.id) {
                        components.push(c);
                    } else {
                        notes.push(provider_note(
                            &spec.name,
                            &ProviderError::Failed(format!(
                                "component `{}` is invalid or taken",
                                c.id
                            )),
                        ));
                    }
                }
                providers.insert(
                    spec.name.clone(),
                    onus_core::DiscoveryProvider::version(&plugin),
                );
            }
            Err(e) => notes.push(provider_note(&spec.name, &e)),
        }
    }
    components.sort_by(|a, b| a.id.cmp(&b.id));
    let matcher = ComponentMatcher::new(&components);
    let component_ids: BTreeSet<String> = components.iter().map(|c| c.id.clone()).collect();
    let tests = test_globs(cfg);

    // Language providers, in priority order: the built-in TypeScript adapter,
    // protocol plugins, then language servers.
    let mut languages: Vec<(Box<dyn LanguageAdapter>, bool)> = vec![(
        Box::new(match &opts.facts_cache {
            Some(c) => TypeScriptAdapter::with_cache(c.clone()),
            None => TypeScriptAdapter::default(),
        }),
        false,
    )];
    let mut keep_alive = Vec::new();
    for spec in &opts.plugins.plugins {
        match spec.kind {
            SpecKind::Language => {
                languages.push((
                    Box::new(ExternalPlugin::new(spec.clone(), runner.clone())?),
                    true,
                ));
            }
            SpecKind::Lsp => match runner.prepare(spec, root) {
                Ok(prepared) => {
                    languages.push((
                        Box::new(onus_lang_lsp::LspAdapter::new(
                            spec.name.clone(),
                            prepared.argv.clone(),
                            crate::plugin::file_globs(spec)?,
                            spec.language_id.clone().unwrap_or_default(),
                            spec.timeout(),
                        )),
                        true,
                    ));
                    keep_alive.push(prepared);
                }
                Err(e) => notes.push(provider_note(&spec.name, &e)),
            },
            _ => {}
        }
    }
    let ws_files: Vec<WorkspaceFile> = files
        .iter()
        .map(|f| WorkspaceFile {
            path: f.clone(),
            component: matcher.component_of(f),
            is_test: tests.is_match(f) && languages.iter().any(|(l, _)| l.handles(f)),
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
    let mut pack_texts: Vec<String> = loaded.as_ref().map(|l| l.packs.clone()).unwrap_or_default();
    pack_texts.extend(opts.plugins.pack_texts.iter().cloned());
    let extractors = resolve_extractors(cfg, pack_texts);
    let workspace = Workspace {
        root: root.to_path_buf(),
        files: ws_files,
        components: components.clone(),
        packages,
        extractors: extractors.clone(),
    };
    // A broken pack is a configuration error, not a quiet loss of facts.
    onus_lang_ts::patterns(&workspace).map_err(MapError::Config)?;
    let mut partial = PartialMap::default();
    let mut claimed: BTreeSet<String> = BTreeSet::new();
    for (lang, external) in &languages {
        let mine: Vec<WorkspaceFile> = workspace
            .files
            .iter()
            .filter(|f| lang.handles(&f.path) && !claimed.contains(&f.path))
            .cloned()
            .collect();
        if mine.is_empty() {
            continue;
        }
        claimed.extend(mine.iter().map(|f| f.path.clone()));
        // The built-in adapter sees every file (tsconfig, package.json);
        // plugins get the files they handle and may read the rest.
        let view = if *external {
            Workspace {
                files: mine,
                ..workspace.clone()
            }
        } else {
            workspace.clone()
        };
        match lang.build(&view) {
            Ok(out) => {
                let out = if *external {
                    merge::validate(lang.id(), out, &file_set, &component_ids)
                } else {
                    out
                };
                merge::merge(&mut partial, out);
                providers.insert(lang.id().to_string(), lang.version());
            }
            Err(e) => notes.push(provider_note(lang.id(), &e)),
        }
    }

    // Fact providers: SCIP indexes first (compiler-confirmed references),
    // then fact plugins, which see everything found so far.
    let mut facts: Vec<Box<dyn FactProvider>> = Vec::new();
    for path in &opts.scip_indexes {
        facts.push(Box::new(onus_lang_scip::ScipImport::new(
            format!(
                "scip:{}",
                path.file_name()
                    .map_or("index".into(), |n| n.to_string_lossy().into_owned())
            ),
            path.clone(),
        )));
    }
    for spec in opts
        .plugins
        .plugins
        .iter()
        .filter(|p| p.kind == SpecKind::Scip)
    {
        match run_scip_indexer(&runner, spec, root) {
            Ok((prepared, index)) => {
                facts.push(Box::new(onus_lang_scip::ScipImport::new(
                    spec.name.clone(),
                    index,
                )));
                keep_alive.push(prepared);
            }
            Err(e) => notes.push(provider_note(&spec.name, &e)),
        }
    }
    for spec in opts
        .plugins
        .plugins
        .iter()
        .filter(|p| p.kind == SpecKind::Facts)
    {
        facts.push(Box::new(ExternalPlugin::new(spec.clone(), runner.clone())?));
    }
    for provider in &facts {
        match provider.facts(&workspace, &partial) {
            Ok(out) => {
                let out = merge::validate(provider.id(), out, &file_set, &component_ids);
                merge::merge(&mut partial, out);
                providers.insert(provider.id().to_string(), provider.version());
            }
            Err(e) => notes.push(provider_note(provider.id(), &e)),
        }
    }
    drop(keep_alive);
    partial.diagnostics.extend(notes);

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
        // Applications have no public surface by design; only libraries
        // without an entrypoint are worth a note.
        let js_package = file_set.contains(&discover::join_dir(
            &component_dir(&c.roots),
            "package.json",
        ));
        if c.public_entrypoints.is_empty()
            && c.kind == onus_core::ComponentKind::Package
            && js_package
        {
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

    Ok(CodebaseMap {
        schema_version: SCHEMA_VERSION,
        commit: opts.commit.clone().unwrap_or_else(|| "worktree".into()),
        built_with: BuiltWith {
            onus: ONUS_VERSION.to_string(),
            providers,
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

/// The `testData` globs of onus.yaml.
pub fn test_data_globs(cfg: Option<&OnusConfig>) -> GlobSet {
    let mut b = GlobSetBuilder::new();
    for g in cfg.map(|c| c.test_data.as_slice()).unwrap_or_default() {
        if let Ok(glob) = Glob::new(g) {
            b.add(glob);
        }
    }
    b.build().unwrap_or_else(|_| GlobSet::empty())
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
