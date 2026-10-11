//! Onus Cloud from the command line and from agents.
//!
//! `onus login` signs the CLI in as a person: it asks Onus Cloud for a code, the person
//! approves it in the browser (the device authorization grant, RFC 8628), and the CLI
//! keeps the token it is given in `~/.config/onus/cloud.json`. In CI, an organisation's
//! API key in `ONUS_API_KEY` stands in for a login. Either way [`Client`] calls the API:
//! hosted environments Onus Cloud builds and records, pull requests and their lanes, and
//! evidence attached to them.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// The hosted API, unless `--url` or `ONUS_CLOUD_URL` names another.
pub const DEFAULT_URL: &str = "https://api.onushq.com";

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
    pub code: String,
    pub message: String,
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.status {
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
            _ => write!(f, "{} ({})", self.message, self.status),
        }
    }
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
pub struct Client {
    pub url: String,
    token: Option<String>,
    /// The organisation (slug or id) commands act in, when known.
    pub org: Option<String>,
    /// How the client signed in: `login` (a person, from `onus login`) or `api-key`.
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

    /// How this process is signed in: `ONUS_API_KEY` (CI), else `onus login`'s
    /// credentials; none when neither. `ONUS_CLOUD_URL` and `ONUS_CLOUD_ORG` override
    /// where and in which organisation.
    pub fn discover() -> Result<Option<Client>> {
        let url = env_var("ONUS_CLOUD_URL");
        let org = env_var("ONUS_CLOUD_ORG");
        if let Some(key) = env_var("ONUS_API_KEY") {
            return Ok(Some(Client {
                url: url
                    .unwrap_or_else(|| DEFAULT_URL.into())
                    .trim_end_matches('/')
                    .into(),
                token: Some(key),
                org,
                via: "api-key",
                agent: agent(),
            }));
        }
        let Some(c) = load_credentials()? else {
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

    /// [`Client::discover`], or the reason there is none.
    pub fn require() -> Result<Client> {
        Client::discover()?.ok_or_else(|| {
            anyhow!("not signed in to Onus Cloud: run `onus login` (in CI, set ONUS_API_KEY)")
        })
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
        let body: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
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
        Err(ApiError {
            status,
            code: body["error"].as_str().unwrap_or_default().to_string(),
            message,
        }
        .into())
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
                Some(_) => client.get(&client.org_path("/environments")?)?["allowance"].clone(),
                None => Value::Null,
            };
            Ok(json!({
                "url": client.url,
                "signedInWith": client.via,
                "who": if me["kind"] == "apiKey" { me["apiKey"].clone() } else { me["user"].clone() },
                "organisation": org,
                "organisations": me["organisations"],
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
                None => {
                    let head = commit_of(root, "HEAD").context("this worktree has no commit")?;
                    if !is_pushed(root, &head) {
                        bail!(
                            "HEAD ({}) is not pushed, and Onus Cloud builds from GitHub: push it first, or pass ref",
                            &head[..12.min(head.len())]
                        );
                    }
                    head
                }
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
