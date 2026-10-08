//! Rights and scopes: what a task may read, write, push, reach and use.
//!
//! A right is written `<kind>:<target>`:
//!
//! - `read:path:<glob>` and `write:path:<glob>`: repository paths;
//! - `push:ref:<glob>`: git refs the gateway accepts (`refs/heads/onus/task-1/*`);
//! - `net:host:<host>`: a network host the task's environment may reach;
//! - `secret:<NAME>`: a secret the task's environment receives;
//! - `guard:path:<glob>`: paths that need a granted escalation before they
//!   change, even when they are writable (a declared contract's file);
//! - `unlock:path:<glob>`: guarded paths an escalation unlocked.
//!
//! Path globs use `*` (within one folder), `**` (any depth) and `?`. The
//! same glob always means the same set, whether Onus checks it in Rust or a
//! token checks it in Datalog: both use the regex from [`glob_regex`].

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// The kind of thing a right is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    Read,
    Write,
    Push,
    Net,
    Secret,
    Guard,
    Unlock,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Read => "read",
            Kind::Write => "write",
            Kind::Push => "push",
            Kind::Net => "net",
            Kind::Secret => "secret",
            Kind::Guard => "guard",
            Kind::Unlock => "unlock",
        }
    }

    pub fn parse(s: &str) -> Option<Kind> {
        Some(match s {
            "read" => Kind::Read,
            "write" => Kind::Write,
            "push" => Kind::Push,
            "net" => Kind::Net,
            "secret" => Kind::Secret,
            "guard" => Kind::Guard,
            "unlock" => Kind::Unlock,
            _ => return None,
        })
    }

    /// The word between the kind and the pattern in the written form.
    fn target(self) -> Option<&'static str> {
        match self {
            Kind::Read | Kind::Write | Kind::Guard | Kind::Unlock => Some("path"),
            Kind::Push => Some("ref"),
            Kind::Net => Some("host"),
            Kind::Secret => None,
        }
    }
}

/// One right: a kind and a pattern (a glob for paths and refs, a host, or a
/// secret name).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Right {
    pub kind: Kind,
    pub pattern: String,
}

impl Right {
    pub fn new(kind: Kind, pattern: impl Into<String>) -> Right {
        Right {
            kind,
            pattern: pattern.into(),
        }
    }

    /// The anchored regex this right's pattern matches with.
    pub fn regex(&self) -> String {
        match self.kind {
            Kind::Read | Kind::Write | Kind::Push | Kind::Guard | Kind::Unlock => {
                glob_regex(&self.pattern)
            }
            Kind::Net | Kind::Secret => format!("^{}$", regex::escape(&self.pattern)),
        }
    }

    pub fn allows(&self, kind: Kind, value: &str) -> bool {
        self.kind == kind
            && regex::Regex::new(&self.regex())
                .map(|re| re.is_match(value))
                .unwrap_or(false)
    }
}

impl fmt::Display for Right {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind.target() {
            Some(t) => write!(f, "{}:{t}:{}", self.kind.as_str(), self.pattern),
            None => write!(f, "{}:{}", self.kind.as_str(), self.pattern),
        }
    }
}

impl FromStr for Right {
    type Err = String;

    fn from_str(s: &str) -> Result<Right, String> {
        let bad = || {
            format!(
                "`{s}` is not a right; write `read:path:<glob>`, `write:path:<glob>`, \
                 `push:ref:<glob>`, `net:host:<host>`, `secret:<NAME>`, `guard:path:<glob>` \
                 or `unlock:path:<glob>`"
            )
        };
        let (kind, rest) = s.split_once(':').ok_or_else(bad)?;
        let kind = Kind::parse(kind).ok_or_else(bad)?;
        let pattern = match kind.target() {
            Some(t) => rest
                .strip_prefix(t)
                .and_then(|r| r.strip_prefix(':'))
                .ok_or_else(bad)?,
            None => rest,
        };
        if pattern.is_empty() {
            return Err(bad());
        }
        if matches!(kind, Kind::Read | Kind::Write | Kind::Guard | Kind::Unlock) {
            check_path_glob(pattern)?;
        }
        Ok(Right::new(kind, pattern))
    }
}

