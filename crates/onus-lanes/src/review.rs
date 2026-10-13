//! The human lane's review: which GitHub reviews count, whether the lane
//! is satisfied, and whom to ask. The host only fetches the reviews
//! (`onus ci pr --reviews`); every decision about them is made here.

use std::collections::BTreeMap;

use anyhow::{Context, Result};
use onus_core::{ApprovalRule, Lane};
use serde::{Deserialize, Serialize};

/// Associations with write access to the repository.
pub const WRITE_ACCESS: &[&str] = &["OWNER", "MEMBER", "COLLABORATOR"];

/// One GitHub review, as the host passes it (`--reviews`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Review {
    /// The reviewer's login, without `@`.
    pub by: String,
    /// `APPROVED`, `CHANGES_REQUESTED`, `COMMENTED`, `DISMISSED` or
    /// `PENDING`, in any case.
    pub state: String,
    /// The commit reviewed.
    pub commit: String,
    /// `OWNER`, `MEMBER`, `COLLABORATOR`, `CONTRIBUTOR`, …
    pub association: String,
    #[serde(default)]
    pub bot: bool,
    /// Seconds since 1970; reviews at the same second keep their order.
    #[serde(default)]
    pub at: u64,
}

/// The members of owning teams, as the host knows them: `@org/team` →
/// logins, with or without `@`.
pub type Teams = BTreeMap<String, Vec<String>>;

/// What a `--reviews` file holds: a JSON array of reviews, or
/// `{"reviews": [...], "teams": {...}}` with the members of owning teams.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewsFile {
    pub reviews: Vec<Review>,
    #[serde(default)]
    pub teams: Teams,
}

impl ReviewsFile {
    /// Reads either shape.
    pub fn parse(text: &str) -> Result<ReviewsFile> {
        let value: serde_json::Value = serde_json::from_str(text)?;
        if value.is_array() {
            return Ok(ReviewsFile {
                reviews: serde_json::from_value(value)?,
                teams: Teams::new(),
            });
        }
        serde_json::from_value(value)
            .context("expected a list of reviews, or {\"reviews\": [...], \"teams\": {...}}")
    }
}

/// Whose approval counts: `lanes.approval` (none when not written) and
/// the members of owning teams.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ApprovalPolicy {
    pub rule: Option<ApprovalRule>,
    pub teams: Teams,
}

