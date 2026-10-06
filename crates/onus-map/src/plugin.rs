//! External plugins (ADR 0006): a plugins file says which commands to run,
//! the runner starts them with a JSON request on stdin, enforces timeouts,
//! and wraps the ones that run repository code in a sandbox, only in
//! trusted mode.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use globset::{Glob, GlobSet, GlobSetBuilder};
use onus_core::protocol::{PLUGIN_PROTOCOL_VERSION, PluginKind, PluginRequest, PluginResponse};
use onus_core::{
    Component, DiscoveryProvider, FactProvider, LanguageAdapter, PartialMap, ProviderError,
    Workspace,
};

use crate::MapError;

pub use onus_core::plugins::{PluginSpec, PluginsFile, SandboxPreset, SandboxSpec, SpecKind};

/// The globs of the files a plugin handles.
pub fn file_globs(spec: &PluginSpec) -> Result<GlobSet, MapError> {
    let mut b = GlobSetBuilder::new();
    for g in &spec.files {
        b.add(Glob::new(g).map_err(|e| {
            MapError::Config(format!("plugin `{}`: invalid glob `{g}`: {e}", spec.name))
        })?);
    }
    b.build()
        .map_err(|e| MapError::Config(format!("plugin `{}`: {e}", spec.name)))
}

pub fn load_plugins_file(path: &Path) -> Result<PluginsFile, MapError> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| MapError::Io(format!("{}: {e}", path.display())))?;
    let file: PluginsFile = serde_yaml_ng::from_str(&text)
        .map_err(|e| MapError::Config(format!("{}: {e}", path.display())))?;
    let mut names = std::collections::BTreeSet::new();
    for p in &file.plugins {
        if p.command.is_empty() {
            return Err(MapError::Config(format!(
                "plugin `{}` has an empty command",
                p.name
            )));
        }
        if !names.insert(p.name.clone()) {
            return Err(MapError::Config(format!(
                "plugin name `{}` is used twice",
                p.name
            )));
        }
        if file.sandbox.preset == SandboxPreset::Container && file.sandbox.image.is_none() {
            return Err(MapError::Config(
                "sandbox preset `container` needs an `image`".into(),
            ));
        }
        if matches!(p.kind, SpecKind::Language | SpecKind::Lsp) && p.files.is_empty() {
            return Err(MapError::Config(format!(
                "plugin `{}` needs `files` globs for the files it handles",
                p.name
            )));
        }
        if p.kind == SpecKind::Lsp && p.language_id.is_none() {
            return Err(MapError::Config(format!(
                "plugin `{}` needs a `language_id`",
                p.name
            )));
        }
        file_globs(p)?;
    }
    Ok(file)
}

/// Runs plugin commands.
#[derive(Debug, Clone, Default)]
pub struct Runner {
    pub sandbox: SandboxSpec,
    /// Allows plugins that run repository code.
    pub trusted: bool,
    /// Allows them without a sandbox.
    pub allow_unsandboxed: bool,
}

/// A prepared command: its argument vector and the temporary `{out}`
/// directory, removed when this value is dropped.
#[derive(Debug)]
pub struct Prepared {
    pub argv: Vec<String>,
    pub out: tempfile::TempDir,
    pub sandboxed: bool,
}

impl Runner {
    fn sandbox_prefix(&self) -> Option<Vec<String>> {
        let words = |w: &[&str]| w.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        if !self.sandbox.command.is_empty() {
            return Some(self.sandbox.command.clone());
        }
        let bwrap = || {
            words(&[
                "bwrap",
                "--ro-bind",
                "/",
                "/",
                "--dev",
                "/dev",
                "--proc",
                "/proc",
                // A fresh /tmp, then the tree (which may live in /tmp)
                // read-only and {out} writable on top of it.
                "--tmpfs",
                "/tmp",
                "--ro-bind",
                "{root}",
                "{root}",
                "--bind",
                "{out}",
                "{out}",
                "--unshare-net",
                "--die-with-parent",
                "--chdir",
                "{root}",
            ])
        };
        let sandbox_exec = || {
            words(&[
                "sandbox-exec",
                "-p",
                "(version 1)(allow default)(deny network*)(allow network* (local unix))\
                 (deny file-write*)(allow file-write* (subpath \"{out}\") (subpath \"/dev\") \
                 (subpath \"/private/var/folders\"))\
                 (deny file-write* (subpath \"{root}\"))",
            ])
        };
        let container = || -> Option<Vec<String>> {
            let image = self.sandbox.image.clone()?;
            let runtime = self.sandbox.runtime.clone().or_else(|| {
                ["docker", "podman"]
                    .into_iter()
                    .find(|r| on_path(r))
                    .map(str::to_string)
            })?;
            let mut argv = words(&[
                "run",
                "--rm",
                "-i",
                "--network",
                "none",
                "-v",
                "{root}:{root}:ro",
                "-v",
                "{out}:{out}",
                "-w",
                "{root}",
            ]);
            argv.insert(0, runtime);
            argv.push(image);
            Some(argv)
        };
        match self.sandbox.preset {
            SandboxPreset::None => None,
            SandboxPreset::Container => container(),
            SandboxPreset::Bwrap => Some(bwrap()),
            SandboxPreset::SandboxExec => Some(sandbox_exec()),
            SandboxPreset::Auto => {
                if cfg!(target_os = "linux") && on_path("bwrap") {
                    Some(bwrap())
                } else if cfg!(target_os = "macos") && Path::new("/usr/bin/sandbox-exec").exists() {
                    Some(sandbox_exec())
                } else {
                    None
                }
            }
        }
    }

