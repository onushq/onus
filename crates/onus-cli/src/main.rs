//! `onus`: semantic change reports for pull requests.
#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use onus_cli::{DiffOptions, EXIT_ERROR, EXIT_FAIL_ON, FailOn, Format};

#[derive(Debug, Parser)]
#[command(
    name = "onus",
    version,
    about = "Turns a pull request into a short report of changes in meaning",
    long_about = "Onus reads two versions of a codebase, builds a map of each and reports \
                  what changed in meaning: contracts, relationships, external services, \
                  tests and rules. It only reads files; it never installs or runs the \
                  code it analyzes."
)]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Debug, Subcommand)]
enum Cmd {
    /// Build the codebase map of a directory.
    Map {
        dir: PathBuf,
        /// Print the full map as JSON.
        #[arg(long)]
        json: bool,
        /// Use this onus.yaml instead of the directory's own.
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// Report the changes in meaning between two directories.
    Diff {
        base: PathBuf,
        head: PathBuf,
        #[arg(long, value_enum, default_value = "md")]
        format: Format,
        /// A YAML intent file, or Markdown containing an `onus-intent` block.
        #[arg(long)]
        intent: Option<PathBuf>,
        /// Use this onus.yaml for both trees (default: the base tree's).
        #[arg(long)]
        config: Option<PathBuf>,
        /// Exit with code 2 when this is found. Repeatable.
        #[arg(long, value_enum, value_delimiter = ',')]
        fail_on: Vec<FailOn>,
    },
    /// Report the changes in meaning between two git refs.
    Report {
        #[arg(long)]
        base: String,
        #[arg(long)]
        head: String,
        /// The git repository (default: the current directory).
        #[arg(long, default_value = ".")]
        repo: PathBuf,
        #[arg(long, value_enum, default_value = "md")]
        format: Format,
        #[arg(long)]
        intent: Option<PathBuf>,
        #[arg(long)]
        config: Option<PathBuf>,
        #[arg(long, value_enum, value_delimiter = ',')]
        fail_on: Vec<FailOn>,
    },
    /// Write a starter onus.yaml inferred from the repository.
    Init {
        #[arg(default_value = ".")]
        dir: PathBuf,
        /// Overwrite an existing onus.yaml.
        #[arg(long)]
        force: bool,
        /// Print the file instead of writing it.
        #[arg(long)]
        stdout: bool,
    },
    /// Write the JSON Schemas of the map, report and config formats.
    Schema {
        /// Directory to write the schema files into (default: print them).
        #[arg(long)]
        out: Option<PathBuf>,
    },
}

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) => {
            let code = if e.use_stderr() { EXIT_ERROR } else { 0 };
            let _ = e.print();
            return ExitCode::from(code as u8);
        }
    };
    match run(cli) {
        Ok(code) => ExitCode::from(code as u8),
        Err(e) => {
            eprintln!("onus: {e:#}");
            ExitCode::from(EXIT_ERROR as u8)
        }
    }
}

fn intent(path: Option<&PathBuf>) -> Result<Option<onus_diff::Intent>> {
    let Some(p) = path else {
        return Ok(None);
    };
    let parsed = onus_cli::read_intent(p)?;
    if parsed.is_none() {
        eprintln!(
            "onus: {} has no onus-intent block; the intent check is skipped",
            p.display()
        );
    }
    Ok(parsed)
}

