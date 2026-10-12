//! Evidence-based escalation: an agent that needs more than its token
//! allows asks for it in a structured request, with evidence graded from
//! strongest to weakest:
//!
//! 1. a failing test (reproduced in a container by the test runner);
//! 2. a trace;
//! 3. a path through the map;
//! 4. a draft diff;
//! 5. a rationale.
//!
//! Onus computes the request's blast radius and the sensitivity labels it
//! touches from the map. Low-risk requests with reproduced grade-1 evidence
//! are granted automatically; the rest go to a person. A grant is a new
//! token with the original rights plus exactly what was asked, for the rest
//! of the task.

use std::collections::BTreeSet;

use anyhow::{Context, Result, bail};
use onus_core::CodebaseMap;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::scope::{Kind, Right, glob_regex};
use crate::token::{Grant, RootKey, Verified, mint_granted};
use biscuit_auth::PublicKey;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EscalationKind {
    /// The task needs a right its token does not have.
    Permission,
    /// A test the task did not write fails, and the task cannot fix it in scope.
    BrokenTest,
    /// The specification contradicts itself or the code.
    ContradictorySpec,
    /// The task cannot be done as asked.
    ImpossibleTask,
}

/// One piece of evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Evidence {
    /// 1 (strongest) to 5.
    pub grade: u8,
    /// `failing-test`, `trace`, `map-path`, `draft-diff` or `rationale`.
    pub kind: String,
    /// The test file, trace file, map path, diff file or text.
    pub reference: String,
    /// For a failing test: whether the test runner saw it fail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reproduced: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<crate::runner::TestRun>,
}

impl Evidence {
    /// `failing-test:<file>`, `trace:<file>`, `map-path:<a -> b>`,
    /// `draft-diff:<file>` or `rationale:<text>`.
    pub fn parse(s: &str) -> Result<Evidence> {
        let (kind, reference) = s
            .split_once(':')
            .with_context(|| format!("`{s}`: write evidence as <kind>:<reference>"))?;
        let grade = match kind {
            "failing-test" => 1,
            "trace" => 2,
            "map-path" => 3,
            "draft-diff" => 4,
            "rationale" => 5,
            _ => bail!(
                "`{kind}` is not a kind of evidence: failing-test, trace, map-path, draft-diff or rationale"
            ),
        };
        Ok(Evidence {
            grade,
            kind: kind.to_string(),
            reference: reference.to_string(),
            reproduced: None,
            run: None,
        })
    }
}

/// A request for more than a token allows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    pub id: String,
    pub task: String,
    pub kind: EscalationKind,
    /// The rights asked for.
    pub scopes: Vec<Right>,
    pub evidence: Vec<Evidence>,
    pub reason: String,
    /// Files (outside the asked paths) that depend on code in them.
    pub blast_radius: u32,
    /// Sensitivity labels of the components the asked paths are in.
    pub sensitive: Vec<String>,
    /// Seconds since the Unix epoch.
    pub at: u64,
}

impl Request {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        task: &str,
        kind: EscalationKind,
        scopes: Vec<Right>,
        evidence: Vec<Evidence>,
        reason: &str,
        map: Option<&CodebaseMap>,
        sensitive_labels: &[String],
        at: u64,
    ) -> Request {
        let (blast_radius, sensitive) = match map {
            Some(m) => impact(m, &scopes, sensitive_labels),
            None => (0, vec![]),
        };
        let mut r = Request {
            id: String::new(),
            task: task.to_string(),
            kind,
            scopes,
            evidence,
            reason: reason.to_string(),
            blast_radius,
            sensitive,
            at,
        };
        r.id = r.content_id();
        r
    }

    /// The id the request's contents give it: the first 6 bytes of the
    /// sha256 of its JSON with the id left empty. A request changed after it
    /// was filed no longer matches its id.
    pub fn content_id(&self) -> String {
        let mut copy = self.clone();
        copy.id = String::new();
        let json = serde_json::to_string(&copy).unwrap_or_default();
        let mut h = Sha256::new();
        h.update(json.as_bytes());
        h.finalize()[..6]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    pub fn best_grade(&self) -> Option<u8> {
        self.evidence.iter().map(|e| e.grade).min()
    }
}

