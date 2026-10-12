//! Onus Cloud from the command line and from agents.
//!
//! `onus login` signs the CLI in as a person: it asks Onus Cloud for a code, the person
//! approves it in the browser (the device authorization grant, RFC 8628), and the CLI
//! keeps the token it is given in `~/.config/onus/cloud.json`. In CI, an organisation's
//! API key in `ONUS_API_KEY` stands in for a login. A scoped token in `ONUS_CLOUD_TOKEN`
//! (`onus cloud token create`) stands in for both, and opens only the doors it names.
//! Either way [`Client`] calls the API: hosted environments Onus Cloud builds and
//! records, pull requests and their lanes, evidence attached to them, escalations and
//! the maps Onus Cloud keeps.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// The hosted API, unless `--url` or `ONUS_CLOUD_URL` names another.
pub const DEFAULT_URL: &str = "https://api.onushq.com";

/// What a scoped Onus Cloud token can open (C2's `TOKEN_SCOPES`, same order).
/// `env:run` implies `env:read` on the server.
pub const CLOUD_SCOPES: &[&str] = &[
    "env:read",
    "env:run",
    "evidence:write",
    "pulls:read",
    "map:read",
    "escalation:write",
];

/// The scopes `onus mcp` mints for an agent from a login: all of them, for one
/// organisation and one repository, for hours.
pub const AGENT_SCOPES: &[&str] = CLOUD_SCOPES;

/// How long a scoped token may last: 60 seconds to 24 hours.
pub const TOKEN_TTL: std::ops::RangeInclusive<u64> = 60..=86_400;

/// What `onus login` keeps: where, as whom, and the organisation commands act in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Credentials {
    pub url: String,
    pub token: String,
    /// The organisation (slug) commands act in, unless `--org` or `ONUS_CLOUD_ORG` says otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub organisation: Option<String>,
    /// Who approved the login: id, email, name, githubLogin.
    #[serde(default)]
    pub user: Value,
}

/// `$ONUS_CONFIG_DIR`, else `$XDG_CONFIG_HOME/onus`, else `~/.config/onus`
/// (`%APPDATA%\onus` on Windows).
pub fn config_dir() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os("ONUS_CONFIG_DIR").filter(|d| !d.is_empty()) {
        return Ok(PathBuf::from(dir));
    }
    if let Some(dir) = std::env::var_os("XDG_CONFIG_HOME").filter(|d| !d.is_empty()) {
        return Ok(PathBuf::from(dir).join("onus"));
    }
    if cfg!(windows)
        && let Some(dir) = std::env::var_os("APPDATA")
    {
        return Ok(PathBuf::from(dir).join("onus"));
    }
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .context("cannot tell where your home folder is: set ONUS_CONFIG_DIR")?;
    Ok(PathBuf::from(home).join(".config").join("onus"))
}

pub fn credentials_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("cloud.json"))
}

pub fn load_credentials() -> Result<Option<Credentials>> {
    load_credentials_at(&credentials_path()?)
}

pub fn load_credentials_at(path: &Path) -> Result<Option<Credentials>> {
    match std::fs::read_to_string(path) {
        Ok(text) => {
            Ok(Some(serde_json::from_str(&text).with_context(|| {
                format!("{} is not valid", path.display())
            })?))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("cannot read {}", path.display())),
    }
}

/// Writes the credentials readable by their owner only.
pub fn save_credentials(c: &Credentials) -> Result<PathBuf> {
    let path = credentials_path()?;
    save_credentials_at(&path, c)?;
    Ok(path)
}

pub fn save_credentials_at(path: &Path, c: &Credentials) -> Result<()> {
    let dir = path.parent().context("no config folder")?;
    std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    let tmp = tempfile::NamedTempFile::new_in(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(tmp.path(), std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::write(tmp.path(), serde_json::to_vec_pretty(c)?)?;
    tmp.persist(path)
        .with_context(|| format!("cannot write {}", path.display()))?;
    Ok(())
}

/// Removes the saved login; whether there was one.
pub fn forget_credentials() -> Result<bool> {
    forget_credentials_at(&credentials_path()?)
}

pub fn forget_credentials_at(path: &Path) -> Result<bool> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e).with_context(|| format!("cannot remove {}", path.display())),
    }
}

/// An error the API answered with: its status and what it said.
#[derive(Debug)]
pub struct ApiError {
    pub status: u16,
    /// The body's `error`, such as `insufficient_scope` or `no_map`.
    pub code: Option<String>,
    /// For `insufficient_scope`: the scope the route needs.
    pub scope: Option<String>,
    pub message: String,
    /// How the caller signed in ([`Client::via`]).
    pub via: &'static str,
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.status {
            401 if self.via == "token" => write!(
                f,
                "{} (the token ended or was revoked: ask for a new one)",
                self.message
            ),
            401 => write!(
                f,
                "{} (run `onus login`, or set ONUS_API_KEY in CI)",
                self.message
            ),
            402 => write!(
                f,
                "{} (see your plan in the Onus Cloud billing settings)",
                self.message
            ),
            403 if self.code.as_deref() == Some("insufficient_scope") => write!(
                f,
                "{} (mint one with `onus cloud token create --scope {}`, or ask with onus_escalate)",
                self.message,
                self.scope.as_deref().unwrap_or("<scope>")
            ),
            // Denials teach as they are.
            403 => write!(f, "{}", self.message),
            _ => write!(f, "{} ({})", self.message, self.status),
        }
    }
}

/// Whether `e` is an answer of the API with this status.
pub fn is_status(e: &anyhow::Error, status: u16) -> bool {
    e.downcast_ref::<ApiError>()
        .is_some_and(|a| a.status == status)
}

impl std::error::Error for ApiError {}

