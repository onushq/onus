//! The human lane's review: which GitHub reviews count, whether the lane
//! is satisfied, and whom to ask. The host only fetches the reviews
//! (`onus ci pr --reviews`); every decision about them is made here.

use std::collections::BTreeMap;

use onus_core::Lane;
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
}

fn login(s: &str) -> String {
    s.trim().trim_start_matches('@').to_string()
}

/// Which reviews count for a change at `head` opened by `author`, and
/// whom of `owners` to ask that was not asked before.
pub fn evaluate(
    lane: Lane,
    head: &str,
    author: &str,
    reviews: &[Review],
    owners: Vec<String>,
    asked_before: &[String],
) -> ReviewState {
    let author = login(author).to_lowercase();
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

    fn eval(reviews: &[Review]) -> ReviewState {
        evaluate(
            Lane::Human,
            HEAD,
            "mike",
            reviews,
            vec!["@team-growth".into()],
            &[],
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
        );
        assert!(!s.required && s.satisfied);
        assert!(s.request.is_empty());
        assert_eq!(s.approved_by, ["@grace"]);
        let s = evaluate(Lane::AutoMerge, HEAD, "mike", &[], vec![], &[]);
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
}
