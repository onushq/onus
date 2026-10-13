//! `onus signals`: production signals kept with the records (ADR 0016,
//! `onus help production`).

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::Subcommand;
use onus_core::signal::{CodeRef, ObservedCall, ProductionSignal, SignalKind};
use onus_lanes::outcomes::HistoryScope;
use onus_lanes::signals;
use sha2::{Digest, Sha256};

use crate::lanes;

/// The most calls one trace signal keeps, the busiest first.
const MAX_CALLS: usize = 5000;

#[derive(Debug, Subcommand)]
pub enum SignalsCmd {
    /// Record signals from monitoring or deploy tools: a JSON object, an array, or
    /// JSON Lines (schemas/production-signal.schema.json). Each id is kept once;
    /// incidents and rollbacks tied to a change become its outcomes.
    #[command(after_long_help = "\
Examples:
  onus signals ingest --records rec --file pagerduty.json
  echo '{\"id\":\"pd:1\",\"kind\":\"incident\",\"commit\":\"3f9a1c2\",\"note\":\"500s\"}' \\
    | onus signals ingest --records rec --file -")]
    Ingest {
        /// The records folder (see `onus ci records`).
        #[arg(long, value_name = "DIR")]
        records: PathBuf,
        /// Signals to record; `-` reads stdin (repeatable).
        #[arg(long = "file", value_name = "FILE", required = true)]
        files: Vec<PathBuf>,
        /// The time of signals that give none, in seconds since 1970; now when left out.
        #[arg(long, value_name = "SECONDS")]
        at: Option<u64>,
    },
    /// Keep the calls production traces observed (OTLP JSON) as one trace signal;
    /// `--traces <records>/signals.jsonl` then folds them into the map.
    #[command(after_long_help = "\
Examples:
  onus signals traces --records rec --file otel-export.json --source otel-prod
  onus signals traces --records rec --file otel-export.json --repo .
  onus signals traces --records rec --file traces.jsonl --service-component checkout-api=orders")]
    Traces {
        #[arg(long, value_name = "DIR")]
        records: PathBuf,
        /// OTLP JSON: one document, or one per line (repeatable).
        #[arg(long = "file", value_name = "FILE", required = true)]
        files: Vec<PathBuf>,
        /// Where the traces came from.
        #[arg(long, default_value = "otel")]
        source: String,
        /// A service whose name is not its component's id (repeatable).
        #[arg(long = "service-component", value_name = "SERVICE=COMPONENT")]
        service_component: Vec<String>,
        /// The repository whose components the services are: the signal names
        /// the ones it finds. Without it, only --service-component targets.
        #[arg(long, value_name = "DIR")]
        repo: Option<PathBuf>,
        /// When, in seconds since 1970; now when left out.
        #[arg(long, value_name = "SECONDS")]
        at: Option<u64>,
    },
    /// Each component's history in the window: reverts and rollbacks, incidents and
    /// error spikes, from the outcomes and the signals.
    Areas {
        #[arg(long, value_name = "DIR")]
        records: PathBuf,
        /// How many days back (default: lanes.inputs.history.days in --repo, 90).
        #[arg(long)]
        days: Option<u32>,
        /// The repository whose onus.yaml sets the window.
        #[arg(long, default_value = ".", value_name = "DIR")]
        repo: PathBuf,
    },
}

/// The signals in one input, each with where it was (`line 3 of f.jsonl`).
fn parse(text: &str, name: &str) -> Result<Vec<(String, ProductionSignal)>> {
    let trimmed = text.trim_start();
    if trimmed.is_empty() {
        return Ok(vec![]);
    }
    // One object or an array; else JSON Lines.
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(text) {
        let items = match v {
            serde_json::Value::Array(items) => items,
            one => vec![one],
        };
        let n = items.len();
        return items
            .into_iter()
            .enumerate()
            .map(|(i, item)| {
                let at = if n == 1 && !trimmed.starts_with('[') {
                    name.to_string()
                } else {
                    format!("item {} of {name}", i + 1)
                };
                let s = serde_json::from_value(item)
                    .with_context(|| format!("{at} is not a production signal"))?;
                Ok((at, s))
            })
            .collect();
    }
    text.lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty())
        .map(|(i, l)| {
            let at = format!("line {} of {name}", i + 1);
            let s = serde_json::from_str(l)
                .with_context(|| format!("{at} is not a production signal"))?;
            Ok((at, s))
        })
        .collect()
}