/// Percent-encodes one path segment (`owner/name` becomes `owner%2Fname`).
pub fn segment(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// A caller of the Onus Cloud API.
#[derive(Clone)]
pub struct Client {
    pub url: String,
    token: Option<String>,
    /// The organisation (slug or id) commands act in, when known.
    pub org: Option<String>,
    /// How the client signed in: `login` (a person, from `onus login`), `api-key`, or
    /// `token` (a scoped token, from `ONUS_CLOUD_TOKEN` or minted by `onus mcp`).
    pub via: &'static str,
    agent: ureq::Agent,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("url", &self.url)
            .field("org", &self.org)
            .field("via", &self.via)
            .finish_non_exhaustive()
    }
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(90)))
        .http_status_as_error(false)
        .user_agent(format!("onus/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

fn env_var(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

/// Seconds since the Unix epoch.
fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

impl Client {
    /// A client that is not signed in, for `onus login`.
    pub fn anonymous(url: &str) -> Client {
        Client {
            url: url.trim_end_matches('/').to_string(),
            token: None,
            org: None,
            via: "none",
            agent: agent(),
        }
    }

    /// How this process is signed in: `ONUS_CLOUD_TOKEN` (a scoped token), else
    /// `ONUS_API_KEY` (CI), else `onus login`'s credentials; none when there is none.
    /// `ONUS_CLOUD_URL` and `ONUS_CLOUD_ORG` override where and in which organisation.
    pub fn discover() -> Result<Option<Client>> {
        Client::discover_from(&|k| std::env::var(k).ok(), &load_credentials)
    }

    /// [`Client::discover`] over a given environment and credentials loader. With a
    /// token, the loader is never called: the token and nothing else.
    pub fn discover_from(
        env: &dyn Fn(&str) -> Option<String>,
        load: &dyn Fn() -> Result<Option<Credentials>>,
    ) -> Result<Option<Client>> {
        let get = |k: &str| {
            env(k)
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        };
        let url = get("ONUS_CLOUD_URL");
        let org = get("ONUS_CLOUD_ORG");
        let with = |token: String, via: &'static str| Client {
            url: url
                .clone()
                .unwrap_or_else(|| DEFAULT_URL.into())
                .trim_end_matches('/')
                .into(),
            token: Some(token),
            org: org.clone(),
            via,
            agent: agent(),
        };
        if let Some(t) = get("ONUS_CLOUD_TOKEN") {
            return Ok(Some(with(t, "token")));
        }
        if let Some(key) = get("ONUS_API_KEY") {
            return Ok(Some(with(key, "api-key")));
        }
        let Some(c) = load()? else {
            return Ok(None);
        };
        Ok(Some(Client {
            url: url.unwrap_or(c.url).trim_end_matches('/').into(),
            token: Some(c.token),
            org: org.or(c.organisation),
            via: "login",
            agent: agent(),
        }))
    }

    /// The same API and organisation, signed in with another token.
    pub fn with_token(&self, token: String, via: &'static str) -> Client {
        Client {
            token: Some(token),
            via,
            ..self.clone()
        }
    }

    /// [`Client::discover`], or the reason there is none.
    pub fn require() -> Result<Client> {
        Client::discover()?.ok_or_else(|| {
            anyhow!("not signed in to Onus Cloud: run `onus login` (in CI, set ONUS_API_KEY)")
        })
    }

    /// The error for an answer that is not a success.
    fn api_error(&self, status: u16, text: &str) -> anyhow::Error {
        let body: Value = serde_json::from_str(text).unwrap_or(Value::Null);
        let message = body["message"]
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| {
                if text.trim().is_empty() {
                    format!("{} answered {status}", self.url)
                } else {
                    text.chars().take(300).collect()
                }
            });
        ApiError {
            status,
            code: body["error"].as_str().map(str::to_string),
            scope: body["scope"].as_str().map(str::to_string),
            message,
            via: self.via,
        }
        .into()
    }

    fn answer(
        &self,
        response: Result<ureq::http::Response<ureq::Body>, ureq::Error>,
    ) -> Result<Value> {
        let mut response = response.map_err(|e| anyhow!("cannot reach {}: {e}", self.url))?;
        let status = response.status().as_u16();
        let text = response
            .body_mut()
            .with_config()
            .limit(64 * 1024 * 1024)
            .read_to_string()
            .unwrap_or_default();
        if (200..300).contains(&status) {
            if text.trim().is_empty() {
                return Ok(Value::Null);
            }
            // JSON, or text (a run's log).
            return Ok(serde_json::from_str(&text).unwrap_or(Value::String(text)));
        }
        Err(self.api_error(status, &text))
    }

    fn authorised<B>(&self, req: ureq::RequestBuilder<B>) -> ureq::RequestBuilder<B> {
        match &self.token {
            Some(t) => req.header("authorization", format!("Bearer {t}")),
            None => req,
        }
    }

    pub fn get(&self, path: &str) -> Result<Value> {
        let req = self.authorised(self.agent.get(format!("{}{path}", self.url)));
        self.answer(req.call())
    }

    pub fn post(&self, path: &str, body: &Value) -> Result<Value> {
        let req = self.authorised(self.agent.post(format!("{}{path}", self.url)));
        self.answer(req.send_json(body))
    }

    pub fn delete(&self, path: &str) -> Result<Value> {
        let req = self.authorised(self.agent.delete(format!("{}{path}", self.url)));
        self.answer(req.call())
    }

    /// A GET answered with bytes (a map, up to 256 MiB): the status, the body and the
    /// `x-onus-map-commit` header.
    pub fn get_raw(&self, path: &str) -> Result<(u16, Vec<u8>, Option<String>)> {
        let req = self.authorised(self.agent.get(format!("{}{path}", self.url)));
        let mut response = req
            .call()
            .map_err(|e| anyhow!("cannot reach {}: {e}", self.url))?;
        let status = response.status().as_u16();
        let commit = response
            .headers()
            .get("x-onus-map-commit")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let bytes = response
            .body_mut()
            .with_config()
            .limit(256 * 1024 * 1024)
            .read_to_vec()
            .with_context(|| format!("cannot read the answer of {}", self.url))?;
        Ok((status, bytes, commit))
    }

    /// Who is calling, and their organisations.
    pub fn whoami(&self) -> Result<Value> {
        self.get("/v1/whoami")
    }

    /// The organisation to act in: the one chosen, else the caller's only one.
    pub fn organisation(&self) -> Result<String> {
        if let Some(o) = &self.org {
            return Ok(o.clone());
        }
        let me = self.whoami()?;
        // A scoped token acts in its one organisation.
        if let Some(slug) = me["token"]["organisation"]["slug"].as_str() {
            return Ok(slug.to_string());
        }
        let orgs = me["organisations"].as_array().cloned().unwrap_or_default();
        match orgs.as_slice() {
            [one] => Ok(one["slug"].as_str().unwrap_or_default().to_string()),
            [] => {
                bail!("you belong to no organisation in Onus Cloud yet: create one in the web app")
            }
            many => bail!(
                "you belong to several organisations ({}): pick one with `onus cloud use <slug>`, --org or ONUS_CLOUD_ORG",
                many.iter()
                    .filter_map(|o| o["slug"].as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }

    /// `/v1/orgs/<org><rest>`, in the organisation to act in.
    pub fn org_path(&self, rest: &str) -> Result<String> {
        Ok(format!("/v1/orgs/{}{rest}", segment(&self.organisation()?)))
    }

    /// Creates an environment and, with `wait`, waits until it is ready (or failed).
    pub fn create_environment(
        &self,
        input: &Value,
        wait: Option<Duration>,
        progress: &mut dyn FnMut(&Value),
    ) -> Result<Value> {
        let created = self.post(&self.org_path("/environments")?, input)?;
        let mut env = created["environment"].clone();
        let Some(limit) = wait else {
            return Ok(env);
        };
        let id = env["id"].as_str().unwrap_or_default().to_string();
        let started = Instant::now();
        let mut last = String::new();
        loop {
            let status = env["status"].as_str().unwrap_or_default().to_string();
            if status != last {
                progress(&env);
                last = status.clone();
            }
            if !matches!(status.as_str(), "queued" | "creating") {
                return Ok(env);
            }
            if started.elapsed() > limit {
                bail!(
                    "environment {} is still {status} after {}s; check with `onus cloud env show {}`",
                    env["name"].as_str().unwrap_or(&id),
                    limit.as_secs(),
                    env["name"].as_str().unwrap_or(&id)
                );
            }
            std::thread::sleep(Duration::from_secs(2));
            env = self.get(&self.org_path(&format!("/environments/{}", segment(&id)))?)?["environment"].clone();
        }
    }

    /// Runs a command in an environment and, with `wait`, waits for it to finish.
    pub fn run_in_environment(
        &self,
        env: &str,
        command: &str,
        timeout_seconds: Option<u64>,
        wait: bool,
    ) -> Result<Value> {
        let mut body = json!({ "command": command });
        if let Some(t) = timeout_seconds {
            body["timeoutSeconds"] = json!(t);
        }
        let base = self.org_path(&format!("/environments/{}/runs", segment(env)))?;
        let queued = self.post(&base, &body)?;
        let mut run = queued["run"].clone();
        if !wait {
            return Ok(run);
        }
        let id = run["id"].as_str().unwrap_or_default().to_string();
        loop {
            if !matches!(run["state"].as_str(), Some("queued" | "running")) {
                return Ok(run);
            }
            run = self.get(&format!("{base}/{}?wait=50", segment(&id)))?["run"].clone();
        }
    }
}

/// `owner/name` from a GitHub remote URL (https, ssh or scp-like).
pub fn github_repository(remote: &str) -> Option<String> {
    let r = remote.trim().trim_end_matches('/');
    let rest = r
        .strip_prefix("git@github.com:")
        .or_else(|| r.strip_prefix("ssh://git@github.com/"))
        .or_else(|| r.strip_prefix("https://github.com/"))
        .or_else(|| r.strip_prefix("http://github.com/"))
        .or_else(|| {
            // https://user:token@github.com/owner/name
            let at = r.find("@github.com/")?;
            r.starts_with("https://")
                .then(|| &r[at + "@github.com/".len()..])
        })?;
    let rest = rest.strip_suffix(".git").unwrap_or(rest);
    let mut parts = rest.split('/');
    let (owner, name) = (parts.next()?, parts.next()?);
    (parts.next().is_none() && !owner.is_empty() && !name.is_empty())
        .then(|| format!("{owner}/{name}"))
}

fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// The GitHub repository of a worktree: its `origin`, else its first GitHub remote.
pub fn repository_of(dir: &Path) -> Option<String> {
    if let Some(r) = git(dir, &["remote", "get-url", "origin"]).and_then(|u| github_repository(&u))
    {
        return Some(r);
    }
    git(dir, &["remote"])?
        .lines()
        .filter_map(|name| git(dir, &["remote", "get-url", name]))
        .find_map(|u| github_repository(&u))
}

/// The commit `reference` names in a worktree.
pub fn commit_of(dir: &Path, reference: &str) -> Option<String> {
    git(dir, &["rev-parse", &format!("{reference}^{{commit}}")])
}

/// Whether a remote-tracking branch has the commit (Onus Cloud can only fetch what is pushed).
pub fn is_pushed(dir: &Path, commit: &str) -> bool {
    git(dir, &["branch", "-r", "--contains", commit]).is_some_and(|b| !b.is_empty())
}

/// This worktree's HEAD, which must be pushed: Onus Cloud reads it from GitHub.
pub fn pushed_head(root: &Path) -> Result<String> {
    let head = commit_of(root, "HEAD").context("this worktree has no commit")?;
    if !is_pushed(root, &head) {
        bail!(
            "HEAD ({}) is not pushed, and Onus Cloud reads it from GitHub: push it first, or pass ref",
            &head[..12.min(head.len())]
        );
    }
    Ok(head)
}

/// Whether `s` is a full commit sha: 40 lowercase hex characters.
pub fn is_sha(s: &str) -> bool {
    s.len() == 40
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

// ---------------------------------------------------------------------------
// Scoped tokens (C2)

/// What a scoped token is for: some repositories of one organisation, some scopes,
/// for a while. The body of `POST /v1/orgs/:org/tokens`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Narrow {
    /// `owner/name`; `None` is every repository (from a login only).
    pub repositories: Option<Vec<String>>,
    pub scopes: Vec<String>,
    pub ttl_seconds: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// Mints a scoped token in the client's organisation. A login or a scoped token can;
/// an API key cannot.
pub fn mint_token(client: &Client, n: &Narrow) -> Result<Value> {
    if client.via == "api-key" {
        bail!("API keys cannot mint tokens; sign in with onus login");
    }
    if let Some(s) = n
        .scopes
        .iter()
        .find(|s| !CLOUD_SCOPES.contains(&s.as_str()))
    {
        bail!("`{s}` is not a scope: {}", CLOUD_SCOPES.join(", "));
    }
    client.post(&client.org_path("/tokens")?, &serde_json::to_value(n)?)
}

/// A token's lifetime: `90`, `90s`, `30m`, `2h` or `1d`, from 60 seconds to 24 hours.
pub fn parse_ttl(s: &str) -> Result<u64> {
    let t = s.trim();
    let split = t.find(|c: char| !c.is_ascii_digit()).unwrap_or(t.len());
    let (number, unit) = t.split_at(split);
    let n: u64 = number
        .parse()
        .with_context(|| format!("`{s}` is not a duration: write 90s, 30m, 2h or 1d"))?;
    let unit = match unit {
        "" | "s" => 1,
        "m" => 60,
        "h" => 3600,
        "d" => 86_400,
        _ => bail!("`{s}` is not a duration: write 90s, 30m, 2h or 1d"),
    };
    let seconds = n.saturating_mul(unit);
    if !TOKEN_TTL.contains(&seconds) {
        bail!("a token lasts 60 seconds to 24 hours, not {s}");
    }
    Ok(seconds)
}

/// Seconds since the Unix epoch of an ISO 8601 UTC time (`2026-10-12T20:14:03.000Z`).
pub fn parse_utc(s: &str) -> Option<u64> {
    let s = s.trim().strip_suffix('Z')?;
    let (date, time) = s.split_once('T')?;
    let mut d = date.split('-').map(|p| p.parse::<i64>().ok());
    let (y, m, day) = (d.next()??, d.next()??, d.next()??);
    let time = time.split('.').next()?;
    let mut t = time.split(':').map(|p| p.parse::<i64>().ok());
    let (hh, mm, ss) = (t.next()??, t.next()??, t.next()??);
    // Days from civil (Howard Hinnant's algorithm).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    u64::try_from(days * 86_400 + hh * 3600 + mm * 60 + ss).ok()
}

/// What `onus mcp` uses for every Onus Cloud call. Signed in with a login, it never
/// sends the login on: it mints a scoped token from it on first use, for one
/// repository and the agent's scopes, keeps it in memory, and mints again 60 seconds
/// before it ends. With a scoped token or an API key, it uses that as it is.
pub struct CloudDoor {
    base: Client,
    narrow: Option<Narrow>,
    repository: Option<String>,
    current: Mutex<Option<(Client, u64)>>,
}

impl std::fmt::Debug for CloudDoor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CloudDoor")
            .field("base", &self.base)
            .field("narrow", &self.narrow)
            .finish_non_exhaustive()
    }
}

impl CloudDoor {
    /// A door that narrows a login to `narrow`. Its repositories name the one the
    /// agent works in; an empty list means it is not known, which the first call
    /// reports.
    pub fn new(base: Client, narrow: Narrow) -> CloudDoor {
        let repository = narrow
            .repositories
            .as_ref()
            .and_then(|r| r.first().cloned());
        CloudDoor {
            base,
            narrow: Some(narrow),
            repository,
            current: Mutex::new(None),
        }
    }

    /// A door that uses the client as it is (the CLI, for its own person).
    pub fn plain(base: Client, repository: Option<String>) -> CloudDoor {
        CloudDoor {
            base,
            narrow: None,
            repository,
            current: Mutex::new(None),
        }
    }

    /// How the process signed in: `login`, `api-key` or `token`.
    pub fn via(&self) -> &'static str {
        self.base.via
    }

    /// The repository (`owner/name`) the agent works in, when known.
    pub fn repository(&self) -> Option<String> {
        self.repository.clone()
    }

    /// The client for the next call.
    pub fn client(&self) -> Result<Client> {
        let Some(n) = self.narrow.as_ref().filter(|_| self.base.via == "login") else {
            return Ok(self.base.clone());
        };
        if n.repositories.as_ref().is_some_and(Vec::is_empty) {
            bail!(
                "this worktree has no GitHub remote, so onus mcp cannot tell which repository its Onus Cloud token is for: start it with --cloud-repo owner/name (or ONUS_CLOUD_REPO)"
            );
        }
        let mut current = self
            .current
            .lock()
            .map_err(|_| anyhow!("the cloud door is poisoned"))?;
        let now = unix_now();
        if let Some((c, ends)) = current.as_ref()
            && *ends > now + 60
        {
            return Ok(c.clone());
        }
        let minted = mint_token(&self.base, n).context("cannot mint a scoped Onus Cloud token")?;
        let token = minted["token"]
            .as_str()
            .context("Onus Cloud answered without a token")?;
        let latest = now + n.ttl_seconds;
        let ends = minted["expiresAt"]
            .as_str()
            .and_then(parse_utc)
            .map_or(latest, |e| e.min(latest));
        let mut c = self.base.with_token(token.to_string(), "token");
        if let Some(slug) = minted["organisation"]["slug"].as_str() {
            c.org = Some(slug.to_string());
        }
        *current = Some((c.clone(), ends));
        Ok(c)
    }
}

// ---------------------------------------------------------------------------
// Escalations (C5)

/// Files an escalation request in Onus Cloud: `POST /v1/orgs/:org/escalations`.
pub fn file_escalation(c: &Client, body: &Value) -> Result<Value> {
    c.post(&c.org_path("/escalations")?, body)
}

/// Where an escalation stands, waiting up to `wait` seconds (at most 50) while it is
/// being checked or waiting for a person.
pub fn escalation(c: &Client, id: &str, wait: u64) -> Result<Value> {
    c.get(&c.org_path(&format!(
        "/escalations/{}?wait={}",
        segment(id),
        wait.min(50)
    ))?)
}

/// The organisation's escalations: those waiting for a person, or all.
pub fn escalations(c: &Client, waiting: bool) -> Result<Value> {
    c.get(&c.org_path(if waiting {
        "/escalations?status=waiting"
    } else {
        "/escalations?status=all"
    })?)
}

/// Withdraws an escalation the caller filed.
pub fn withdraw_escalation(c: &Client, id: &str) -> Result<Value> {
    c.post(
        &c.org_path(&format!("/escalations/{}/withdraw", segment(id)))?,
        &json!({}),
    )
}

// ---------------------------------------------------------------------------
// The maps Onus Cloud keeps (C6)

/// `$ONUS_CACHE_DIR`, else `$XDG_CACHE_HOME/onus`, else `~/.cache/onus`.
pub fn cache_dir() -> Result<PathBuf> {
    if let Some(dir) = env_var("ONUS_CACHE_DIR") {
        return Ok(PathBuf::from(dir));
    }
    if let Some(dir) = env_var("XDG_CACHE_HOME") {
        return Ok(PathBuf::from(dir).join("onus"));
    }
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .context("cannot tell where your home folder is: set ONUS_CACHE_DIR")?;
    Ok(PathBuf::from(home).join(".cache").join("onus"))
}

/// Where the map of `sha` is kept: `<cache>/cloud-maps/<api host>/<org>/<owner>__<name>/<sha>.json`.
pub fn map_cache_path(cache: &Path, url: &str, org: &str, repo: &str, sha: &str) -> PathBuf {
    let host = url
        .split_once("://")
        .map_or(url, |(_, rest)| rest)
        .split('/')
        .next()
        .unwrap_or_default();
    let clean = |s: &str| -> String {
        s.chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || "-._".contains(c) {
                    c
                } else {
                    '_'
                }
            })
            .collect()
    };
    cache
        .join("cloud-maps")
        .join(clean(host))
        .join(clean(org))
        .join(clean(&repo.replace('/', "__")))
        .join(format!("{sha}.json"))
}