/// Files outside the asked paths that depend on code inside them, and the
/// sensitive labels of the components the asked paths are in.
fn impact(map: &CodebaseMap, scopes: &[Right], sensitive_labels: &[String]) -> (u32, Vec<String>) {
    let res: Vec<regex::Regex> = scopes
        .iter()
        .filter(|r| matches!(r.kind, Kind::Write | Kind::Read))
        .filter_map(|r| regex::Regex::new(&glob_regex(&r.pattern)).ok())
        .collect();
    let inside = |p: &str| res.iter().any(|re| re.is_match(p));
    let files: BTreeSet<&str> = map
        .files
        .iter()
        .filter(|f| inside(&f.path))
        .map(|f| f.path.as_str())
        .collect();
    let symbols: BTreeSet<&str> = map
        .symbols
        .iter()
        .filter(|s| {
            s.loc
                .as_ref()
                .is_some_and(|l| files.contains(l.file.as_str()))
        })
        .map(|s| s.id.as_str())
        .collect();
    let mut dependents: BTreeSet<&str> = BTreeSet::new();
    for e in &map.edges {
        if !e.kind.is_code_dependency() || !symbols.contains(e.to.as_str()) {
            continue;
        }
        for s in &e.sites {
            if !inside(&s.file) {
                dependents.insert(s.file.as_str());
            }
        }
    }
    let comps: BTreeSet<&str> = map
        .files
        .iter()
        .filter(|f| files.contains(f.path.as_str()))
        .filter_map(|f| f.component_id.as_deref())
        .collect();
    let mut labels: BTreeSet<String> = BTreeSet::new();
    for c in &map.components {
        if comps.contains(c.id.as_str()) {
            labels.extend(
                c.labels
                    .iter()
                    .filter(|l| sensitive_labels.contains(l))
                    .cloned(),
            );
        }
    }
    (dependents.len() as u32, labels.into_iter().collect())
}

/// When a request is granted without a person.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Policy {
    /// The most dependent files an automatic grant may reach.
    pub max_blast_radius: u32,
}