    /// The argument vector for `spec`, sandboxed when it runs repository
    /// code; or why it may not run.
    pub fn prepare(&self, spec: &PluginSpec, root: &Path) -> Result<Prepared, ProviderError> {
        let out = tempfile::Builder::new()
            .prefix("onus-plugin-")
            .tempdir()
            .map_err(|e| {
                ProviderError::Failed(format!("cannot create a temporary directory: {e}"))
            })?;
        // Real paths: sandboxes match them (on macOS /var is /private/var).
        let real = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
        let root_s = real(root).to_string_lossy().into_owned();
        let out_s = real(out.path()).to_string_lossy().into_owned();
        let fill = |w: &str| w.replace("{root}", &root_s).replace("{out}", &out_s);
        let mut argv: Vec<String> = spec.command.iter().map(|w| fill(w)).collect();
        let mut sandboxed = false;
        if spec.runs_repo_code() {
            if !self.trusted {
                return Err(ProviderError::Skipped(format!(
                    "`{}` may run code from the repository; it runs only with --trusted",
                    spec.name
                )));
            }
            match self.sandbox_prefix() {
                Some(prefix) => {
                    let mut wrapped: Vec<String> = prefix.iter().map(|w| fill(w)).collect();
                    wrapped.append(&mut argv);
                    argv = wrapped;
                    sandboxed = true;
                }
                None if self.allow_unsandboxed => {}
                None => {
                    return Err(ProviderError::Failed(format!(
                        "`{}` may run code from the repository and no sandbox is available; \
                         configure `sandbox` in the plugins file or pass --allow-unsandboxed",
                        spec.name
                    )));
                }
            }
        }
        Ok(Prepared {
            argv,
            out,
            sandboxed,
        })
    }

    /// Runs a prepared command in `root` with `input` on stdin; returns its
    /// stdout.
    pub fn run(
        &self,
        prepared: &Prepared,
        root: &Path,
        input: &[u8],
        timeout: Duration,
    ) -> Result<Vec<u8>, ProviderError> {
        let (program, args) = prepared
            .argv
            .split_first()
            .ok_or_else(|| ProviderError::Failed("empty command".into()))?;
        let mut child = Command::new(program)
            .args(args)
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| ProviderError::Failed(format!("cannot start `{program}`: {e}")))?;
        let mut stdin = child.stdin.take();
        let input = input.to_vec();
        let writer = std::thread::spawn(move || {
            if let Some(s) = stdin.as_mut() {
                let _ = s.write_all(&input);
            }
        });
        let mut stdout = child.stdout.take();
        let reader = std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(s) = stdout.as_mut() {
                let _ = s.read_to_end(&mut buf);
            }
            buf
        });
        let mut stderr = child.stderr.take();
        let err_reader = std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(s) = stderr.as_mut() {
                let _ = s.read_to_end(&mut buf);
            }
            buf
        });
        let start = Instant::now();
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if start.elapsed() > timeout => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(ProviderError::Failed(format!(
                        "`{program}` timed out after {} s",
                        timeout.as_secs()
                    )));
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(10)),
                Err(e) => return Err(ProviderError::Failed(e.to_string())),
            }
        };
        let _ = writer.join();
        let out = reader.join().unwrap_or_default();
        let err = err_reader.join().unwrap_or_default();
        if !status.success() {
            // Keep machine-specific paths out of the map.
            let msg = String::from_utf8_lossy(&err)
                .replace(&*prepared.out.path().to_string_lossy(), "{out}")
                .replace(&*root.to_string_lossy(), "{root}");
            let msg = msg.trim();
            return Err(ProviderError::Failed(format!(
                "`{program}` exited with {status}{}",
                if msg.is_empty() {
                    String::new()
                } else {
                    format!(": {}", msg.chars().take(500).collect::<String>())
                }
            )));
        }
        Ok(out)
    }

    fn call(
        &self,
        spec: &PluginSpec,
        root: &Path,
        request: &PluginRequest,
    ) -> Result<PluginResponse, ProviderError> {
        let prepared = self.prepare(spec, root)?;
        let input = serde_json::to_vec(request)
            .map_err(|e| ProviderError::Failed(format!("cannot encode the request: {e}")))?;
        let out = self.run(&prepared, root, &input, spec.timeout())?;
        let response: PluginResponse = serde_json::from_slice(&out).map_err(|e| {
            ProviderError::Failed(format!("`{}` returned invalid JSON: {e}", spec.name))
        })?;
        if response.protocol != PLUGIN_PROTOCOL_VERSION {
            return Err(ProviderError::Failed(format!(
                "`{}` speaks protocol {}, Onus speaks {PLUGIN_PROTOCOL_VERSION}",
                spec.name, response.protocol
            )));
        }
        Ok(response)
    }
}

