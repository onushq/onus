//! Task tokens: Biscuit tokens that carry a task's rights.
//!
//! A token's first block, signed with the root key, holds the task id, one
//! `right(kind, pattern, regex)` fact per right, the agent the task is for
//! when the plan names one (`agent(tool, model, config, team)`), the
//! escalation request it grants when it was minted for one
//! (`escalation(id)`) and a check that the token has not expired. Whoever
//! holds a token can attenuate it: an appended block of checks that narrows
//! what it allows (for a sub-agent). Nobody can widen it: facts in later
//! blocks are not trusted by the authorizer, and changing the first block
//! breaks its signature.
//!
//! Values reach Datalog only as parameters, never as text pasted into a
//! rule, so a path cannot inject logic.

use std::collections::HashMap;

use anyhow::{Context, Result, anyhow, bail};
/// The root public key type tokens are verified with.
pub use biscuit_auth::PublicKey;
use biscuit_auth::builder::{Algorithm, Term};
use biscuit_auth::{AuthorizerBuilder, Biscuit, BlockBuilder, KeyPair, PrivateKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::scope::{Kind, Right, Scope};

/// The key that mints tokens. Keep it with the operator, never with an
/// agent; the public key alone verifies tokens.
pub struct RootKey {
    pair: KeyPair,
}

impl std::fmt::Debug for RootKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RootKey(…)")
    }
}

impl RootKey {
    pub fn generate() -> RootKey {
        RootKey {
            pair: KeyPair::new(),
        }
    }

    pub fn from_hex(private_hex: &str) -> Result<RootKey> {
        let private = PrivateKey::from_bytes_hex(private_hex.trim(), Algorithm::Ed25519)
            .map_err(|e| anyhow!("not an Ed25519 private key: {e}"))?;
        Ok(RootKey {
            pair: KeyPair::from(&private),
        })
    }

    pub fn private_hex(&self) -> String {
        self.pair.private().to_bytes_hex().to_string()
    }

    pub fn public_hex(&self) -> String {
        self.pair.public().to_bytes_hex()
    }

    pub fn public(&self) -> PublicKey {
        self.pair.public()
    }
}

pub fn public_key(hex: &str) -> Result<PublicKey> {
    PublicKey::from_bytes_hex(hex.trim(), Algorithm::Ed25519)
        .map_err(|e| anyhow!("not an Ed25519 public key: {e}"))
}

/// The agent a task is for, as its plan names it. Only the root key writes
/// it, so whoever holds the token cannot change it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentClaim {
    /// The harness, such as `claude-code` or `codex`.
    pub tool: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub config: String,
    #[serde(default)]
    pub team: String,
}

/// What a token grants.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Grant {
    pub task: String,
    pub rights: Vec<Right>,
    /// Seconds since the Unix epoch.
    pub expires: u64,
    /// The agent the task is for, when the plan names one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<AgentClaim>,
}

