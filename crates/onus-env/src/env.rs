//! Environments: a container built from the repository at a commit, kept
//! running so commands can be run in it one after another, each leaving
//! evidence in the store.
//!
//! - **Warm images.** `setup` (installing dependencies) runs once per image,
//!   setup command and lockfile contents, with network, and the result is
//!   committed as an image `onus-warm:<key>` that later environments start
//!   from.
//! - **Seed.** `seed` runs in each new environment after setup, so tests get
//!   synthetic data.
//! - **Network.** None, unless the task's token names hosts. Then the
//!   environment sits on an internal network whose only way out is an egress
//!   proxy container that allows exactly those hosts.
//! - **Secrets.** Each secret the token names comes from the operator's
//!   `ONUS_SECRET_<NAME>`; no other secret reaches the container.

use std::collections::BTreeSet;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use onus_doors::runner::{RootTar, checkout, copy_in, engine, tail};
use onus_doors::scope::glob_regex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::spec::Spec;
use crate::store::{
    Artifact, ArtifactKind, EnvironmentInfo, Manifest, SCHEMA, Store, TestCounts, junit_counts,
};

/// What a task's token lets an environment have.
#[derive(Debug, Clone, Default)]
pub struct Grants {
    /// Hosts the environment may reach, from `net:host:…` rights.
    pub hosts: Vec<String>,
    /// Secret names, from `secret:…` rights.
    pub secrets: Vec<String>,
}

/// A created environment, as recorded on its container.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Env {
    pub name: String,
    pub container: String,
    pub commit: String,
    pub image: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warm_image: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<String>,
    pub hosts: Vec<String>,
    pub secrets: Vec<String>,
    pub evidence: Vec<String>,
    pub traces: Vec<String>,
    pub created_at: u64,
}

/// An environment and its container's state (`running`, `exited`, …).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Listed {
    #[serde(flatten)]
    pub env: Env,
    pub state: String,
}

const LABEL: &str = "onus.env";
const INFO: &str = "onus.info";
const PROXY_PORT: u16 = 3128;
/// The tracked files of the commit a warm image was built from: removed
/// before a later commit's files are copied in, so deleted files go.
const WARM_FILES: &str = "/onus-warm-files";
/// Keeps an environment's container running until it is destroyed.
const IDLE: &str = "trap 'exit 0' TERM INT; while :; do sleep 3600 & wait $!; done";

fn container(name: &str) -> String {
    format!("onus-env-{name}")
}

fn egress(name: &str) -> String {
    format!("onus-egress-{name}")
}

fn network(name: &str) -> String {
    format!("onus-env-{name}")
}