fn on_path(program: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
}

/// A discovery, language or facts plugin speaking the protocol.
#[derive(Debug)]
pub struct ExternalPlugin {
    pub spec: PluginSpec,
    pub runner: Runner,
    globs: GlobSet,
    version: std::sync::Mutex<Option<String>>,
}

impl ExternalPlugin {
    pub fn new(spec: PluginSpec, runner: Runner) -> Result<Self, MapError> {
        let globs = file_globs(&spec)?;
        Ok(ExternalPlugin {
            spec,
            runner,
            globs,
            version: std::sync::Mutex::new(None),
        })
    }

    fn remember_version(&self, v: Option<String>) {
        if let Ok(mut slot) = self.version.lock() {
            *slot = v;
        }
    }

    fn reported_version(&self) -> String {
        self.version
            .lock()
            .ok()
            .and_then(|v| v.clone())
            .unwrap_or_else(|| "plugin".into())
    }
}

impl DiscoveryProvider for ExternalPlugin {
    fn id(&self) -> &str {
        &self.spec.name
    }

    fn version(&self) -> String {
        self.reported_version()
    }

    fn discover(&self, root: &Path, files: &[String]) -> Result<Vec<Component>, ProviderError> {
        let workspace = Workspace {
            root: root.to_path_buf(),
            files: files
                .iter()
                .map(|f| onus_core::WorkspaceFile {
                    path: f.clone(),
                    component: None,
                    is_test: false,
                })
                .collect(),
            components: vec![],
            packages: Default::default(),
            extractors: Default::default(),
        };
        let response = self.runner.call(
            &self.spec,
            root,
            &PluginRequest {
                protocol: PLUGIN_PROTOCOL_VERSION,
                kind: PluginKind::Discovery,
                workspace,
                map: None,
            },
        )?;
        self.remember_version(response.version);
        Ok(response.components)
    }
}

impl LanguageAdapter for ExternalPlugin {
    fn id(&self) -> &str {
        &self.spec.name
    }

    fn version(&self) -> String {
        self.reported_version()
    }

    fn handles(&self, path: &str) -> bool {
        self.globs.is_match(path)
    }

    fn build(&self, workspace: &Workspace) -> Result<PartialMap, ProviderError> {
        let response = self.runner.call(
            &self.spec,
            &workspace.root,
            &PluginRequest {
                protocol: PLUGIN_PROTOCOL_VERSION,
                kind: PluginKind::Language,
                workspace: workspace.clone(),
                map: None,
            },
        )?;
        self.remember_version(response.version);
        Ok(response.map)
    }
}

impl FactProvider for ExternalPlugin {
    fn id(&self) -> &str {
        &self.spec.name
    }

    fn version(&self) -> String {
        self.reported_version()
    }

    fn facts(
        &self,
        workspace: &Workspace,
        so_far: &PartialMap,
    ) -> Result<PartialMap, ProviderError> {
        let response = self.runner.call(
            &self.spec,
            &workspace.root,
            &PluginRequest {
                protocol: PLUGIN_PROTOCOL_VERSION,
                kind: PluginKind::Facts,
                workspace: workspace.clone(),
                map: Some(so_far.clone()),
            },
        )?;
        self.remember_version(response.version);
        Ok(response.map)
    }
}