/// A short id for a token, to log instead of the token: the first 16 hex
/// characters of its sha256.
pub fn token_id(raw: &str) -> String {
    Sha256::digest(raw.trim().as_bytes())[..8]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Datalog limits: generous on time (regexes compile inside the run, and
/// debug builds are slow), bounded on facts and iterations.
fn limits() -> biscuit_auth::AuthorizerLimits {
    biscuit_auth::AuthorizerLimits {
        max_facts: 10_000,
        max_iterations: 100,
        max_time: std::time::Duration::from_millis(500),
    }
}

fn params(pairs: &[(&str, Term)]) -> HashMap<String, Term> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect()
}

fn str(s: &str) -> Term {
    Term::Str(s.to_string())
}

/// Mints a token for `grant`, signed with `root`.
pub fn mint(root: &RootKey, grant: &Grant) -> Result<String> {
    build(root, grant, None)
}

/// Mints the token that grants escalation request `request`: `grant`, plus
/// the request's id, so the token proves which request it was granted for.
pub fn mint_granted(root: &RootKey, grant: &Grant, request: &str) -> Result<String> {
    build(root, grant, Some(request))
}

fn build(root: &RootKey, grant: &Grant, request: Option<&str>) -> Result<String> {
    if grant.task.is_empty() {
        bail!("a token needs a task id");
    }
    let mut builder = Biscuit::builder()
        .code_with_params(
            "task({task});",
            params(&[("task", str(&grant.task))]),
            HashMap::new(),
        )
        .map_err(|e| anyhow!("{e}"))?;
    for r in &grant.rights {
        builder = builder
            .code_with_params(
                "right({kind}, {pattern}, {regex});",
                params(&[
                    ("kind", str(r.kind.as_str())),
                    ("pattern", str(&r.pattern)),
                    ("regex", str(&r.regex())),
                ]),
                HashMap::new(),
            )
            .map_err(|e| anyhow!("{e}"))?;
    }
    if let Some(a) = &grant.agent {
        builder = builder
            .code_with_params(
                "agent({tool}, {model}, {config}, {team});",
                params(&[
                    ("tool", str(&a.tool)),
                    ("model", str(&a.model)),
                    ("config", str(&a.config)),
                    ("team", str(&a.team)),
                ]),
                HashMap::new(),
            )
            .map_err(|e| anyhow!("{e}"))?;
    }
    if let Some(id) = request {
        builder = builder
            .code_with_params(
                "escalation({id});",
                params(&[("id", str(id))]),
                HashMap::new(),
            )
            .map_err(|e| anyhow!("{e}"))?;
    }
    builder = builder
        .code_with_params(
            "check if time($t), $t <= {expires};",
            params(&[("expires", Term::Date(grant.expires))]),
            HashMap::new(),
        )
        .map_err(|e| anyhow!("{e}"))?;
    let token = builder.build(&root.pair).map_err(|e| anyhow!("{e}"))?;
    token.to_base64().map_err(|e| anyhow!("{e}"))
}

/// A token whose signature checked out, with what its first block grants.
#[derive(Debug)]
pub struct Verified {
    token: Biscuit,
    pub task: String,
    pub expires: u64,
    /// The rights of the first block; attenuations may narrow them.
    pub rights: Vec<Right>,
    /// The checks each attenuation added, as Datalog.
    pub attenuations: Vec<String>,
    /// The agent the root key named, if any (tokens minted before agents
    /// were named have none).
    pub agent: Option<AgentClaim>,
    /// The escalation request the root key granted with this token, if it
    /// was minted for one.
    pub escalation: Option<String>,
}

/// Checks a token's signatures and reads what it grants. It does not check
/// expiry or attenuations: [`Verified::authorize`] does, per operation.
pub fn verify(token: &str, root: &PublicKey) -> Result<Verified> {
    let token = Biscuit::from_base64(token.trim(), *root)
        .map_err(|e| anyhow!("the token is not valid: {e}"))?;
    let mut authorizer = AuthorizerBuilder::new()
        .set_limits(limits())
        .build(&token)
        .map_err(|e| anyhow!("{e}"))?;
    let tasks: Vec<(String,)> = authorizer
        .query("t($t) <- task($t)")
        .map_err(|e| anyhow!("{e}"))?;
    let task = tasks
        .into_iter()
        .next()
        .map(|(t,)| t)
        .context("the token names no task")?;
    let rights: Vec<(String, String)> = authorizer
        .query("r($k, $p) <- right($k, $p, $re)")
        .map_err(|e| anyhow!("{e}"))?;
    let agents: Vec<(String, String, String, String)> = authorizer
        .query("a($t, $m, $c, $e) <- agent($t, $m, $c, $e)")
        .map_err(|e| anyhow!("{e}"))?;
    let agent = agents
        .into_iter()
        .next()
        .map(|(tool, model, config, team)| AgentClaim {
            tool,
            model,
            config,
            team,
        });
    let escalations: Vec<(String,)> = authorizer
        .query("e($i) <- escalation($i)")
        .map_err(|e| anyhow!("{e}"))?;
    let escalation = escalations.into_iter().next().map(|(i,)| i);
    let mut parsed = Vec::new();
    for (k, p) in rights {
        let kind = Kind::parse(&k).with_context(|| format!("unknown right kind `{k}`"))?;
        parsed.push(Right::new(kind, p));
    }
    let source = token.print_block_source(0).map_err(|e| anyhow!("{e}"))?;
    let expires = source
        .lines()
        .find_map(|l| {
            let at = l.find("<= ")?;
            let date = l[at + 3..].trim_end_matches(';').trim();
            parse_rfc3339(date)
        })
        .unwrap_or(0);
    let attenuations = (1..token.block_count())
        .filter_map(|i| token.print_block_source(i).ok())
        .collect();
    let scope = Scope::new(parsed);
    Ok(Verified {
        token,
        task,
        expires,
        rights: scope.rights,
        attenuations,
        agent,
        escalation,
    })
}

/// `2026-10-08T19:00:00Z` → Unix seconds (UTC, as Biscuit prints dates).
fn parse_rfc3339(s: &str) -> Option<u64> {
    let s = s.strip_suffix('Z').or_else(|| s.strip_suffix("+00:00"))?;
    let (date, time) = s.split_once('T')?;
    let d: Vec<i64> = date
        .split('-')
        .map(|p| p.parse().ok())
        .collect::<Option<_>>()?;
    let t: Vec<i64> = time
        .split(':')
        .map(|p| p.split('.').next().unwrap_or(p).parse().ok())
        .collect::<Option<_>>()?;
    let (y, m, day) = (d[0], d[1], d[2]);
    // Days from civil (Howard Hinnant's algorithm).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    u64::try_from(days * 86_400 + t[0] * 3600 + t[1] * 60 + t.get(2).copied().unwrap_or(0)).ok()
}

impl Verified {
    /// Whether the token allows `kind` on `value` at `now` (Unix seconds),
    /// after every attenuation. A read is allowed by a read or a write right.
    pub fn authorize(&self, kind: Kind, value: &str, now: u64) -> Result<()> {
        let mut authorizer = AuthorizerBuilder::new()
            .code_with_params(
                "operation({kind}, {value});
                 time({now});
                 allow if operation($k, $v), right($k, $p, $re), $v.matches($re);
                 allow if operation(\"read\", $v), right(\"write\", $p, $re), $v.matches($re);
                 deny if true;",
                params(&[
                    ("kind", str(kind.as_str())),
                    ("value", str(value)),
                    ("now", Term::Date(now)),
                ]),
                HashMap::new(),
            )
            .map_err(|e| anyhow!("{e}"))?
            .set_limits(limits())
            .build(&self.token)
            .map_err(|e| anyhow!("{e}"))?;
        authorizer.authorize().map(|_| ()).map_err(|e| {
            anyhow!(
                "`{}` is not allowed for task `{}`: {}",
                Right::new(kind, value),
                self.task,
                explain(&e)
            )
        })
    }

    /// Attenuates the token to `narrower`: for every kind `narrower` names,
    /// only values it allows stay allowed. Kinds it does not name keep what
    /// the token allowed.
    pub fn attenuate(&self, narrower: &[Right]) -> Result<String> {
        // Narrowing guards would unprotect paths; guards only come from the
        // root key.
        if let Some(r) = narrower.iter().find(|r| r.kind == Kind::Guard) {
            bail!("`{r}`: guards cannot be attenuated; they only come with the token");
        }
        let mut block = BlockBuilder::new();
        let mut kinds: Vec<Kind> = narrower.iter().map(|r| r.kind).collect();
        kinds.sort();
        kinds.dedup();
        for kind in kinds {
            let mut p = vec![("kind".to_string(), str(kind.as_str()))];
            let mut alts = vec!["operation($k, $v), $k != {kind}".to_string()];
            for (i, r) in narrower.iter().filter(|r| r.kind == kind).enumerate() {
                alts.push(format!("operation({{kind}}, $v), $v.matches({{re{i}}})"));
                p.push((format!("re{i}"), str(&r.regex())));
                // A narrowed write also narrows the reads it implies.
                if kind == Kind::Write {
                    alts.push(format!("operation(\"read\", $v), $v.matches({{re{i}}})"));
                }
            }
            let source = format!("check if {};", alts.join(" or "));
            block = block
                .code_with_params(source, p.into_iter().collect(), HashMap::new())
                .map_err(|e| anyhow!("{e}"))?;
        }
        self.token
            .append(block)
            .and_then(|t| t.to_base64())
            .map_err(|e| anyhow!("{e}"))
    }

    pub fn scope(&self) -> Scope {
        Scope::new(self.rights.clone())
    }
}

/// A readable reason from a Biscuit authorization error.
fn explain(e: &biscuit_auth::error::Token) -> String {
    let text = e.to_string();
    if text.contains("time(") || text.contains("<=") {
        "the token has expired".into()
    } else if text.contains("NoMatchingPolicy") || text.contains("deny") || text.contains("Deny") {
        "no right in the token covers it".into()
    } else if text.contains("check") || text.contains("Check") {
        "an attenuation of the token excludes it".into()
    } else {
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_791_500_000; // 2026-10-09
    const LATER: u64 = NOW + 3600;

    fn rights(list: &[&str]) -> Vec<Right> {
        list.iter().map(|s| s.parse().unwrap()).collect()
    }

    fn grant() -> Grant {
        Grant {
            task: "task-1".into(),
            rights: rights(&[
                "write:path:services/notifications/**",
                "read:path:services/orders/**",
                "net:host:sms.test.example",
            ]),
            expires: LATER,
            agent: None,
        }
    }

    #[test]
    fn a_token_allows_its_rights_and_nothing_else() {
        let root = RootKey::generate();
        let token = mint(&root, &grant()).unwrap();
        let v = verify(&token, &root.public()).unwrap();
        assert_eq!(v.task, "task-1");
        assert_eq!(v.expires, LATER);
        assert_eq!(v.rights.len(), 3);
        let ok = |k, p: &str| v.authorize(k, p, NOW).is_ok();
        assert!(ok(Kind::Write, "services/notifications/src/sms.ts"));
        assert!(ok(Kind::Read, "services/notifications/src/sms.ts"));
        assert!(ok(Kind::Read, "services/orders/src/events.ts"));
        assert!(!ok(Kind::Write, "services/orders/src/events.ts"));
        assert!(!ok(Kind::Write, "services/billing/src/charge.ts"));
        assert!(ok(Kind::Net, "sms.test.example"));
        assert!(!ok(Kind::Net, "evil.example"));
        // Expired.
        let err = v
            .authorize(Kind::Write, "services/notifications/a.ts", LATER + 1)
            .unwrap_err();
        assert!(err.to_string().contains("expired"), "{err}");
    }

    #[test]
    fn attenuation_narrows_and_never_widens() {
        let root = RootKey::generate();
        let token = mint(&root, &grant()).unwrap();
        let v = verify(&token, &root.public()).unwrap();
        let narrow = v
            .attenuate(&rights(&["write:path:services/notifications/src/sms/**"]))
            .unwrap();
        let n = verify(&narrow, &root.public()).unwrap();
        assert_eq!(n.attenuations.len(), 1);
        assert!(
            n.authorize(Kind::Write, "services/notifications/src/sms/a.ts", NOW)
                .is_ok()
        );
        assert!(
            n.authorize(Kind::Write, "services/notifications/src/email.ts", NOW)
                .is_err()
        );
        // Other kinds keep what the token allowed.
        assert!(n.authorize(Kind::Net, "sms.test.example", NOW).is_ok());
        // "Attenuating" to something wider grants nothing new.
        let wider = v.attenuate(&rights(&["write:path:**"])).unwrap();
        let w = verify(&wider, &root.public()).unwrap();
        assert!(
            w.authorize(Kind::Write, "services/billing/a.ts", NOW)
                .is_err()
        );
    }

    #[test]
    fn tampered_or_foreign_tokens_are_rejected() {
        let root = RootKey::generate();
        let token = mint(&root, &grant()).unwrap();
        let other = RootKey::generate();
        assert!(verify(&token, &other.public()).is_err());
        let mut bytes: Vec<char> = token.chars().collect();
        let mid = bytes.len() / 2;
        bytes[mid] = if bytes[mid] == 'A' { 'B' } else { 'A' };
        let tampered: String = bytes.into_iter().collect();
        assert!(verify(&tampered, &root.public()).is_err());
        // Keys round-trip through hex.
        let again = RootKey::from_hex(&root.private_hex()).unwrap();
        assert_eq!(again.public_hex(), root.public_hex());
        assert!(verify(&token, &public_key(&root.public_hex()).unwrap()).is_ok());
    }

    #[test]
    fn values_cannot_inject_datalog() {
        let root = RootKey::generate();
        let token = mint(&root, &grant()).unwrap();
        let v = verify(&token, &root.public()).unwrap();
        let sneaky = "x\"); right(\"write\", \"**\", \".*\"); operation(\"write\", \"a";
        assert!(v.authorize(Kind::Write, sneaky, NOW).is_err());
    }

    #[test]
    fn the_agent_comes_from_the_root_key_only() {
        let root = RootKey::generate();
        // Tokens minted without an agent have none.
        let plain = mint(&root, &grant()).unwrap();
        assert_eq!(verify(&plain, &root.public()).unwrap().agent, None);

        let claim = AgentClaim {
            tool: "claude-code".into(),
            model: "opus".into(),
            config: String::new(),
            team: "growth".into(),
        };
        let mut g = grant();
        g.agent = Some(claim.clone());
        let token = mint(&root, &g).unwrap();
        let v = verify(&token, &root.public()).unwrap();
        assert_eq!(v.agent.as_ref(), Some(&claim));
        // Attenuation keeps it.
        let narrow = v
            .attenuate(&rights(&["write:path:services/notifications/src/**"]))
            .unwrap();
        assert_eq!(verify(&narrow, &root.public()).unwrap().agent, Some(claim));
        // Values that look like Datalog are stored as they are.
        let sneaky = "x\"); right(\"write\", \"**\", \".*\"); agent(\"y";
        g.agent = Some(AgentClaim {
            tool: sneaky.into(),
            ..Default::default()
        });
        let v = verify(&mint(&root, &g).unwrap(), &root.public()).unwrap();
        assert_eq!(v.agent.as_ref().unwrap().tool, sneaky);
        assert_eq!(v.rights.len(), 3);
        assert!(
            v.authorize(Kind::Write, "services/billing/a.ts", NOW)
                .is_err()
        );
        // Ids are short, stable and not the token.
        assert_eq!(token_id(&token).len(), 16);
        assert_eq!(token_id(&format!(" {token}\n")), token_id(&token));
    }

    #[test]
    fn a_granted_token_names_its_request_and_only_the_root_key_can() {
        let root = RootKey::generate();
        let plain = verify(&mint(&root, &grant()).unwrap(), &root.public()).unwrap();
        assert_eq!(plain.escalation, None);
        let granted = mint_granted(&root, &grant(), "a1b2c3d4e5f6").unwrap();
        let v = verify(&granted, &root.public()).unwrap();
        assert_eq!(v.escalation.as_deref(), Some("a1b2c3d4e5f6"));
        // A block appended by whoever holds a token is not trusted for it.
        let forged = plain
            .token
            .append(
                BlockBuilder::new()
                    .code("escalation(\"a1b2c3d4e5f6\");")
                    .unwrap(),
            )
            .unwrap()
            .to_base64()
            .unwrap();
        assert_eq!(verify(&forged, &root.public()).unwrap().escalation, None);
    }
}