/// Runs the engine and returns stdout, or fails with its stderr.
fn engine_cmd(engine: &str, args: &[&str]) -> Result<String> {
    let out = Command::new(engine)
        .args(args)
        .output()
        .with_context(|| format!("cannot run {engine}"))?;
    if !out.status.success() {
        bail!(
            "{engine} {} failed: {}",
            args.first().copied().unwrap_or(""),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// The isolation every Onus container gets (the same as `onus run-test`).
const HARDENING: &[&str] = &[
    "--cap-drop",
    "ALL",
    "--security-opt",
    "no-new-privileges",
    "--memory",
    "4g",
    "--cpus",
    "2",
    "--pids-limit",
    "1024",
];

pub fn check_name(name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && name.len() <= 48
        && name.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "-_.".contains(c));
    if !ok {
        bail!(
            "`{name}` is not an environment name: use lowercase letters, digits, `-`, `_` and `.`"
        );
    }
    Ok(())
}

/// A host the egress proxy can allow: `api.example.com`, or `*.example.com`
/// for its subdomains.
fn check_host(host: &str) -> Result<()> {
    let re = regex::Regex::new(
        r"^(\*\.)?[a-z0-9]([a-z0-9-]*[a-z0-9])?(\.[a-z0-9]([a-z0-9-]*[a-z0-9])?)*$",
    )
    .expect("valid regex");
    if !re.is_match(host) {
        bail!(
            "`net:host:{host}` cannot be enforced: name a host, or `*.domain` for its subdomains"
        );
    }
    Ok(())
}

fn check_secret(name: &str) -> Result<()> {
    let re = regex::Regex::new(r"^[A-Za-z_][A-Za-z0-9_]*$").expect("valid regex");
    if !re.is_match(name) {
        bail!("`secret:{name}` is not a variable name; environments take secrets by exact name");
    }
    Ok(())
}

/// The squid configuration that allows exactly `hosts`.
pub fn proxy_config(hosts: &[String]) -> String {
    let domains: Vec<String> = hosts
        .iter()
        .map(|h| match h.strip_prefix("*.") {
            Some(d) => format!(".{d}"),
            None => h.clone(),
        })
        .collect();
    format!(
        "http_port {PROXY_PORT}\n\
         acl allowed dstdomain {}\n\
         acl safe_ports port 80 443\n\
         http_access deny !safe_ports\n\
         http_access allow allowed\n\
         http_access deny all\n\
         cache deny all\n\
         coredump_dir /var/spool/squid\n",
        domains.join(" ")
    )
}

/// The warm image for `spec` and the lockfiles in `tree`, built if missing.
fn warm_image(engine: &str, spec: &Spec, tree: &Path) -> Result<Option<String>> {
    let Some(setup) = &spec.setup else {
        return Ok(None);
    };
    let mut h = Sha256::new();
    h.update(format!("{}\0{setup}\0", spec.image).as_bytes());
    let mut lockfiles = spec.lockfiles.clone();
    lockfiles.sort();
    for l in &lockfiles {
        if let Ok(bytes) = std::fs::read(tree.join(l)) {
            h.update(format!("{l}\0{}\0", bytes.len()).as_bytes());
            h.update(&bytes);
        }
    }
    let key: String = h.finalize()[..8]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let tag = format!("onus-warm:{key}");
    if engine_cmd(engine, &["image", "inspect", &tag]).is_ok() {
        return Ok(Some(tag));
    }
    let id = engine_cmd(
        engine,
        &[
            &["create", "--label", "onus.warm=1", "-w", "/work"][..],
            HARDENING,
            &["--entrypoint", "sh", &spec.image, "-c", setup],
        ]
        .concat(),
    )?;
    let built = (|| -> Result<()> {
        let mut tar = RootTar::new();
        let files = tar.tree(tree, "work")?;
        tar.file(
            WARM_FILES.trim_start_matches('/'),
            (files.join("\n") + "\n").as_bytes(),
        )?;
        copy_in(engine, &id, tar)?;
        let out = Command::new(engine)
            .args(["start", "-a", &id])
            .output()
            .context("cannot start the setup container")?;
        if !out.status.success() {
            let mut text = String::from_utf8_lossy(&out.stdout).to_string();
            text.push_str(&String::from_utf8_lossy(&out.stderr));
            bail!("the setup step failed:\n{}", tail(&text, 20));
        }
        engine_cmd(engine, &["commit", &id, &tag])?;
        Ok(())
    })();
    let _ = engine_cmd(engine, &["rm", "-f", &id]);
    built.map(|()| Some(tag))
}

/// Creates an environment named `name` from `reference` of `repo`.
pub fn create(
    repo: &Path,
    reference: &str,
    name: &str,
    spec: &Spec,
    grants: &Grants,
) -> Result<Env> {
    check_name(name)?;
    for h in &grants.hosts {
        check_host(h)?;
    }
    let mut secrets = Vec::new();
    for s in &grants.secrets {
        check_secret(s)?;
        let var = format!("ONUS_SECRET_{s}");
        let value = std::env::var(&var)
            .with_context(|| format!("the token grants secret {s}, but ${var} is not set"))?;
        secrets.push((s.clone(), value));
    }
    let engine = engine()?;
    if engine_cmd(&engine, &["container", "inspect", &container(name)]).is_ok() {
        bail!("environment {name} already exists; `onus env destroy {name}` first");
    }
    let (commit, tree) = checkout(repo, reference)?;
    let warm = warm_image(&engine, spec, tree.path())?;
    let env = Env {
        name: name.to_string(),
        container: container(name),
        commit,
        image: spec.image.clone(),
        warm_image: warm.clone(),
        setup: spec.setup.clone(),
        seed: spec.seed.clone(),
        hosts: grants.hosts.clone(),
        secrets: grants.secrets.clone(),
        evidence: spec.evidence.clone(),
        traces: spec.traces.clone(),
        created_at: onus_doors::gateway::now(),
    };
    let made = start(&engine, &env, spec, &secrets, tree.path());
    if let Err(e) = made {
        let _ = destroy(name);
        return Err(e);
    }
    Ok(env)
}

fn start(
    engine: &str,
    env: &Env,
    spec: &Spec,
    secrets: &[(String, String)],
    tree: &Path,
) -> Result<()> {
    let name = &env.name;
    let mut args: Vec<String> = vec![
        "create".into(),
        "--name".into(),
        env.container.clone(),
        "--label".into(),
        format!("{LABEL}={name}"),
        "--label".into(),
        format!("{INFO}={}", serde_json::to_string(env)?),
        "-w".into(),
        "/work".into(),
    ];
    args.extend(HARDENING.iter().map(|s| s.to_string()));
    if env.hosts.is_empty() {
        args.extend(["--network".into(), "none".into()]);
    } else {
        start_egress(engine, name, &env.hosts, &spec.egress_image)?;
        args.extend(["--network".into(), network(name)]);
        let proxy = format!("http://{}:{PROXY_PORT}", egress(name));
        for var in ["HTTP_PROXY", "HTTPS_PROXY", "http_proxy", "https_proxy"] {
            args.extend(["-e".into(), format!("{var}={proxy}")]);
        }
        args.extend(["-e".into(), "NO_PROXY=localhost,127.0.0.1".into()]);
    }
    // Secret values travel in the engine's environment, never in its
    // arguments, so they do not show in process listings.
    for (s, _) in secrets {
        args.extend(["-e".into(), s.clone()]);
    }
    args.extend(["--entrypoint".into(), "sh".into()]);
    args.push(env.warm_image.clone().unwrap_or_else(|| env.image.clone()));
    args.extend(["-c".into(), IDLE.into()]);
    let out = Command::new(engine)
        .args(&args)
        .envs(secrets.iter().map(|(k, v)| (k, v)))
        .output()
        .context("cannot create the environment")?;
    if !out.status.success() {
        bail!(
            "cannot create the environment: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let c = &env.container;
    engine_cmd(engine, &["start", c])?;
    if env.warm_image.is_some() {
        engine_cmd(
            engine,
            &[
                "exec",
                c,
                "sh",
                "-c",
                &format!(
                    "cd /work && while IFS= read -r f; do rm -f -- \"$f\"; done < {WARM_FILES}"
                ),
            ],
        )?;
    }
    let mut tar = RootTar::new();
    tar.tree(tree, "work")?;
    copy_in(engine, c, tar)?;
    if let Some(seed) = &env.seed {
        let out = Command::new(engine)
            .args([
                "exec",
                "-w",
                "/work",
                c,
                "sh",
                "-c",
                &format!("exec 2>&1\n{seed}"),
            ])
            .output()
            .context("cannot run the seed command")?;
        if !out.status.success() {
            bail!(
                "the seed command failed:\n{}",
                tail(&String::from_utf8_lossy(&out.stdout), 20)
            );
        }
    }
    Ok(())
}

/// Starts the egress proxy: on the default network, and on the
/// environment's internal network, which has no other way out.
fn start_egress(engine: &str, name: &str, hosts: &[String], image: &str) -> Result<()> {
    let label = format!("{LABEL}={name}");
    engine_cmd(
        engine,
        &[
            "network",
            "create",
            "--internal",
            "--label",
            &label,
            &network(name),
        ],
    )?;
    let proxy = egress(name);
    engine_cmd(
        engine,
        &["create", "--name", &proxy, "--label", &label, image],
    )?;
    let mut tar = RootTar::new();
    tar.file("etc/squid/squid.conf", proxy_config(hosts).as_bytes())?;
    copy_in(engine, &proxy, tar)?;
    engine_cmd(engine, &["network", "connect", &network(name), &proxy])?;
    engine_cmd(engine, &["start", &proxy])?;
    // Ready when something listens on the proxy port.
    let probe =
        format!("cat /proc/net/tcp /proc/net/tcp6 2>/dev/null | grep -qi ':{PROXY_PORT:04X} '");
    for _ in 0..60 {
        if engine_cmd(engine, &["exec", &proxy, "sh", "-c", &probe]).is_ok() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    bail!("the egress proxy ({image}) did not start listening on port {PROXY_PORT}")
}

/// The environment named `name`.
pub fn get(name: &str) -> Result<Env> {
    check_name(name)?;
    let engine = engine()?;
    let info = engine_cmd(
        &engine,
        &[
            "container",
            "inspect",
            "--format",
            &format!("{{{{index .Config.Labels \"{INFO}\"}}}}"),
            &container(name),
        ],
    )
    .with_context(|| format!("there is no environment {name}"))?;
    serde_json::from_str(&info)
        .with_context(|| format!("{} is not an Onus environment", container(name)))
}

/// Runs `command` in the environment and records the run in `store`.
pub fn run(name: &str, command: &str, store: &Store) -> Result<(String, Manifest)> {
    let env = get(name)?;
    let engine = engine()?;
    let c = &env.container;
    // Some `find`s compare whole seconds, so the marker is a second old and
    // files already that new before the run are left out.
    let marker = "/tmp/.onus-run-start";
    let started_at = onus_doors::gateway::now();
    engine_cmd(
        &engine,
        &[
            "exec",
            c,
            "sh",
            "-c",
            &format!(
                "touch -d @{} {marker} 2>/dev/null || touch {marker}",
                started_at.saturating_sub(1)
            ),
        ],
    )?;
    let newer = || -> BTreeSet<String> {
        engine_cmd(
            &engine,
            &[
                "exec",
                c,
                "find",
                "/work",
                "(",
                "-name",
                "node_modules",
                "-o",
                "-name",
                ".git",
                ")",
                "-prune",
                "-o",
                "-type",
                "f",
                "-newer",
                marker,
                "-print",
            ],
        )
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.strip_prefix("/work/"))
        .map(str::to_string)
        .collect()
    };
    let before = newer();
    let out = Command::new(&engine)
        .args([
            "exec",
            "-w",
            "/work",
            c,
            "sh",
            "-c",
            &format!("exec 2>&1\n{command}"),
        ])
        .stdin(Stdio::null())
        .output()
        .context("cannot run the command in the environment")?;
    let finished_at = onus_doors::gateway::now();
    let mut log = out.stdout;
    log.extend_from_slice(&out.stderr);
    let secrets = secret_values(&engine, &env)?;
    let log = redact(log, &secrets);

    let mut artifacts = vec![Artifact {
        kind: ArtifactKind::Log,
        path: "-".into(),
        sha256: store.put(&log)?,
        size: log.len() as u64,
    }];
    // Files the command wrote that the environment declares as evidence.
    let written: Vec<String> = newer().difference(&before).cloned().collect();
    let globs = |gs: &[String]| -> Result<Vec<regex::Regex>> {
        gs.iter()
            .map(|g| Ok(regex::Regex::new(&glob_regex(g))?))
            .collect()
    };
    let (junit, traces) = (globs(&env.evidence)?, globs(&env.traces)?);
    let mut tests: Option<TestCounts> = None;
    for path in &written {
        let kind = if junit.iter().any(|r| r.is_match(path)) {
            ArtifactKind::Junit
        } else if traces.iter().any(|r| r.is_match(path)) {
            ArtifactKind::Trace
        } else {
            continue;
        };
        let bytes = Command::new(&engine)
            .args(["exec", c, "cat", "--", &format!("/work/{path}")])
            .output()?
            .stdout;
        let bytes = redact(bytes, &secrets);
        if kind == ArtifactKind::Junit {
            let n = junit_counts(&String::from_utf8_lossy(&bytes));
            let t = tests.get_or_insert_default();
            t.tests += n.tests;
            t.failures += n.failures;
            t.errors += n.errors;
            t.skipped += n.skipped;
        }
        artifacts.push(Artifact {
            kind,
            path: path.to_string(),
            sha256: store.put(&bytes)?,
            size: bytes.len() as u64,
        });
    }
    artifacts.sort_by(|a, b| (a.kind, &a.path).cmp(&(b.kind, &b.path)));
    let manifest = Manifest {
        schema: SCHEMA,
        commit: env.commit.clone(),
        command: command.to_string(),
        exit_code: out.status.code().unwrap_or(-1),
        started_at,
        finished_at,
        environment: EnvironmentInfo {
            name: env.name.clone(),
            image: env.image.clone(),
            warm_image: env.warm_image.clone(),
            setup: env.setup.clone(),
            seed: env.seed.clone(),
            hosts: env.hosts.clone(),
            secrets: env.secrets.clone(),
        },
        artifacts,
        tests,
    };
    let id = store.record(&manifest)?;
    Ok((id, manifest))
}

/// The values of the environment's secrets, to keep them out of evidence.
fn secret_values(engine: &str, env: &Env) -> Result<Vec<(String, Vec<u8>)>> {
    if env.secrets.is_empty() {
        return Ok(Vec::new());
    }
    let vars: Vec<String> = serde_json::from_str(&engine_cmd(
        engine,
        &[
            "container",
            "inspect",
            "--format",
            "{{json .Config.Env}}",
            &env.container,
        ],
    )?)?;
    Ok(vars
        .iter()
        .filter_map(|v| v.split_once('='))
        .filter(|(k, v)| env.secrets.iter().any(|s| s == k) && !v.is_empty())
        .map(|(k, v)| (k.to_string(), v.as_bytes().to_vec()))
        .collect())
}

/// Replaces each secret value in `bytes` with `[secret NAME]`.
fn redact(mut bytes: Vec<u8>, secrets: &[(String, Vec<u8>)]) -> Vec<u8> {
    for (name, value) in secrets {
        let mark = format!("[secret {name}]").into_bytes();
        let mut out = Vec::with_capacity(bytes.len());
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i..].starts_with(value) {
                out.extend_from_slice(&mark);
                i += value.len();
            } else {
                out.push(bytes[i]);
                i += 1;
            }
        }
        bytes = out;
    }
    bytes
}

/// Every environment, by name.
pub fn list() -> Result<Vec<Listed>> {
    let engine = engine()?;
    let out = engine_cmd(
        &engine,
        &[
            "ps",
            "-a",
            "--filter",
            &format!("label={INFO}"),
            "--format",
            &format!("{{{{.State}}}}\t{{{{.Label \"{INFO}\"}}}}"),
        ],
    )?;
    let mut all: Vec<Listed> = out
        .lines()
        .filter_map(|l| {
            let (state, info) = l.split_once('\t')?;
            Some(Listed {
                env: serde_json::from_str(info).ok()?,
                state: state.to_string(),
            })
        })
        .collect();
    all.sort_by(|a, b| a.env.name.cmp(&b.env.name));
    Ok(all)
}

/// Removes an environment, its egress proxy and its network. Returns
/// whether there was one.
pub fn destroy(name: &str) -> Result<bool> {
    check_name(name)?;
    let engine = engine()?;
    let existed = engine_cmd(&engine, &["container", "inspect", &container(name)]).is_ok();
    let _ = engine_cmd(&engine, &["rm", "-f", &container(name)]);
    let _ = engine_cmd(&engine, &["rm", "-f", &egress(name)]);
    let _ = engine_cmd(&engine, &["network", "rm", &network(name)]);
    Ok(existed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_hosts_and_secrets_are_checked() {
        assert!(check_name("task-42.a").is_ok());
        assert!(check_name("Task").is_err());
        assert!(check_name("-x").is_err());
        assert!(check_host("api.example.com").is_ok());
        assert!(check_host("*.example.com").is_ok());
        assert!(check_host("api.*.com").is_err());
        assert!(check_host("example.com:443").is_err());
        assert!(check_secret("SMS_TEST_KEY").is_ok());
        assert!(check_secret("SMS_*").is_err());
    }

    #[test]
    fn secret_values_never_reach_the_store() {
        let secrets = vec![("API_KEY".to_string(), b"s3cr3t".to_vec())];
        assert_eq!(
            redact(b"key=s3cr3t; again s3cr3t".to_vec(), &secrets),
            b"key=[secret API_KEY]; again [secret API_KEY]"
        );
    }

    #[test]
    fn the_proxy_allows_exactly_the_granted_hosts() {
        let conf = proxy_config(&["api.example.com".into(), "*.npmjs.org".into()]);
        assert!(conf.contains("acl allowed dstdomain api.example.com .npmjs.org\n"));
        assert!(conf.contains("http_access deny all"));
    }
}