/// The commit a hosted map is kept under: the one asked for, else the one Onus Cloud
/// named; none (and nothing is kept) when neither is a full sha.
pub fn cached_sha(asked: Option<&str>, named: Option<&str>) -> Option<String> {
    asked.or(named).filter(|s| is_sha(s)).map(str::to_string)
}

/// A map Onus Cloud sent: this onus's schema, and the commit asked for.
pub fn parse_hosted_map(bytes: &[u8], want: Option<&str>) -> Result<onus_core::CodebaseMap> {
    let value: Value =
        serde_json::from_slice(bytes).context("Onus Cloud sent a map that is not JSON")?;
    let version = value.get("schemaVersion").and_then(Value::as_u64);
    if version != Some(u64::from(onus_core::map::SCHEMA_VERSION)) {
        bail!(
            "the map Onus Cloud keeps was built by onus {} (schema {}); this onus reads schema {}: update onus",
            value
                .pointer("/builtWith/onus")
                .and_then(Value::as_str)
                .unwrap_or("unknown"),
            version.map_or("none".to_string(), |v| v.to_string()),
            onus_core::map::SCHEMA_VERSION
        );
    }
    let map: onus_core::CodebaseMap =
        serde_json::from_value(value).context("Onus Cloud sent something that is not a map")?;
    if let Some(w) = want
        && is_sha(&map.commit)
        && map.commit != w
    {
        bail!(
            "Onus Cloud sent the map of {} when {w} was asked for",
            map.commit
        );
    }
    Ok(map)
}