fn read_input(path: &Path) -> Result<(String, String)> {
    if path == Path::new("-") {
        let mut text = String::new();
        std::io::stdin().read_to_string(&mut text)?;
        return Ok((text, "stdin".into()));
    }
    let text =
        std::fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
    Ok((text, path.display().to_string()))
}

fn ingest(records: &Path, files: &[PathBuf], at: Option<u64>) -> Result<i32> {
    let mut all = Vec::new();
    for f in files {
        let (text, name) = read_input(f)?;
        for (at, s) in parse(&text, &name)? {
            // The whole input fails on one bad signal, naming where it is.
            if let Err(why) = signals::check(&s) {
                bail!("{at}: {why}");
            }
            if s.kind == SignalKind::Rollout {
                bail!(
                    "{at}: rollout stages carry Onus's decisions; they are written by `onus ci rollout`"
                );
            }
            all.push(s);
        }
    }
    let now = at.unwrap_or_else(onus_doors::gateway::now);
    let summary = signals::ingest(records, all, now)?;
    println!("{}", serde_json::to_string(&summary)?);
    Ok(0)
}

/// `SERVICE=COMPONENT` pairs.
fn aliases(pairs: &[String]) -> Result<BTreeMap<String, String>> {
    pairs
        .iter()
        .map(|p| {
            let (s, c) = p
                .split_once('=')
                .filter(|(s, c)| !s.is_empty() && !c.is_empty())
                .with_context(|| format!("`{p}`: write SERVICE=COMPONENT"))?;
            Ok((s.to_string(), c.to_string()))
        })
        .collect()
}

/// The component ids of the repository at `root`, found as the map finds
/// them (onus.yaml, workspaces, then folders).
fn component_ids(root: &Path) -> Result<BTreeSet<String>> {
    if !root.is_dir() {
        bail!("{} is not a directory", root.display());
    }
    let config = onus_map::config::load_from_tree(root)?;
    let files = onus_map::walk::list_files(root);
    Ok(
        onus_map::discover::discover(root, &files, config.as_ref().map(|c| &c.config))
            .components
            .into_iter()
            .map(|c| c.id)
            .collect(),
    )
}

/// What `onus signals traces` keeps.
struct TraceArgs<'a> {
    records: &'a Path,
    files: &'a [PathBuf],
    source: String,
    service_component: &'a [String],
    repo: Option<&'a Path>,
    at: Option<u64>,
}

