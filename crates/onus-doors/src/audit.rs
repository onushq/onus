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
    /// The task (token) that asked, `operator`, or `local` (an MCP server
    /// without a token).
    pub actor: String,
    /// `mint`, `attenuate`, `push`, `fetch`, `auth`, `escalate`, `grant`,
    /// `deny`, `adopt-token`, `mcp:<tool>`, …
    pub action: String,
    /// What was asked about: a ref, a scope, a request id.
    pub subject: String,
    /// `allowed`, `denied`, `done`, `failed`, `refused`, `granted`,
    /// `needs-person`, …
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

    /// The last entry, read from the end of the file in blocks: appending
    /// costs the same however long the log is.
    fn last(&self) -> Result<Option<Entry>> {
        use std::io::{Read, Seek, SeekFrom};
        const BLOCK: u64 = 8 * 1024;
        let Ok(mut f) = std::fs::File::open(&self.path) else {
            return Ok(None);
        };
        let mut pos = f.metadata()?.len();
        let mut tail: Vec<u8> = Vec::new();
        loop {
            let end = tail
                .iter()
                .rposition(|b| !b.is_ascii_whitespace())
                .map(|i| i + 1);
            if let Some(end) = end {
                let start = tail[..end].iter().rposition(|b| *b == b'\n');
                if start.is_some() || pos == 0 {
                    let line = &tail[start.map_or(0, |i| i + 1)..end];
                    let entry = serde_json::from_slice(line)
                        .context("the audit log's last line is not an entry")?;
                    return Ok(Some(entry));
                }
            } else if pos == 0 {
                return Ok(None);
            }
            let from = pos.saturating_sub(BLOCK);
            let mut block = vec![0u8; (pos - from) as usize];
            f.seek(SeekFrom::Start(from))?;
            f.read_exact(&mut block)?;
            block.extend_from_slice(&tail);
            tail = block;
            pos = from;
        }
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

/// A digest of a call's arguments: sha256 of their JSON with object keys
/// sorted, so the same arguments in another order give the same digest.
pub fn args_digest(v: &serde_json::Value) -> String {
    let json = serde_json::to_string(&canonical(v)).unwrap_or_default();
    format!("sha256:{}", hex(&Sha256::digest(json.as_bytes())))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// `v` with every object rebuilt in key order.
fn canonical(v: &serde_json::Value) -> serde_json::Value {
    use serde_json::Value;
    match v {
        Value::Object(m) => {
            let sorted: std::collections::BTreeMap<&String, Value> =
                m.iter().map(|(k, v)| (k, canonical(v))).collect();
            let mut out = serde_json::Map::new();
            for (k, v) in sorted {
                out.insert(k.clone(), v);
            }
            Value::Object(out)
        }
        Value::Array(a) => Value::Array(a.iter().map(canonical).collect()),
        other => other.clone(),
    }
}

/// A copy of a call's arguments small enough to log: strings longer than
/// 200 characters keep their start, length and digest; arrays longer than
/// 50 keep 50 items and a count.
pub fn redact(v: &serde_json::Value) -> serde_json::Value {
    use serde_json::Value;
    const TEXT: usize = 200;
    const ITEMS: usize = 50;
    match v {
        Value::String(s) if s.chars().count() > TEXT => {
            let start: String = s.chars().take(TEXT).collect();
            Value::String(format!(
                "{start}… ({} chars, sha256:{})",
                s.chars().count(),
                hex(&Sha256::digest(s.as_bytes()))
            ))
        }
        Value::Array(a) => {
            let mut out: Vec<Value> = a.iter().take(ITEMS).map(redact).collect();
            if a.len() > ITEMS {
                out.push(Value::String(format!("… {} more", a.len() - ITEMS)));
            }
            Value::Array(out)
        }
        Value::Object(m) => Value::Object(m.iter().map(|(k, v)| (k.clone(), redact(v))).collect()),
        other => other.clone(),
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

    #[test]
    fn the_last_entry_is_read_from_the_end() {
        let dir = tempfile::tempdir().unwrap();
        for n in [1u64, 2, 10_000] {
            let path = dir.path().join(format!("audit-{n}.jsonl"));
            // Written in one go: appending 10 000 entries one by one syncs each.
            let mut text = String::new();
            let mut prev = GENESIS.to_string();
            for i in 0..n {
                let r = record(if i % 2 == 0 { "push" } else { "fetch" });
                let mut e = Entry {
                    seq: i + 1,
                    at: i,
                    actor: r.actor,
                    action: r.action,
                    subject: r.subject,
                    decision: r.decision,
                    reason: r.reason,
                    details: r.details,
                    prev: prev.clone(),
                    hash: String::new(),
                };
                e.hash = digest(&e);
                prev = e.hash.clone();
                text.push_str(&serde_json::to_string(&e).unwrap());
                text.push('\n');
            }
            std::fs::write(&path, text).unwrap();
            let log = AuditLog::new(&path);
            let all = log.verify().unwrap();
            let last = log.last().unwrap().unwrap();
            assert_eq!((last.seq, &last.hash), (n, &all.last().unwrap().hash));
        }
        // A file ending without a newline, or with blank lines.
        let path = dir.path().join("audit-1.jsonl");
        let text = std::fs::read_to_string(&path).unwrap();
        let log = AuditLog::new(&path);
        let expected = log.last().unwrap().unwrap();
        std::fs::write(&path, text.trim_end()).unwrap();
        assert_eq!(log.last().unwrap().unwrap(), expected);
        std::fs::write(&path, format!("{text}\n  \n")).unwrap();
        assert_eq!(log.last().unwrap().unwrap(), expected);
        assert!(
            AuditLog::new(dir.path().join("none.jsonl"))
                .last()
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn arguments_are_digested_in_key_order_and_redacted() {
        let a: serde_json::Value =
            serde_json::from_str(r#"{"a":1,"b":{"y":2,"x":[1,{"q":1,"p":2}]}}"#).unwrap();
        let b: serde_json::Value =
            serde_json::from_str(r#"{"b":{"x":[1,{"p":2,"q":1}],"y":2},"a":1}"#).unwrap();
        assert_eq!(args_digest(&a), args_digest(&b));
        assert!(args_digest(&a).starts_with("sha256:"));
        assert_ne!(args_digest(&a), args_digest(&serde_json::json!({"a": 2})));

        let long = "x".repeat(250);
        let items: Vec<u32> = (0..60).collect();
        let r = redact(&serde_json::json!({ "text": long, "items": items, "short": "ok" }));
        let text = r["text"].as_str().unwrap();
        assert!(text.starts_with(&"x".repeat(200)));
        assert!(text.contains("(250 chars, sha256:"), "{text}");
        assert_eq!(r["items"].as_array().unwrap().len(), 51);
        assert_eq!(r["items"][50], "… 10 more");
        assert_eq!(r["short"], "ok");
    }
}