/// Runs a SCIP indexer and returns the path of the index it wrote, inside
/// the returned temporary directory.
pub fn run_scip_indexer(
    runner: &Runner,
    spec: &PluginSpec,
    root: &Path,
) -> Result<(Prepared, PathBuf), ProviderError> {
    let prepared = runner.prepare(spec, root)?;
    runner.run(&prepared, root, &[], spec.timeout())?;
    let index = prepared.out.path().join("index.scip");
    if !index.is_file() {
        return Err(ProviderError::Failed(format!(
            "`{}` did not write {{out}}/index.scip",
            spec.name
        )));
    }
    Ok((prepared, index))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(kind: SpecKind, command: &[&str]) -> PluginSpec {
        PluginSpec {
            name: "t".into(),
            kind,
            command: command.iter().map(|s| s.to_string()).collect(),
            files: vec![],
            language_id: None,
            runs_repo_code: None,
            timeout_seconds: Some(5),
        }
    }

    #[test]
    fn plugins_that_run_repo_code_need_trusted_mode_and_a_sandbox() {
        let root = tempfile::tempdir().unwrap();
        let scip = spec(SpecKind::Scip, &["indexer", "--out", "{out}/index.scip"]);
        let untrusted = Runner::default();
        assert!(matches!(
            untrusted.prepare(&scip, root.path()),
            Err(ProviderError::Skipped(_))
        ));
        let no_sandbox = Runner {
            trusted: true,
            sandbox: SandboxSpec {
                preset: SandboxPreset::None,
                ..SandboxSpec::default()
            },
            ..Runner::default()
        };
        assert!(matches!(
            no_sandbox.prepare(&scip, root.path()),
            Err(ProviderError::Failed(_))
        ));
        let custom = Runner {
            trusted: true,
            sandbox: SandboxSpec {
                preset: SandboxPreset::None,
                command: vec!["jail".into(), "--root={root}".into()],
                ..SandboxSpec::default()
            },
            ..Runner::default()
        };
        let p = custom.prepare(&scip, root.path()).unwrap();
        assert!(p.sandboxed);
        assert_eq!(p.argv[0], "jail");
        assert!(p.argv[1].starts_with("--root="));
        assert!(p.argv[4].ends_with("index.scip") && !p.argv[4].contains("{out}"));
        // A protocol plugin that does not run repo code needs neither.
        let facts = spec(SpecKind::Facts, &["plugin"]);
        let p = untrusted.prepare(&facts, root.path()).unwrap();
        assert!(!p.sandboxed);
        assert_eq!(p.argv, ["plugin"]);
    }

    #[test]
    fn the_container_preset_runs_without_network_and_with_a_read_only_tree() {
        let root = tempfile::tempdir().unwrap();
        let runner = Runner {
            trusted: true,
            sandbox: SandboxSpec {
                preset: SandboxPreset::Container,
                command: vec![],
                image: Some("ghcr.io/acme/indexers:1".into()),
                runtime: Some("podman".into()),
            },
            ..Runner::default()
        };
        let p = runner
            .prepare(&spec(SpecKind::Scip, &["indexer"]), root.path())
            .unwrap();
        let root_s = root.path().canonicalize().unwrap();
        let root_s = root_s.to_string_lossy();
        assert_eq!(
            &p.argv[..6],
            ["podman", "run", "--rm", "-i", "--network", "none"]
        );
        assert!(p.argv.contains(&format!("{root_s}:{root_s}:ro")));
        assert_eq!(
            &p.argv[p.argv.len() - 2..],
            ["ghcr.io/acme/indexers:1", "indexer"]
        );
    }

    #[test]
    fn plugins_files_are_validated() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("plugins.yaml");
        std::fs::write(&path, "plugins:\n  - { name: py, kind: lsp, command: [pyright-langserver, --stdio], files: [\"**/*.py\"] }\n").unwrap();
        let err = load_plugins_file(&path).unwrap_err().to_string();
        assert!(err.contains("language_id"), "{err}");
        std::fs::write(&path, "plugins:\n  - { name: py, kind: lsp, command: [pyright-langserver, --stdio], files: [\"**/*.py\"], language_id: python }\nsandbox: { preset: bwrap }\n").unwrap();
        let f = load_plugins_file(&path).unwrap();
        assert!(f.plugins[0].runs_repo_code());
        assert_eq!(f.sandbox.preset, SandboxPreset::Bwrap);
        std::fs::write(
            &path,
            "plugins:\n  - { name: x, kind: facts, command: [] }\n",
        )
        .unwrap();
        assert!(load_plugins_file(&path).is_err());
    }
}
