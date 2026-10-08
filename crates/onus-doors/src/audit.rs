//! The audit log: one JSON line per door decision, each carrying the hash
//! of the line before it, so that removing or editing a line breaks the
//! chain. `verify` recomputes it.

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// One decision.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub seq: u64,
    /// Seconds since the Unix epoch.
    pub at: u64,
    /// The task (token) that asked, or `operator`.
    pub actor: String,
    /// `mint`, `attenuate`, `push`, `escalate`, `grant`, `deny`, …
    pub action: String,
    /// What was asked about: a ref, a scope, a request id.
    pub subject: String,
    /// `allowed`, `refused`, `granted`, `needs-person`, …
    pub decision: String,
    pub reason: String,
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub details: serde_json::Value,
    /// The hash of the previous entry (64 zeros for the first).
    pub prev: String,
    /// sha256 of this entry's JSON with `hash` empty.
    pub hash: String,
}

/// What a door records; the log adds the sequence, the chain and the hash.
#[derive(Debug, Clone, Default)]
pub struct Record {
    pub actor: String,
    pub action: String,
    pub subject: String,
    pub decision: String,
    pub reason: String,
    pub details: serde_json::Value,
}

const GENESIS: &str = "0000000000000000000000000000000000000000000000000000000000000000";

fn digest(e: &Entry) -> String {
    let mut copy = e.clone();
    copy.hash = String::new();
    let json = serde_json::to_string(&copy).unwrap_or_default();
    let mut h = Sha256::new();
    h.update(json.as_bytes());
    h.finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>()
}

#[derive(Debug, Clone)]
pub struct AuditLog {
    path: PathBuf,
}

impl AuditLog {
    pub fn new(path: impl Into<PathBuf>) -> AuditLog {
        AuditLog { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Appends a decision. Writers on one machine take turns through a lock
    /// file next to the log.
    pub fn append(&self, record: Record, at: u64) -> Result<Entry> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let lock = self.path.with_extension("lock");
        let _guard = LockFile::acquire(&lock)?;
        let (seq, prev) = match self.last()? {
            Some(e) => (e.seq + 1, e.hash),
            None => (1, GENESIS.to_string()),
        };
        let mut entry = Entry {
            seq,
            at,
            actor: record.actor,
            action: record.action,
            subject: record.subject,
            decision: record.decision,
            reason: record.reason,
            details: record.details,
            prev,
            hash: String::new(),
        };
        entry.hash = digest(&entry);
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .with_context(|| format!("cannot open the audit log {}", self.path.display()))?;
        writeln!(f, "{}", serde_json::to_string(&entry)?)?;
        f.sync_data()?;
        Ok(entry)
    }

    fn last(&self) -> Result<Option<Entry>> {
        let Ok(f) = std::fs::File::open(&self.path) else {
            return Ok(None);
        };
        let mut last = None;
        for line in std::io::BufReader::new(f).lines() {
            let line = line?;
            if !line.trim().is_empty() {
                last = Some(line);
            }
        }
        last.map(|l| serde_json::from_str(&l).context("the audit log's last line is not an entry"))
            .transpose()
    }

    /// Every entry, after checking the chain: sequence numbers count up from
    /// 1, each entry names the hash of the one before, and each hash matches.
    pub fn verify(&self) -> Result<Vec<Entry>> {
        let f = std::fs::File::open(&self.path)
            .with_context(|| format!("cannot open the audit log {}", self.path.display()))?;
        let mut out: Vec<Entry> = Vec::new();
        for (i, line) in std::io::BufReader::new(f).lines().enumerate() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let e: Entry = serde_json::from_str(&line)
                .with_context(|| format!("line {} is not an audit entry", i + 1))?;
            let prev = out.last().map_or(GENESIS, |p| p.hash.as_str());
            if e.seq != out.len() as u64 + 1 {
                bail!(
                    "line {}: expected entry {}, found {}",
                    i + 1,
                    out.len() + 1,
                    e.seq
                );
            }
            if e.prev != prev {
                bail!(
                    "line {}: the chain is broken (an entry before it was changed or removed)",
                    i + 1
                );
            }
            if digest(&e) != e.hash {
                bail!("line {}: the entry was changed after it was written", i + 1);
            }
            out.push(e);
        }
        Ok(out)
    }
}

/// A lock held by creating a file exclusively; removed on drop.
struct LockFile(PathBuf);

impl LockFile {
    fn acquire(path: &Path) -> Result<LockFile> {
        for _ in 0..500 {
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
            {
                Ok(_) => return Ok(LockFile(path.to_path_buf())),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    // A lock older than a minute was left by a crash.
                    if let Ok(meta) = std::fs::metadata(path)
                        && meta
                            .modified()
                            .ok()
                            .and_then(|m| m.elapsed().ok())
                            .is_some_and(|age| age.as_secs() > 60)
                    {
                        let _ = std::fs::remove_file(path);
                        continue;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                Err(e) => return Err(e.into()),
            }
        }
        bail!("the audit log is locked ({})", path.display())
    }
}

impl Drop for LockFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(action: &str) -> Record {
        Record {
            actor: "task-1".into(),
            action: action.into(),
            subject: "refs/heads/onus/task-1/x".into(),
            decision: "allowed".into(),
            reason: "in scope".into(),
            details: serde_json::Value::Null,
        }
    }

    #[test]
    fn the_chain_detects_edits_and_removals() {
        let dir = tempfile::tempdir().unwrap();
        let log = AuditLog::new(dir.path().join("audit.jsonl"));
        for a in ["mint", "push", "escalate"] {
            log.append(record(a), 100).unwrap();
        }
        let entries = log.verify().unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[1].prev, entries[0].hash);

        let text = std::fs::read_to_string(log.path()).unwrap();
        // Edit a decision.
        std::fs::write(log.path(), text.replacen("\"allowed\"", "\"granted\"", 1)).unwrap();
        assert!(log.verify().unwrap_err().to_string().contains("changed"));
        // Remove a line.
        let lines: Vec<&str> = text.lines().collect();
        std::fs::write(log.path(), format!("{}\n{}\n", lines[0], lines[2])).unwrap();
        assert!(log.verify().is_err());
    }
}