/// A map asked for by `sha` names that commit, or carries a label (a map of a
/// checked-out folder, `worktree`) and Onus Cloud says in `x-onus-map-commit` that it is
/// of that commit. Anything else is neither used nor cached.
pub fn vouched(map_commit: &str, sha: &str, named: Option<&str>) -> Result<()> {
    if map_commit == sha || (!is_sha(map_commit) && named == Some(sha)) {
        return Ok(());
    }
    bail!(
        "Onus Cloud sent a map of {map_commit} (said to be of {}) when {sha} was asked for",
        named.unwrap_or("no commit")
    )
}

/// The map Onus Cloud keeps of a repository (`owner/name`) at `commit`, or of its
/// default branch's newest commit. Maps of a commit are kept in [`cache_dir`] and
/// never asked for again.
pub fn hosted_map(c: &Client, repo: &str, commit: Option<&str>) -> Result<onus_core::CodebaseMap> {
    if let Some(s) = commit
        && !is_sha(s)
    {
        bail!("`{s}` is not a full commit sha (40 hex characters)");
    }
    let org = c.organisation()?;
    let cache = cache_dir().ok();
    if let (Some(sha), Some(dir)) = (commit, &cache) {
        let path = map_cache_path(dir, &c.url, &org, repo, sha);
        if let Ok(bytes) = std::fs::read(&path)
            && let Ok(map) = parse_hosted_map(&bytes, Some(sha))
        {
            return Ok(at_commit(map, sha));
        }
    }
    let mut path = format!("/v1/orgs/{}/repos/{}/map", segment(&org), segment(repo));
    if let Some(sha) = commit {
        path.push_str(&format!("?commit={sha}"));
    }
    let (status, bytes, named) = c.get_raw(&path)?;
    if !(200..300).contains(&status) {
        return Err(c.api_error(status, &String::from_utf8_lossy(&bytes)));
    }
    let want = commit.or(named.as_deref().filter(|s| is_sha(s)));
    let map = parse_hosted_map(&bytes, want)?;
    if let Some(sha) = commit {
        vouched(&map.commit, sha, named.as_deref())?;
    }
    let Some(sha) = cached_sha(commit, named.as_deref()) else {
        return Ok(map);
    };
    if let Some(dir) = &cache {
        // Best effort: an unwritable cache only costs a download.
        let _ = write_atomically(&map_cache_path(dir, &c.url, &org, repo, &sha), &bytes);
    }
    Ok(at_commit(map, &sha))
}