/// A review that was not counted, and why.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Ignored {
    pub by: String,
    pub why: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewState {
    /// The change is in the human lane, so a person must approve it.
    pub required: bool,
    /// Nothing more is needed: not required, or approved with no changes
    /// requested.
    pub satisfied: bool,
    /// Counted approvals of the head commit (`@login`), in every lane.
    pub approved_by: Vec<String>,
    /// Counted reviewers whose latest review requests changes, at any commit.
    pub changes_requested_by: Vec<String>,
    /// Whom the change is routed to.
    pub owners: Vec<String>,
    /// Owners to ask now: not asked before and not the author. Empty unless
    /// a review is required and missing.
    pub request: Vec<String>,
    pub ignored: Vec<Ignored>,
    /// Whose approval counted: `owner` or `member` (anyone with write
    /// access).
    pub approval: ApprovalRule,
    /// Why approvals from anyone with write access counted although
    /// `lanes.approval` asks for an owner.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

fn login(s: &str) -> String {
    s.trim().trim_start_matches('@').to_string()
}

fn norm(s: &str) -> String {
    login(s).to_lowercase()
}

/// An owner Onus can match a login against without being told more: a
/// person (`@login`), not a team (`@org/team`) or an email.
fn is_person(owner: &str) -> bool {
    owner.trim().starts_with('@') && !login(owner).contains(['/', '@'])
}

/// The rule that applies, and why it is `member` when `owner` was asked
/// for: no owners, teams whose members are not known, or no owner but the
/// author who could approve.
fn resolve(
    policy: &ApprovalPolicy,
    owners: &[String],
    author: &str,
) -> (ApprovalRule, Option<String>) {
    let asked = policy.rule.unwrap_or(if owners.is_empty() {
        ApprovalRule::Member
    } else {
        ApprovalRule::Owner
    });
    if asked == ApprovalRule::Member {
        return (ApprovalRule::Member, None);
    }
    let fallback = |why: String| {
        (
            ApprovalRule::Member,
            Some(format!(
                "{why}, so an approval from any person with write access counts"
            )),
        )
    };
    if owners.is_empty() {
        return fallback("Nobody owns what changed".into());
    }
    // A team the host lists is known, even with nobody in it; one it does
    // not list is not.
    let members_of = |owner: &str| {
        policy
            .teams
            .iter()
            .find(|(t, _)| norm(t) == norm(owner))
            .map(|(_, members)| members)
    };
    let unknown: Vec<&str> = owners
        .iter()
        .filter(|o| !is_person(o) && members_of(o).is_none())
        .map(String::as_str)
        .collect();
    if !unknown.is_empty() {
        return fallback(format!(
            "The members of {} are not known",
            unknown.join(", ")
        ));
    }
    // Someone other than the author must be able to approve, or the change
    // would wait forever.
    let others = owners.iter().any(|o| match members_of(o) {
        Some(members) => members.iter().any(|m| norm(m) != author),
        None => norm(o) != author,
    });
    if !others {
        return fallback(if owners.iter().all(|o| is_person(o)) {
            format!("The only owner, {}, opened it", owners.join(", "))
        } else {
            format!(
                "No owner but the author can approve ({})",
                owners.join(", ")
            )
        });
    }
    (ApprovalRule::Owner, None)
}

/// Whether `reviewer` is one of `owners`, or a member of an owning team.
fn owns(reviewer: &str, owners: &[String], teams: &Teams) -> bool {
    owners.iter().any(|o| {
        let o = norm(o);
        o == reviewer
            || teams
                .iter()
                .filter(|(t, _)| norm(t) == o)
                .any(|(_, members)| members.iter().any(|m| norm(m) == reviewer))
    })
}

/// Which reviews count for a change at `head` opened by `author`, and
/// whom of `owners` to ask that was not asked before. With the `owner`
/// rule, only owners' approvals count in the human lane; changes requested
/// by anyone with write access still stand. Other lanes need no approval,
/// so every approval with write access is kept there, for provenance.
pub fn evaluate(
    lane: Lane,
    head: &str,
    author: &str,
    reviews: &[Review],
    owners: Vec<String>,
    asked_before: &[String],
    policy: &ApprovalPolicy,
) -> ReviewState {
    let author = norm(author);
    let (rule, note) = match lane {
        Lane::Human => resolve(policy, &owners, &author),
        _ => (ApprovalRule::Member, None),
    };
    // Each reviewer's latest verdict; comments never override one.
    let mut ordered: Vec<&Review> = reviews.iter().collect();
    ordered.sort_by_key(|r| r.at);
    let mut latest: BTreeMap<String, &Review> = BTreeMap::new();
    for r in ordered {
        let key = login(&r.by).to_lowercase();
        match r.state.to_uppercase().as_str() {
            "APPROVED" | "CHANGES_REQUESTED" => {
                latest.insert(key, r);
            }
            "DISMISSED" => {
                latest.remove(&key);
            }
            _ => {}
        }
    }
    let mut state = ReviewState {
        required: lane == Lane::Human,
        owners,
        approval: rule,
        note,
        ..Default::default()
    };
    for (key, r) in latest {
        let by = format!("@{}", login(&r.by));
        let approved = r.state.eq_ignore_ascii_case("APPROVED");
        let association = r.association.to_uppercase();
        let why = if r.bot || key.ends_with("[bot]") {
            Some("a bot".to_string())
        } else if !author.is_empty() && key == author {
            Some("opened it".to_string())
        } else if !WRITE_ACCESS.contains(&association.as_str()) {
            Some(format!("no write access ({association})"))
        } else if approved && r.commit != head {
            Some("approved an earlier commit".to_string())
        } else if approved
            && rule == ApprovalRule::Owner
            && !owns(&key, &state.owners, &policy.teams)
        {
            Some("not an owner".to_string())
        } else {
            None
        };
        match why {
            Some(why) => state.ignored.push(Ignored { by, why }),
            None if approved => state.approved_by.push(by),
            None => state.changes_requested_by.push(by),
        }
    }
    state.satisfied =
        !state.required || (!state.approved_by.is_empty() && state.changes_requested_by.is_empty());
    if state.required && !state.satisfied {
        let asked: Vec<String> = asked_before.iter().map(|a| a.to_lowercase()).collect();
        state.request = state
            .owners
            .iter()
            .filter(|o| !asked.contains(&o.to_lowercase()))
            .filter(|o| author.is_empty() || login(o).to_lowercase() != author)
            .cloned()
            .collect();
    }
    state
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEAD: &str = "h1";

    fn review(by: &str, state: &str, at: u64) -> Review {
        Review {
            by: by.into(),
            state: state.into(),
            commit: HEAD.into(),
            association: "MEMBER".into(),
            bot: false,
            at,
        }
    }

    /// Anyone with write access counts: the rules every review follows.
    fn member() -> ApprovalPolicy {
        ApprovalPolicy {
            rule: Some(ApprovalRule::Member),
            ..Default::default()
        }
    }

    fn eval(reviews: &[Review]) -> ReviewState {
        evaluate(
            Lane::Human,
            HEAD,
            "mike",
            reviews,
            vec!["@team-growth".into()],
            &[],
            &member(),
        )
    }

    fn ignored(s: &ReviewState) -> Vec<(&str, &str)> {
        s.ignored
            .iter()
            .map(|i| (i.by.as_str(), i.why.as_str()))
            .collect()
    }

    #[test]
    fn nobody_has_reviewed_so_the_owners_are_asked() {
        let s = eval(&[]);
        assert!(s.required && !s.satisfied);
        assert_eq!(s.request, ["@team-growth"]);
        assert!(s.approved_by.is_empty() && s.ignored.is_empty());
    }

    #[test]
    fn the_authors_approval_is_not_counted() {
        let s = eval(&[review("Mike", "APPROVED", 1)]);
        assert!(!s.satisfied);
        assert_eq!(ignored(&s), [("@Mike", "opened it")]);
    }

    #[test]
    fn bots_are_not_counted() {
        let mut flagged = review("helper", "APPROVED", 1);
        flagged.bot = true;
        let s = eval(&[flagged, review("renovate[bot]", "approved", 2)]);
        assert!(!s.satisfied);
        assert_eq!(
            ignored(&s),
            [("@helper", "a bot"), ("@renovate[bot]", "a bot")]
        );
    }

    #[test]
    fn a_contributor_has_no_write_access() {
        let mut eve = review("eve", "APPROVED", 1);
        eve.association = "CONTRIBUTOR".into();
        let s = eval(&[eve]);
        assert!(!s.satisfied);
        assert_eq!(ignored(&s), [("@eve", "no write access (CONTRIBUTOR)")]);
    }

    #[test]
    fn an_approval_of_an_earlier_commit_is_not_counted() {
        let mut grace = review("grace", "APPROVED", 1);
        grace.commit = "h0".into();
        let s = eval(&[grace]);
        assert!(!s.satisfied);
        assert_eq!(ignored(&s), [("@grace", "approved an earlier commit")]);
        // Changes requested on an earlier commit still stand, as on GitHub.
        let mut x = review("x", "CHANGES_REQUESTED", 2);
        x.commit = "h0".into();
        let s = eval(&[review("grace", "APPROVED", 1), x]);
        assert!(!s.satisfied);
        assert_eq!(s.changes_requested_by, ["@x"]);
    }

    #[test]
    fn each_reviewers_latest_review_counts() {
        let s = eval(&[
            review("grace", "APPROVED", 1),
            review("grace", "CHANGES_REQUESTED", 2),
        ]);
        assert!(!s.satisfied);
        assert_eq!(s.changes_requested_by, ["@grace"]);
        assert!(s.approved_by.is_empty());
        // Ordered by time, not by position.
        let s = eval(&[
            review("grace", "APPROVED", 3),
            review("grace", "CHANGES_REQUESTED", 2),
        ]);
        assert!(s.satisfied);
        assert_eq!(s.approved_by, ["@grace"]);
        // The same second keeps input order.
        let s = eval(&[
            review("grace", "CHANGES_REQUESTED", 2),
            review("grace", "APPROVED", 2),
        ]);
        assert!(s.satisfied);
        assert!(s.request.is_empty());
    }

    #[test]
    fn a_comment_keeps_an_approval_and_a_dismissal_clears_it() {
        let s = eval(&[
            review("grace", "APPROVED", 1),
            review("grace", "COMMENTED", 2),
            review("grace", "PENDING", 3),
        ]);
        assert!(s.satisfied);
        assert_eq!(s.approved_by, ["@grace"]);
        let s = eval(&[
            review("grace", "APPROVED", 1),
            review("grace", "DISMISSED", 2),
        ]);
        assert!(!s.satisfied);
        assert!(s.approved_by.is_empty() && s.ignored.is_empty());
    }

    #[test]
    fn outside_the_human_lane_approvals_are_kept_for_provenance() {
        let s = evaluate(
            Lane::Judge,
            HEAD,
            "mike",
            &[review("grace", "APPROVED", 1)],
            vec!["@team-growth".into()],
            &[],
            &member(),
        );
        assert!(!s.required && s.satisfied);
        assert!(s.request.is_empty());
        assert_eq!(s.approved_by, ["@grace"]);
        let s = evaluate(Lane::AutoMerge, HEAD, "mike", &[], vec![], &[], &member());
        assert!(!s.required && s.satisfied && s.request.is_empty());
    }

    #[test]
    fn nobody_is_asked_twice_and_the_author_never() {
        let owners: Vec<String> = ["@team-growth", "@Mike", "@grace"]
            .map(String::from)
            .to_vec();
        let s = evaluate(
            Lane::Human,
            HEAD,
            "mike",
            &[],
            owners.clone(),
            &["@grace".into()],
            &member(),
        );
        assert_eq!(s.request, ["@team-growth"]);
        assert_eq!(s.owners, owners);
        let s = evaluate(
            Lane::Human,
            HEAD,
            "mike",
            &[review("grace", "APPROVED", 1)],
            owners,
            &[],
            &member(),
        );
        assert!(s.satisfied);
        assert!(s.request.is_empty());
    }

    #[test]
    fn reviews_parse_from_what_a_host_writes() {
        let json = r#"[{"by":"grace","state":"APPROVED","commit":"h1","association":"MEMBER","bot":false,"at":17},
                       {"by":"eve","state":"COMMENTED","commit":"h1","association":"NONE"}]"#;
        let reviews: Vec<Review> = serde_json::from_str(json).unwrap();
        assert_eq!(reviews[1].at, 0);
        assert_eq!(eval(&reviews).approved_by, ["@grace"]);
        let out = serde_json::to_value(eval(&reviews)).unwrap();
        assert_eq!(out["approvedBy"], serde_json::json!(["@grace"]));
        assert_eq!(out["changesRequestedBy"], serde_json::json!([]));
    }

    fn with(owners: &[&str], policy: &ApprovalPolicy, reviews: &[Review]) -> ReviewState {
        evaluate(
            Lane::Human,
            HEAD,
            "mike",
            reviews,
            owners.iter().map(|o| o.to_string()).collect(),
            &[],
            policy,
        )
    }

    fn teams(list: &[(&str, &[&str])]) -> Teams {
        list.iter()
            .map(|(t, m)| (t.to_string(), m.iter().map(|m| m.to_string()).collect()))
            .collect()
    }

    #[test]
    fn owners_approve_by_default_when_there_are_owners() {
        let policy = ApprovalPolicy::default();
        let s = with(&["@grace"], &policy, &[review("ada", "APPROVED", 1)]);
        assert_eq!(s.approval, ApprovalRule::Owner);
        assert!(!s.satisfied && s.note.is_none());
        assert_eq!(ignored(&s), [("@ada", "not an owner")]);
        let s = with(&["@Grace"], &policy, &[review("grace", "APPROVED", 1)]);
        assert!(s.satisfied);
        assert_eq!(s.approved_by, ["@grace"]);
        // A person who is not an owner can still hold a change back.
        let s = with(
            &["@grace"],
            &policy,
            &[
                review("grace", "APPROVED", 1),
                review("ada", "CHANGES_REQUESTED", 2),
            ],
        );
        assert!(!s.satisfied);
        assert_eq!(s.changes_requested_by, ["@ada"]);
        // Nobody owns it: anyone with write access, and nothing to explain.
        let s = with(&[], &policy, &[review("ada", "APPROVED", 1)]);
        assert_eq!(s.approval, ApprovalRule::Member);
        assert!(s.satisfied && s.note.is_none());
    }

    #[test]
    fn members_of_an_owning_team_approve_when_the_host_says_who_they_are() {
        let policy = ApprovalPolicy {
            rule: None,
            teams: teams(&[("acme/growth", &["@Grace", "lin"])]),
        };
        let s = with(
            &["@acme/growth"],
            &policy,
            &[review("grace", "APPROVED", 1), review("ada", "APPROVED", 2)],
        );
        assert_eq!(s.approval, ApprovalRule::Owner);
        assert!(s.satisfied && s.note.is_none());
        assert_eq!(s.approved_by, ["@grace"]);
        assert_eq!(ignored(&s), [("@ada", "not an owner")]);
    }

    #[test]
    fn owners_that_cannot_be_checked_fall_back_to_write_access_and_say_so() {
        let owner = ApprovalPolicy {
            rule: Some(ApprovalRule::Owner),
            ..Default::default()
        };
        // A team whose members the reviews do not carry.
        let s = with(
            &["@acme/growth", "@lin"],
            &ApprovalPolicy::default(),
            &[review("ada", "APPROVED", 1)],
        );
        assert_eq!(s.approval, ApprovalRule::Member);
        assert!(s.satisfied);
        assert_eq!(
            s.note.as_deref(),
            Some(
                "The members of @acme/growth are not known, so an approval from any person with write access counts"
            )
        );
        // An email.
        let s = with(&["lin@acme.dev"], &owner, &[review("ada", "APPROVED", 1)]);
        assert!(s.satisfied && s.note.unwrap().contains("lin@acme.dev"));
        // Nobody owns it.
        let s = with(&[], &owner, &[review("ada", "APPROVED", 1)]);
        assert!(s.satisfied);
        assert!(s.note.unwrap().starts_with("Nobody owns what changed"));
        // The only owner opened it.
        let s = with(&["@mike"], &owner, &[review("ada", "APPROVED", 1)]);
        assert!(s.satisfied);
        assert!(
            s.note
                .unwrap()
                .starts_with("The only owner, @mike, opened it")
        );
    }

    #[test]
    fn a_team_listed_with_nobody_in_it_has_no_one_to_approve() {
        let policy = ApprovalPolicy {
            rule: None,
            teams: teams(&[("@acme/growth", &[]), ("@acme/core", &["mike"])]),
        };
        // Beside another owner, the empty team adds nobody.
        let s = with(
            &["@acme/growth", "@grace"],
            &policy,
            &[review("ada", "APPROVED", 1)],
        );
        assert_eq!(s.approval, ApprovalRule::Owner);
        assert!(!s.satisfied && s.note.is_none());
        assert_eq!(ignored(&s), [("@ada", "not an owner")]);
        // Alone, or with only the author in the teams, nobody could approve:
        // write access counts, and the review says so.
        for owners in [
            &["@acme/growth"][..],
            &["@acme/growth", "@acme/core", "@mike"],
        ] {
            let s = with(owners, &policy, &[review("ada", "APPROVED", 1)]);
            assert_eq!(s.approval, ApprovalRule::Member, "{owners:?}");
            assert!(s.satisfied);
            assert!(
                s.note
                    .unwrap()
                    .starts_with("No owner but the author can approve")
            );
        }
    }

    #[test]
    fn outside_the_human_lane_every_approval_is_kept_for_provenance() {
        let s = evaluate(
            Lane::Judge,
            HEAD,
            "mike",
            &[review("ada", "APPROVED", 1)],
            vec!["@grace".into()],
            &[],
            &ApprovalPolicy {
                rule: Some(ApprovalRule::Owner),
                ..Default::default()
            },
        );
        assert!(!s.required && s.satisfied);
        assert_eq!(s.approved_by, ["@ada"]);
        assert!(s.ignored.is_empty() && s.note.is_none());
        assert_eq!(s.approval, ApprovalRule::Member);
    }

    #[test]
    fn member_counts_anyone_with_write_access() {
        let s = with(&["@grace"], &member(), &[review("ada", "APPROVED", 1)]);
        assert_eq!(s.approval, ApprovalRule::Member);
        assert!(s.satisfied && s.note.is_none());
        let out = serde_json::to_value(&s).unwrap();
        assert_eq!(out["approval"], "member");
        assert!(out.get("note").is_none());
    }

    #[test]
    fn a_reviews_file_is_a_list_or_carries_teams() {
        let list = ReviewsFile::parse(
            r#"[{"by":"grace","state":"APPROVED","commit":"h1","association":"MEMBER"}]"#,
        )
        .unwrap();
        assert_eq!(list.reviews.len(), 1);
        assert!(list.teams.is_empty());
        let both =
            ReviewsFile::parse(r#"{"reviews":[],"teams":{"@acme/growth":["grace"]}}"#).unwrap();
        assert_eq!(both.teams["@acme/growth"], ["grace"]);
        assert!(ReviewsFile::parse(r#"{"reviewz":[]}"#).is_err());
    }
}