fn run(cli: Cli) -> Result<i32> {
    match cli.command {
        Cmd::Map { dir, json, config } => {
            if !dir.is_dir() {
                bail!("{} is not a directory", dir.display());
            }
            let cfg = match config {
                Some(p) => Some(onus_map::config::load(&p)?),
                None => onus_map::config::load_from_tree(&dir)?,
            };
            let map = onus_cli::build(&dir, cfg, None)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&map)?);
            } else {
                print!("{}", map_summary(&map));
            }
            Ok(0)
        }
        Cmd::Diff {
            base,
            head,
            format,
            intent: intent_path,
            config,
            fail_on,
        } => {
            let opts = DiffOptions {
                config,
                intent: intent(intent_path.as_ref())?,
                base_label: "base".into(),
                head_label: "head".into(),
                base_commit: None,
                head_commit: None,
            };
            let outcome = onus_cli::diff_dirs(&base, &head, &opts)?;
            print!("{}", outcome.render(format));
            Ok(if onus_cli::should_fail(&outcome.report, &fail_on) {
                EXIT_FAIL_ON
            } else {
                0
            })
        }
        Cmd::Report {
            base,
            head,
            repo,
            format,
            intent: intent_path,
            config,
            fail_on,
        } => {
            let b = onus_cli::materialize(&repo, &base)?;
            let h = onus_cli::materialize(&repo, &head)?;
            let opts = DiffOptions {
                config,
                intent: intent(intent_path.as_ref())?,
                base_label: onus_cli::ref_label(&base, &b.sha),
                head_label: onus_cli::ref_label(&head, &h.sha),
                base_commit: Some(b.sha.clone()),
                head_commit: Some(h.sha.clone()),
            };
            let outcome = onus_cli::diff_dirs(b.dir.path(), h.dir.path(), &opts)?;
            print!("{}", outcome.render(format));
            Ok(if onus_cli::should_fail(&outcome.report, &fail_on) {
                EXIT_FAIL_ON
            } else {
                0
            })
        }
        Cmd::Init { dir, force, stdout } => {
            let text = onus_map::init::init_config(&dir);
            if stdout {
                print!("{text}");
                return Ok(0);
            }
            let path = dir.join(onus_map::config::CONFIG_FILE);
            if path.exists() && !force {
                bail!(
                    "{} already exists (use --force to overwrite)",
                    path.display()
                );
            }
            std::fs::write(&path, text)
                .with_context(|| format!("cannot write {}", path.display()))?;
            eprintln!("onus: wrote {}", path.display());
            Ok(0)
        }
        Cmd::Schema { out } => {
            let schemas = onus_core::schema::all_schemas();
            match out {
                Some(dir) => {
                    std::fs::create_dir_all(&dir)?;
                    for (name, json) in schemas {
                        std::fs::write(dir.join(name), json)?;
                    }
                }
                None => {
                    for (name, json) in schemas {
                        println!("// {name}");
                        print!("{json}");
                    }
                }
            }
            Ok(0)
        }
    }
}

fn map_summary(map: &onus_core::CodebaseMap) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{} components, {} files, {} symbols, {} edges, {} external services, {} test files, {} diagnostics",
        map.components.len(),
        map.files.len(),
        map.symbols.len(),
        map.edges.len(),
        map.externals.len(),
        map.tests.len(),
        map.diagnostics.len()
    );
    for c in &map.components {
        let public = map
            .symbols
            .iter()
            .filter(|s| {
                s.component_id.as_deref() == Some(c.id.as_str())
                    && s.visibility == onus_core::Visibility::Public
            })
            .count();
        let _ = writeln!(
            out,
            "  {:<20} {:<8} {} public symbols{}{}",
            c.id,
            match c.kind {
                onus_core::ComponentKind::Package => "package",
                onus_core::ComponentKind::Service => "service",
                onus_core::ComponentKind::Module => "module",
            },
            public,
            if c.labels.is_empty() {
                String::new()
            } else {
                format!(", labels: {}", c.labels.join(", "))
            },
            if c.owners.is_empty() {
                String::new()
            } else {
                format!(", owners: {}", c.owners.join(" "))
            }
        );
    }
    for e in &map.externals {
        let _ = writeln!(
            out,
            "  external: {} ({}; egress: {})",
            e.vendor,
            e.category,
            e.egress.join(", ")
        );
    }
    for d in &map.diagnostics {
        let _ = writeln!(
            out,
            "  note: {}:{} {}: {}",
            d.file, d.line, d.kind, d.message
        );
    }
    out
}