/// A hosted map names the commit it is of. The worker maps a checked-out folder, so the
/// map itself may carry a label (`worktree`): the sha Onus Cloud vouched for replaces it.
pub fn at_commit(mut map: onus_core::CodebaseMap, sha: &str) -> onus_core::CodebaseMap {
    if !is_sha(&map.commit) {
        map.commit = sha.to_string();
    }
    map
}

/// The commit to ask the map of: a pull request's head as Onus Cloud analysed it, a
/// commit (a full sha, or anything `root` resolves), or none for the default branch's
/// newest map.
pub fn map_commit(
    c: &Client,
    root: &Path,
    repo: &str,
    commit: Option<&str>,
    pull: Option<u64>,
) -> Result<Option<String>> {
    match (commit, pull) {
        (Some(_), Some(_)) => bail!("give a commit or a pull request, not both"),
        (None, Some(n)) => {
            let p = c.get(&c.org_path(&format!("/repos/{}/pulls/{n}", segment(repo)))?)?;
            Ok(Some(
                p["pull"]["headSha"]
                    .as_str()
                    .with_context(|| format!("Onus Cloud knows no head commit of {repo}#{n}"))?
                    .to_string(),
            ))
        }
        (Some(s), None) if is_sha(s) => Ok(Some(s.to_string())),
        (Some(s), None) => {
            Ok(Some(commit_of(root, s).with_context(|| {
                format!("`{s}` names no commit here: give the full sha")
            })?))
        }
        (None, None) => Ok(None),
    }
}

fn write_atomically(path: &Path, bytes: &[u8]) -> Result<()> {
    let dir = path.parent().context("no folder")?;
    std::fs::create_dir_all(dir)?;
    let tmp = tempfile::NamedTempFile::new_in(dir)?;
    std::fs::write(tmp.path(), bytes)?;
    tmp.persist(path)?;
    Ok(())
}

/// The repository an agent's call is about: the one it names, else this worktree's
/// GitHub remote, else `ONUS_CLOUD_REPO`.
pub fn repository_for(root: &Path, named: Option<&str>) -> Option<String> {
    named
        .map(str::to_string)
        .or_else(|| repository_of(root))
        .or_else(|| env_var("ONUS_CLOUD_REPO"))
}