impl Serialize for Right {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Right {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Right, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

/// Path globs are relative to the repository and cannot climb out of it.
fn check_path_glob(glob: &str) -> Result<(), String> {
    if glob.starts_with('/') || glob.split('/').any(|p| p == ".." || p == ".") {
        return Err(format!(
            "`{glob}` must be a path inside the repository, without `/`, `.` or `..` parts"
        ));
    }
    Ok(())
}

/// The anchored regex of a glob: `**/` any folders (or none), `**` anything,
/// `*` anything within one folder, `?` one character within one folder.
pub fn glob_regex(glob: &str) -> String {
    let mut out = String::from("^");
    let chars: Vec<char> = glob.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '*' if chars.get(i + 1) == Some(&'*') => {
                if chars.get(i + 2) == Some(&'/') {
                    out.push_str("(?:.*/)?");
                    i += 3;
                } else {
                    out.push_str(".*");
                    i += 2;
                }
                continue;
            }
            '*' => out.push_str("[^/]*"),
            '?' => out.push_str("[^/]"),
            c => out.push_str(&regex::escape(&c.to_string())),
        }
        i += 1;
    }
    out.push('$');
    out
}

/// A set of rights, checked in Rust (doors use this after a token has been
/// verified; the token checks the same patterns itself).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Scope {
    pub rights: Vec<Right>,
}

impl Scope {
    pub fn new(mut rights: Vec<Right>) -> Scope {
        rights.sort();
        rights.dedup();
        Scope { rights }
    }

    pub fn allows(&self, kind: Kind, value: &str) -> bool {
        self.rights.iter().any(|r| r.allows(kind, value))
    }

    pub fn can_read(&self, path: &str) -> bool {
        // What a task may write it may also read.
        self.allows(Kind::Read, path) || self.allows(Kind::Write, path)
    }

    pub fn can_write(&self, path: &str) -> bool {
        self.allows(Kind::Write, path)
    }

    /// Whether changing `path` waits for a granted escalation: it is guarded
    /// and not unlocked.
    pub fn guarded(&self, path: &str) -> bool {
        self.allows(Kind::Guard, path) && !self.allows(Kind::Unlock, path)
    }

    pub fn of(&self, kind: Kind) -> impl Iterator<Item = &Right> {
        self.rights.iter().filter(move |r| r.kind == kind)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rights_round_trip_and_reject_bad_forms() {
        for s in [
            "read:path:services/**",
            "write:path:services/notifications/src/*.ts",
            "push:ref:refs/heads/onus/task-1/*",
            "net:host:sms.test.example",
            "secret:SMS_TEST_KEY",
        ] {
            assert_eq!(s.parse::<Right>().unwrap().to_string(), s);
        }
        for s in [
            "write:services/**",
            "delete:path:x",
            "write:path:",
            "write:path:../etc/**",
            "read:path:/etc/passwd",
            "write:path:a/./b",
        ] {
            assert!(s.parse::<Right>().is_err(), "{s}");
        }
    }

    #[test]
    fn globs_match_paths() {
        let r = |s: &str| s.parse::<Right>().unwrap();
        let notif = r("write:path:services/notifications/**");
        assert!(notif.allows(Kind::Write, "services/notifications/src/sms.ts"));
        assert!(!notif.allows(Kind::Write, "services/notifications"));
        assert!(!notif.allows(Kind::Write, "services/notifications-evil/a.ts"));
        assert!(!notif.allows(Kind::Read, "services/notifications/src/sms.ts"));
        let ts = r("write:path:src/*.ts");
        assert!(ts.allows(Kind::Write, "src/a.ts"));
        assert!(!ts.allows(Kind::Write, "src/deep/a.ts"));
        let any = r("read:path:**/*.md");
        assert!(any.allows(Kind::Read, "README.md"));
        assert!(any.allows(Kind::Read, "docs/a/b.md"));
        // Case matters, as in git.
        assert!(!notif.allows(Kind::Write, "Services/Notifications/a.ts"));
        let host = r("net:host:sms.test.example");
        assert!(host.allows(Kind::Net, "sms.test.example"));
        assert!(!host.allows(Kind::Net, "smsXtest.example"));
        let scope = Scope::new(vec![notif.clone()]);
        assert!(scope.can_read("services/notifications/a.ts"));
        assert!(!scope.can_read("services/billing/a.ts"));
    }
}