impl Default for Policy {
    fn default() -> Policy {
        Policy {
            max_blast_radius: 20,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "decision")]
pub enum Decision {
    /// Granted without a person; every condition of the policy held.
    Granted { reasons: Vec<String> },
    /// A person decides; the reasons say why it was not automatic.
    NeedsPerson { reasons: Vec<String> },
}

/// Applies the policy. Only path rights of a permission request with
/// reproduced failing-test evidence, outside sensitive components and with
/// a small blast radius, are granted automatically.
pub fn decide(req: &Request, policy: &Policy) -> Decision {
    let mut why_not = Vec::new();
    if req.kind != EscalationKind::Permission {
        why_not.push(format!(
            "a {} request always goes to a person",
            serde_json::to_value(req.kind)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_default()
        ));
    }
    if let Some(r) = req
        .scopes
        .iter()
        .find(|r| !matches!(r.kind, Kind::Read | Kind::Write))
    {
        why_not.push(format!(
            "`{r}` is not a path right; network, secrets and refs need a person"
        ));
    }
    if !req.sensitive.is_empty() {
        why_not.push(format!(
            "the paths are in components labeled {}",
            req.sensitive.join(", ")
        ));
    }
    if req.blast_radius > policy.max_blast_radius {
        why_not.push(format!(
            "{} files depend on the paths (automatic grants allow {})",
            req.blast_radius, policy.max_blast_radius
        ));
    }
    let tests: Vec<&Evidence> = req.evidence.iter().filter(|e| e.grade == 1).collect();
    if tests.is_empty() {
        why_not.push(match req.best_grade() {
            Some(g) => format!("the strongest evidence is grade {g}; automatic grants need a failing test (grade 1)"),
            None => "there is no evidence".into(),
        });
    } else if !tests.iter().any(|e| e.reproduced == Some(true)) {
        why_not.push("the failing test was not reproduced by the test runner".into());
    }
    if why_not.is_empty() {
        Decision::Granted {
            reasons: vec![format!(
                "permission for path rights outside sensitive components, {} dependent files, with a reproduced failing test",
                req.blast_radius
            )],
        }
    } else {
        Decision::NeedsPerson { reasons: why_not }
    }
}

/// The token for a granted request: the original token's rights plus what
/// was asked, for the same task and until the same expiry. It names the
/// request, so it proves this request was granted and no other.
pub fn grant(req: &Request, original: &Verified, root: &RootKey) -> Result<String> {
    if req.task != original.task {
        bail!(
            "the request is for task `{}`, the token for `{}`",
            req.task,
            original.task
        );
    }
    let mut rights = original.rights.clone();
    rights.extend(req.scopes.iter().cloned());
    rights.sort();
    rights.dedup();
    mint_granted(
        root,
        &Grant {
            task: original.task.clone(),
            rights,
            expires: original.expires,
            // The grant is for the same agent.
            agent: original.agent.clone(),
        },
        &req.id,
    )
}

/// Checks that `granted` is the grant of `req`: the request is as it was
/// filed, and the token verifies with the root key, was minted for this
/// request, is for the request's task and holds every right asked for. The
/// errors say what is wrong.
pub fn verify_granted(req: &Request, granted: &str, root: &PublicKey) -> Result<Verified, String> {
    if req.content_id() != req.id {
        return Err("the request file was changed after it was filed".into());
    }
    let v = crate::token::verify(granted, root)
        .map_err(|_| "its granted token does not verify with the root key".to_string())?;
    if v.task != req.task {
        return Err(format!("its granted token is for task `{}`", v.task));
    }
    if v.escalation.as_deref() != Some(req.id.as_str()) {
        return Err(
            "its token was not minted to grant it (onus escalation grant mints one)".into(),
        );
    }
    if let Some(r) = req.scopes.iter().find(|r| !v.rights.contains(r)) {
        return Err(format!("its granted token does not hold `{r}`"));
    }
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::token::mint;

    fn req(kind: EscalationKind, scope: &str, evidence: &[&str], reproduced: bool) -> Request {
        let mut ev: Vec<Evidence> = evidence
            .iter()
            .map(|e| Evidence::parse(e).unwrap())
            .collect();
        for e in &mut ev {
            if e.grade == 1 {
                e.reproduced = Some(reproduced);
            }
        }
        Request::new(
            "sms",
            kind,
            vec![scope.parse().unwrap()],
            ev,
            "needed",
            None,
            &[],
            100,
        )
    }

    #[test]
    fn only_reproduced_low_risk_permission_requests_are_automatic() {
        let p = Policy::default();
        let ok = req(
            EscalationKind::Permission,
            "write:path:services/billing/src/**",
            &["failing-test:services/billing/src/a.test.ts"],
            true,
        );
        assert!(matches!(decide(&ok, &p), Decision::Granted { .. }));
        let not_run = req(
            EscalationKind::Permission,
            "write:path:x/**",
            &["failing-test:x/a.test.ts"],
            false,
        );
        assert!(matches!(decide(&not_run, &p), Decision::NeedsPerson { .. }));
        let weak = req(
            EscalationKind::Permission,
            "write:path:x/**",
            &["rationale:trust me"],
            true,
        );
        let Decision::NeedsPerson { reasons } = decide(&weak, &p) else {
            panic!()
        };
        assert!(reasons[0].contains("grade 5"));
        let net = req(
            EscalationKind::Permission,
            "net:host:evil.example",
            &["failing-test:a.test.ts"],
            true,
        );
        assert!(matches!(decide(&net, &p), Decision::NeedsPerson { .. }));
        let mut sensitive = ok.clone();
        sensitive.sensitive = vec!["payments".into()];
        assert!(matches!(
            decide(&sensitive, &p),
            Decision::NeedsPerson { .. }
        ));
        let mut wide = ok.clone();
        wide.blast_radius = 50;
        assert!(matches!(decide(&wide, &p), Decision::NeedsPerson { .. }));
        let other = req(
            EscalationKind::BrokenTest,
            "write:path:x/**",
            &["failing-test:x/a.test.ts"],
            true,
        );
        assert!(matches!(decide(&other, &p), Decision::NeedsPerson { .. }));
        assert!(Evidence::parse("hunch:x").is_err());
    }

    #[test]
    fn a_grant_adds_exactly_what_was_asked() {
        let root = RootKey::generate();
        let token = mint(
            &root,
            &Grant {
                task: "sms".into(),
                rights: vec!["write:path:services/notifications/**".parse().unwrap()],
                expires: 2_000_000_000,
                agent: None,
            },
        )
        .unwrap();
        let v = crate::token::verify(&token, &root.public()).unwrap();
        let r = req(
            EscalationKind::Permission,
            "write:path:services/billing/src/**",
            &["failing-test:t.ts"],
            true,
        );
        let granted = grant(&r, &v, &root).unwrap();
        let g = crate::token::verify(&granted, &root.public()).unwrap();
        assert_eq!(g.expires, 2_000_000_000);
        assert!(
            g.authorize(Kind::Write, "services/billing/src/a.ts", 1_900_000_000)
                .is_ok()
        );
        assert!(
            g.authorize(Kind::Write, "services/billing/b.ts", 1_900_000_000)
                .is_err()
        );
        assert!(
            g.authorize(Kind::Write, "services/notifications/a.ts", 1_900_000_000)
                .is_ok()
        );
    }

    #[test]
    fn granted_requests_verify_and_tampering_is_named() {
        let root = RootKey::generate();
        let original = |task: &str, root: &RootKey| {
            let t = mint(
                root,
                &Grant {
                    task: task.into(),
                    rights: vec!["write:path:services/notifications/**".parse().unwrap()],
                    expires: 2_000_000_000,
                    agent: Some(crate::token::AgentClaim {
                        tool: "claude-code".into(),
                        ..Default::default()
                    }),
                },
            )
            .unwrap();
            crate::token::verify(&t, &root.public()).unwrap()
        };
        let r = req(
            EscalationKind::Permission,
            "write:path:services/billing/src/**",
            &["failing-test:t.ts"],
            true,
        );
        assert_eq!(r.content_id(), r.id);
        let granted = grant(&r, &original("sms", &root), &root).unwrap();
        let v = verify_granted(&r, &granted, &root.public()).unwrap();
        assert_eq!(v.task, "sms");
        // The grant keeps the agent.
        assert_eq!(v.agent.unwrap().tool, "claude-code");

        // A scope edited in the request file.
        let mut edited = r.clone();
        edited.scopes = vec!["write:path:**".parse().unwrap()];
        assert_eq!(
            verify_granted(&edited, &granted, &root.public()).unwrap_err(),
            "the request file was changed after it was filed"
        );
        // Another task's token in its place.
        let mut other_req = r.clone();
        other_req.task = "other".into();
        other_req.id = other_req.content_id();
        let other = grant(&other_req, &original("other", &root), &root).unwrap();
        assert_eq!(
            verify_granted(&r, &other, &root.public()).unwrap_err(),
            "its granted token is for task `other`"
        );
        // A token signed by another key.
        let stranger = RootKey::generate();
        let forged = grant(&r, &original("sms", &stranger), &stranger).unwrap();
        assert_eq!(
            verify_granted(&r, &forged, &root.public()).unwrap_err(),
            "its granted token does not verify with the root key"
        );
        // The task's own token, copied in beside a request it filed itself:
        // signed by the root key, but not a grant of that request.
        let own = mint(
            &root,
            &Grant {
                task: "sms".into(),
                rights: vec!["write:path:services/billing/src/**".parse().unwrap()],
                expires: 2_000_000_000,
                agent: None,
            },
        )
        .unwrap();
        assert!(
            verify_granted(&r, &own, &root.public())
                .unwrap_err()
                .contains("was not minted to grant it")
        );
        // Another request's grant.
        let mut wider = r.clone();
        wider.scopes.push("net:host:sms.example".parse().unwrap());
        wider.id = wider.content_id();
        assert!(
            verify_granted(&wider, &granted, &root.public())
                .unwrap_err()
                .contains("was not minted to grant it")
        );
        // A token for the request that lacks what was asked.
        let short = crate::token::mint_granted(
            &root,
            &Grant {
                task: "sms".into(),
                rights: r.scopes.clone(),
                expires: 2_000_000_000,
                agent: None,
            },
            &wider.id,
        )
        .unwrap();
        assert_eq!(
            verify_granted(&wider, &short, &root.public()).unwrap_err(),
            "its granted token does not hold `net:host:sms.example`"
        );
    }
}