/// What the CLI calls itself on the approval page.
fn client_name() -> String {
    let host = env_var("HOSTNAME")
        .or_else(|| env_var("COMPUTERNAME"))
        .or_else(|| {
            Command::new("hostname")
                .output()
                .ok()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .filter(|h| !h.is_empty())
        })
        .unwrap_or_else(|| "this computer".into());
    format!(
        "onus {} on {host} ({})",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS
    )
}

/// Opens a URL in the default browser; false when that did not work.
pub fn open_browser(url: &str) -> bool {
    let mut cmd = if cfg!(target_os = "macos") {
        let mut c = Command::new("open");
        c.arg(url);
        c
    } else if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.args(["/C", "start", "", url]);
        c
    } else {
        let mut c = Command::new("xdg-open");
        c.arg(url);
        c
    };
    cmd.stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// `onus login`: starts a login, shows the code, waits for the person to approve it in the
/// browser, and saves what it is given.
pub fn login(
    url: &str,
    organisation: Option<&str>,
    browser: bool,
    out: &mut dyn Write,
) -> Result<(Credentials, PathBuf)> {
    let anon = Client::anonymous(url);
    let start = anon.post("/v1/cli/login", &json!({ "client": client_name() }))?;
    let code = start["userCode"]
        .as_str()
        .context("no code in the answer")?;
    let device = start["deviceCode"]
        .as_str()
        .context("no device code in the answer")?;
    let mut link = start["verificationUrlComplete"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    if let Some(o) = organisation {
        link.push_str(&format!("&org={}", segment(o)));
    }
    let interval = Duration::from_secs(start["interval"].as_u64().unwrap_or(2).max(1));
    let expires = Duration::from_secs(start["expiresIn"].as_u64().unwrap_or(600));
    writeln!(out, "Your one-time code: {code}")?;
    let opened = browser && open_browser(&link);
    if opened {
        writeln!(
            out,
            "Approve it in the browser window that opened ({link})."
        )?;
    } else {
        writeln!(out, "Open {link} and approve it.")?;
    }
    writeln!(out, "Waiting…")?;
    out.flush()?;
    let started = Instant::now();
    loop {
        std::thread::sleep(interval);
        let poll = anon.post("/v1/cli/login/token", &json!({ "deviceCode": device }))?;
        match poll["status"].as_str() {
            Some("pending") if started.elapsed() < expires => continue,
            Some("approved") => {
                let token = poll["token"].as_str().context("no token in the answer")?;
                let creds = Credentials {
                    url: anon.url.clone(),
                    token: token.to_string(),
                    organisation: poll["organisation"]["slug"]
                        .as_str()
                        .map(str::to_string)
                        .or_else(|| organisation.map(str::to_string)),
                    user: poll["user"].clone(),
                };
                let path = save_credentials(&creds)?;
                return Ok((creds, path));
            }
            Some("denied") => bail!("the sign-in was denied in the browser"),
            _ => bail!("the code expired before it was approved: run `onus login` again"),
        }
    }
}

/// `onus logout`: the token is revoked in Onus Cloud (when it can be reached) and forgotten here.
pub fn logout() -> Result<bool> {
    let Some(c) = load_credentials()? else {
        return Ok(false);
    };
    let client = Client {
        url: c.url.clone(),
        token: Some(c.token.clone()),
        org: None,
        via: "login",
        agent: agent(),
    };
    let _ = client.post("/v1/cli/logout", &json!({}));
    forget_credentials()
}

/// `owner/name#12`, `#12` or `12` (the repository from `dir`'s GitHub remote).
pub fn parse_pull(pull: &str, dir: &Path) -> Result<(String, u64)> {
    let (repo, number) = match pull.rsplit_once('#') {
        Some((r, n)) if !r.is_empty() => (Some(r.to_string()), n),
        Some((_, n)) => (None, n),
        None => (None, pull),
    };
    let number: u64 = number
        .trim()
        .parse()
        .with_context(|| format!("`{pull}` is not a pull request: write owner/name#12 or #12"))?;
    let repo = match repo {
        Some(r) => r,
        None => repository_of(dir)
            .context("this folder has no GitHub remote: name the pull request as owner/name#12")?,
    };
    Ok((repo, number))
}

/// What an agent may do in Onus Cloud through `onus mcp`: the same as the commands, with
/// the repository and commit taken from the agent's worktree when it does not name them.
/// Nothing here approves anything; evidence goes to the judge like any other.
pub fn act(client: &Client, root: &Path, name: &str, input: Value) -> Result<Value> {
    let text = |k: &str| input[k].as_str().map(str::trim).filter(|s| !s.is_empty());
    match name {
        "cloud.status" => {
            let me = client.whoami()?;
            let org = client.organisation().ok();
            let allowance = match &org {
                // A token without env:read still has a status.
                Some(_) => match client.get(&client.org_path("/environments")?) {
                    Ok(v) => v["allowance"].clone(),
                    Err(e) if is_status(&e, 403) => Value::Null,
                    Err(e) => return Err(e),
                },
                None => Value::Null,
            };
            Ok(json!({
                "url": client.url,
                "signedInWith": client.via,
                "who": if me["kind"] == "apiKey" { me["apiKey"].clone() } else { me["user"].clone() },
                "organisation": org,
                "organisations": me["organisations"],
                "token": me["token"],
                "environments": allowance,
            }))
        }
        "cloud.envCreate" => {
            let repository = match text("repository") {
                Some(r) => r.to_string(),
                None => repository_of(root)
                    .context("this worktree has no GitHub remote: pass repository (owner/name)")?,
            };
            let reference = match text("ref") {
                Some(r) => r.to_string(),
                None => pushed_head(root)?,
            };
            let mut body = json!({ "repository": repository, "ref": reference });
            if let Some(n) = text("name") {
                body["name"] = json!(n);
            }
            if let Some(h) = input["hosts"].as_array() {
                body["hosts"] = json!(h);
            }
            let env = client.create_environment(
                &body,
                Some(Duration::from_secs(30 * 60)),
                &mut |_| {},
            )?;
            let ready = env["status"] == "ready";
            Ok(json!({
                "environment": env,
                "next": if ready {
                    format!("Run commands in it with onus_cloud_env_run (name {}).", env["name"].as_str().unwrap_or_default())
                } else {
                    "It could not be built; the error says why.".to_string()
                },
            }))
        }
        "cloud.envRun" => {
            let env = text("name").context("name the environment")?;
            let command = text("command").context("give a command")?;
            let run =
                client.run_in_environment(env, command, input["timeoutSeconds"].as_u64(), true)?;
            let id = run["id"].as_str().unwrap_or_default();
            Ok(json!({
                "run": run,
                "next": format!(
                    "Recorded by Onus Cloud. Attach it to the pull request as evidence with onus_cloud_attach_evidence (run {id}); the judge checks it against the recording."
                ),
            }))
        }
        "cloud.envs" => {
            let all = input["all"].as_bool().unwrap_or(false);
            client.get(&client.org_path(if all {
                "/environments?all=1"
            } else {
                "/environments"
            })?)
        }
        "cloud.envDestroy" => {
            let env = text("name").context("name the environment")?;
            client.delete(&client.org_path(&format!("/environments/{}", segment(env)))?)
        }
        "cloud.pulls" => {
            let mut query = vec![format!(
                "limit={}",
                input["limit"].as_u64().unwrap_or(30).min(100)
            )];
            for k in ["state", "lane"] {
                if let Some(v) = text(k) {
                    query.push(format!("{k}={}", segment(v)));
                }
            }
            if input["attention"].as_bool() == Some(true) {
                query.push("attention=true".into());
            }
            if let Some(r) = text("repository") {
                let repo = client.get(&client.org_path(&format!("/repos/{}", segment(r)))?)?;
                let id = repo["repository"]["id"]
                    .as_str()
                    .or(repo["id"].as_str())
                    .unwrap_or_default();
                query.push(format!("repo={}", segment(id)));
            }
            client.get(&client.org_path(&format!("/pulls?{}", query.join("&")))?)
        }
        "cloud.pull" => {
            let (repo, number) = parse_pull(
                &match (text("repository"), input["number"].as_u64()) {
                    (Some(r), Some(n)) => format!("{r}#{n}"),
                    (None, Some(n)) => format!("#{n}"),
                    _ => text("pull")
                        .context("name the pull request: repository and number")?
                        .to_string(),
                },
                root,
            )?;
            client.get(&client.org_path(&format!("/repos/{}/pulls/{number}", segment(&repo)))?)
        }
        "cloud.attachEvidence" => {
            let run = text("run").context("name the run (its id from onus_cloud_env_run)")?;
            let (repo, number) = parse_pull(
                &match (text("repository"), input["number"].as_u64()) {
                    (Some(r), Some(n)) => format!("{r}#{n}"),
                    (None, Some(n)) => format!("#{n}"),
                    _ => bail!(
                        "name the pull request: number (and repository, unless it is this worktree's)"
                    ),
                },
                root,
            )?;
            client.post(
                &client.org_path(&format!(
                    "/repos/{}/pulls/{number}/evidence",
                    segment(&repo)
                ))?,
                &json!({ "environmentRun": run }),
            )
        }
        "cloud.map" => {
            let repository = repository_for(root, text("repository"))
                .context("this worktree has no GitHub remote: pass repository (owner/name)")?;
            let question: onus_index::Query = serde_json::from_value(input["question"].clone())
                .context("question is not a map question: {\"query\": \"dependents\", \"target\": …}, as onus query takes")?;
            let commit = map_commit(
                client,
                root,
                &repository,
                text("commit"),
                input["pull"].as_u64(),
            )?;
            let map = hosted_map(client, &repository, commit.as_deref())?;
            let index = onus_index::MapIndex::new(map);
            let result = index.answer(&question).map_err(anyhow::Error::msg)?;
            let m = index.map();
            Ok(json!({
                "result": result,
                "map": {
                    "repository": repository,
                    "commit": m.commit,
                    "onus": m.built_with.onus,
                    "source": "onus-cloud",
                },
                "componentDirs": index.status().component_dirs,
            }))
        }
        "cloud.tokenMint" => {
            let scopes: Vec<String> = input["scopes"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str())
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            if scopes.is_empty() {
                bail!("name the scopes it opens: {}", CLOUD_SCOPES.join(", "));
            }
            let named: Vec<String> = input["repositories"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str())
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            let repositories = if input["allRepositories"].as_bool() == Some(true) {
                None
            } else if !named.is_empty() {
                Some(named)
            } else {
                Some(vec![repository_for(root, None).context(
                    "this worktree has no GitHub remote: name the repositories (owner/name), or allRepositories",
                )?])
            };
            let ttl_seconds = input["ttlSeconds"].as_u64().unwrap_or(3600);
            if !TOKEN_TTL.contains(&ttl_seconds) {
                bail!("a token lasts 60 seconds to 24 hours (ttlSeconds 60 to 86400)");
            }
            let mut out = mint_token(
                client,
                &Narrow {
                    repositories,
                    scopes,
                    ttl_seconds,
                    label: text("label").map(str::to_string),
                },
            )?;
            out["next"] = json!(format!(
                "Give it to the sub-agent as ONUS_CLOUD_TOKEN. It opens only these doors, ends at {}, and cannot be widened.",
                out["expiresAt"].as_str().unwrap_or("its end")
            ));
            Ok(out)
        }
        _ => bail!("no cloud action `{name}`"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_remotes_name_their_repository() {
        for remote in [
            "git@github.com:acme/payments.git",
            "https://github.com/acme/payments",
            "https://github.com/acme/payments.git",
            "ssh://git@github.com/acme/payments.git",
            "https://x-access-token:abc@github.com/acme/payments.git",
        ] {
            assert_eq!(
                github_repository(remote).as_deref(),
                Some("acme/payments"),
                "{remote}"
            );
        }
        assert_eq!(github_repository("https://gitlab.com/acme/payments"), None);
        assert_eq!(github_repository("https://github.com/acme"), None);
    }

    #[test]
    fn path_segments_are_encoded() {
        assert_eq!(segment("acme/payments"), "acme%2Fpayments");
        assert_eq!(segment("e-1.2_x~"), "e-1.2_x~");
        assert_eq!(segment("a b#c"), "a%20b%23c");
    }

    #[test]
    fn a_token_beats_an_api_key_which_beats_the_login() {
        let saved = || {
            Ok(Some(Credentials {
                url: "http://login.example".into(),
                token: "onus_cli_login".into(),
                organisation: Some("acme".into()),
                user: Value::Null,
            }))
        };
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |k: &str| {
                pairs
                    .iter()
                    .find(|(n, _)| *n == k)
                    .map(|(_, v)| v.to_string())
            }
        };
        let never = || -> Result<Option<Credentials>> { panic!("cloud.json was read") };
        let c = Client::discover_from(
            &env(&[
                ("ONUS_CLOUD_TOKEN", "onus_tok_a"),
                ("ONUS_API_KEY", "onus_key"),
            ]),
            &never,
        )
        .unwrap()
        .unwrap();
        assert_eq!((c.via, c.token.as_deref()), ("token", Some("onus_tok_a")));
        assert_eq!(c.url, DEFAULT_URL);
        let c = Client::discover_from(
            &env(&[("ONUS_CLOUD_TOKEN", "  "), ("ONUS_API_KEY", "onus_key")]),
            &never,
        )
        .unwrap()
        .unwrap();
        assert_eq!(c.via, "api-key");
        let c = Client::discover_from(
            &env(&[("ONUS_CLOUD_TOKEN", ""), ("ONUS_CLOUD_ORG", "beta")]),
            &saved,
        )
        .unwrap()
        .unwrap();
        assert_eq!((c.via, c.url.as_str()), ("login", "http://login.example"));
        assert_eq!(c.org.as_deref(), Some("beta"));
        assert!(
            Client::discover_from(&env(&[]), &|| Ok(None))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn token_lifetimes_are_a_minute_to_a_day() {
        for (s, secs) in [
            ("90", 90),
            ("90s", 90),
            ("30m", 1800),
            ("8h", 28_800),
            ("1d", 86_400),
        ] {
            assert_eq!(parse_ttl(s).unwrap(), secs, "{s}");
        }
        for s in ["0", "59s", "2d", "25h", "abc", "1w", ""] {
            assert!(parse_ttl(s).is_err(), "{s}");
        }
        assert_eq!(parse_utc("2026-10-12T18:00:00.000Z"), Some(1_791_828_000));
        assert_eq!(parse_utc("2026-10-12T18:00:00Z"), Some(1_791_828_000));
        assert_eq!(parse_utc("yesterday"), None);
    }

    #[test]
    fn a_missing_scope_says_how_to_get_it() {
        let e = ApiError {
            status: 403,
            code: Some("insufficient_scope".into()),
            scope: Some("env:read".into()),
            message: "Denied: this token does not have env:read (it has pulls:read).".into(),
            via: "token",
        };
        assert_eq!(
            e.to_string(),
            "Denied: this token does not have env:read (it has pulls:read). (mint one with `onus cloud token create --scope env:read`, or ask with onus_escalate)"
        );
        let ended = ApiError {
            status: 401,
            code: None,
            scope: None,
            message: "This token ended".into(),
            via: "token",
        };
        assert!(ended.to_string().contains("ask for a new one"), "{ended}");
        let e: anyhow::Error = ended.into();
        assert!(is_status(&e, 401) && !is_status(&e, 403));
        let mint = mint_token(
            &Client::discover_from(&|k| (k == "ONUS_API_KEY").then(|| "k".into()), &|| Ok(None))
                .unwrap()
                .unwrap(),
            &Narrow {
                repositories: None,
                scopes: vec!["pulls:read".into()],
                ttl_seconds: 60,
                label: None,
            },
        )
        .unwrap_err();
        assert!(
            mint.to_string().contains("API keys cannot mint tokens"),
            "{mint}"
        );
    }

    fn map_json(commit: &str, schema: u32) -> Vec<u8> {
        let dir = tempfile::tempdir().unwrap();
        let mut map = serde_json::to_value(crate::build(dir.path(), None, None).unwrap()).unwrap();
        map["commit"] = json!(commit);
        map["schemaVersion"] = json!(schema);
        map["builtWith"]["onus"] = json!("9.9.9");
        serde_json::to_vec(&map).unwrap()
    }

    #[test]
    fn hosted_maps_are_this_schema_and_the_commit_asked_for() {
        let sha = "3f2a1c0000000000000000000000000000000000";
        let other = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let current = onus_core::map::SCHEMA_VERSION;
        assert_eq!(
            parse_hosted_map(&map_json(sha, current), Some(sha))
                .unwrap()
                .commit,
            sha
        );
        let err = parse_hosted_map(&map_json(sha, current + 1), Some(sha)).unwrap_err();
        assert!(err.to_string().contains("built by onus 9.9.9"), "{err}");
        assert!(err.to_string().contains("update onus"), "{err}");
        let err = parse_hosted_map(&map_json(other, current), Some(sha)).unwrap_err();
        assert!(err.to_string().contains("when 3f2a1c"), "{err}");
        // A label (a map of a folder) is not a commit to compare: Onus Cloud vouches.
        assert!(parse_hosted_map(&map_json("worktree", current), Some(sha)).is_ok());
        assert!(vouched("worktree", sha, Some(sha)).is_ok());
        assert!(vouched(sha, sha, None).is_ok());
        assert!(vouched("worktree", sha, None).is_err());
        assert!(vouched("worktree", sha, Some(other)).is_err());
        assert!(vouched(other, sha, Some(sha)).is_err());
        // The vouched sha replaces the label, so answers name the commit.
        let labelled = parse_hosted_map(&map_json("worktree", current), Some(sha)).unwrap();
        assert_eq!(at_commit(labelled, sha).commit, sha);
        assert!(parse_hosted_map(b"{", None).is_err());
    }

    #[test]
    fn hosted_maps_are_cached_by_their_full_sha_only() {
        let sha = "3f2a1c0000000000000000000000000000000000";
        let path = map_cache_path(
            Path::new("/c"),
            "http://localhost:8080",
            "acme",
            "acme/shop",
            sha,
        );
        assert_eq!(
            path,
            Path::new("/c/cloud-maps/localhost_8080/acme/acme__shop").join(format!("{sha}.json"))
        );
        assert_eq!(cached_sha(Some(sha), None).as_deref(), Some(sha));
        assert_eq!(cached_sha(None, Some(sha)).as_deref(), Some(sha));
        assert_eq!(cached_sha(None, None), None);
        assert_eq!(cached_sha(Some("main"), None), None);
        assert_eq!(cached_sha(None, Some("3f2a1c")), None);
        assert!(is_sha(sha) && !is_sha(&sha.to_uppercase()) && !is_sha("HEAD"));
    }

    #[test]
    fn credentials_are_kept_for_their_owner_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("onus").join("cloud.json");
        let c = Credentials {
            url: "http://localhost:8080".into(),
            token: "onus_cli_x".into(),
            organisation: Some("acme".into()),
            user: json!({ "email": "ada@example.com" }),
        };
        save_credentials_at(&path, &c).unwrap();
        assert_eq!(load_credentials_at(&path).unwrap(), Some(c));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        assert!(forget_credentials_at(&path).unwrap());
        assert!(!forget_credentials_at(&path).unwrap());
        assert_eq!(load_credentials_at(&path).unwrap(), None);
    }
}