fn traces(a: TraceArgs) -> Result<i32> {
    let TraceArgs {
        records,
        files,
        source,
        service_component,
        repo,
        at,
    } = a;
    let alias = aliases(service_component)?;
    let mut known: BTreeSet<String> = alias.values().cloned().collect();
    if let Some(root) = repo {
        known.extend(component_ids(root)?);
    }
    let rename = |c: &CodeRef| CodeRef {
        service: alias.get(&c.service).cloned().unwrap_or(c.service.clone()),
        ..c.clone()
    };
    // Identical pairs are one call, counted.
    let mut counted: BTreeMap<(CodeRef, CodeRef), u32> = BTreeMap::new();
    for call in onus_map::traces::observed_calls(files).map_err(|e| anyhow::anyhow!(e))? {
        *counted
            .entry((rename(&call.from), rename(&call.to)))
            .or_default() += call.count;
    }
    if counted.is_empty() {
        bail!("no calls found: the spans carry no code attributes (code.filepath, code.function)");
    }
    let mut calls: Vec<ObservedCall> = counted
        .into_iter()
        .map(|((from, to), count)| ObservedCall { from, to, count })
        .collect();
    // The busiest first, the rest in a fixed order.
    calls.sort_by_key(|c| std::cmp::Reverse(c.count));
    calls.truncate(MAX_CALLS);
    let observed = calls.len();
    let mut keyed = calls.clone();
    keyed.sort_by(|a, b| (&a.from, &a.to).cmp(&(&b.from, &b.to)));
    let digest = Sha256::digest(serde_json::to_vec(&keyed)?);
    let id = format!(
        "trace:{}",
        digest
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
            .get(..16)
            .unwrap_or_default()
    );
    let mut components: Vec<String> = calls
        .iter()
        .flat_map(|c| [&c.from.service, &c.to.service])
        .filter(|s| known.contains(*s))
        .cloned()
        .collect();
    components.sort();
    components.dedup();
    let now = at.unwrap_or_else(onus_doors::gateway::now);
    let summary = signals::ingest(
        records,
        vec![ProductionSignal {
            id: id.clone(),
            kind: SignalKind::Trace,
            at: Some(now),
            source,
            severity: None,
            change: None,
            commit: None,
            deploy: None,
            environment: None,
            components,
            symbols: vec![],
            note: None,
            metrics: vec![],
            calls,
            rollout: None,
            attributed_by: None,
        }],
        now,
    )?;
    println!(
        "{}",
        serde_json::json!({ "recorded": summary.recorded, "calls": observed, "id": id })
    );
    Ok(0)
}

fn areas(records: &Path, days: Option<u32>, repo: &Path) -> Result<i32> {
    let days = match days {
        Some(d) => d,
        None => lanes::lanes_config(repo, None)?.inputs.history.days,
    };
    let now = onus_doors::gateway::now();
    let since = now.saturating_sub(u64::from(days) * 86_400);
    let areas = lanes::areas_for(
        records,
        &HistoryScope {
            since,
            until: Some(now),
            repo: None,
            exclude: None,
        },
    )?;
    println!(
        "{}",
        serde_json::to_string_pretty(
            &serde_json::json!({ "since": since, "days": days, "areas": areas })
        )?
    );
    Ok(0)
}

pub fn run(cmd: SignalsCmd) -> Result<i32> {
    match cmd {
        SignalsCmd::Ingest { records, files, at } => ingest(&records, &files, at),
        SignalsCmd::Traces {
            records,
            files,
            source,
            service_component,
            repo,
            at,
        } => traces(TraceArgs {
            records: &records,
            files: &files,
            source,
            service_component: &service_component,
            repo: repo.as_deref(),
            at,
        }),
        SignalsCmd::Areas {
            records,
            days,
            repo,
        } => areas(&records, days, &repo),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inputs_are_an_object_an_array_or_lines() {
        let one = parse(r#"{"id":"a","kind":"incident"}"#, "f.json").unwrap();
        assert_eq!(one[0].0, "f.json");
        let many = parse(
            r#"[{"id":"a","kind":"incident"},{"id":"b","kind":"metric"}]"#,
            "f.json",
        )
        .unwrap();
        assert_eq!(many[1].0, "item 2 of f.json");
        let lines = parse(
            "{\"id\":\"a\",\"kind\":\"incident\"}\n\n{\"id\":\"b\",\"kind\":\"metric\"}\n",
            "f.jsonl",
        )
        .unwrap();
        assert_eq!((lines.len(), lines[1].0.as_str()), (2, "line 3 of f.jsonl"));
        let err = parse(
            "{\"id\":\"a\",\"kind\":\"incident\"}\n{\"id\":\"b\",\"kind\":\"outage\"}\n",
            "f.jsonl",
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("line 2 of f.jsonl"), "{err:#}");
        assert!(parse("  \n", "f").unwrap().is_empty());
        assert!(aliases(&["checkout=orders".into()]).is_ok());
        assert!(aliases(&["checkout".into()]).is_err());
    }
}
