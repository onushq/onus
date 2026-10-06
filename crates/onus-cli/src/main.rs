//! `onus`: semantic change reports for pull requests.
#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use clap::{CommandFactory, Parser, Subcommand};
use onus_cli::{DiffOptions, EXIT_ERROR, EXIT_FAIL_ON, FailOn, Format};

const AFTER_HELP: &str = "\
Run `onus help <command>` for a command's flags and examples, and
`onus help <topic>` for the guide: getting-started, commands, reports,
changes, configuration, intent, ci, plugins, how-it-works, troubleshooting.";

/// The exit codes, shared by `onus help` and the commands that use them.
macro_rules! exit_codes {
    () => {
        "Exit codes:
  0  success, including reports with findings
  1  usage or runtime error
  2  a --fail-on condition was met"
    };
}

const EXIT_CODES: &str = exit_codes!();

const DIFF_HELP: &str = concat!(
    "Examples:
  onus diff old new
  onus diff old new --format json > report.json
  onus diff old new --intent pr-body.md --fail-on rule-violation,secrets

",
    exit_codes!()
);

const REPORT_HELP: &str = concat!(
    "Examples:
  onus report --base main --head HEAD
  onus report --repo ../shop --base origin/main --head feature/sms --format json
  onus report --base \"$BASE_SHA\" --head \"$HEAD_SHA\" --intent pr-body.md --fail-on secrets

",
    exit_codes!()
);

#[derive(Debug, Parser)]
#[command(
    name = "onus",
    version,
    about = "Turns a pull request into a short report of changes in meaning",
    long_about = "Onus reads two versions of a codebase, builds a map of each and reports \
                  what changed in meaning: contracts, relationships, external services, \
                  tests and rules. It only reads files; it never installs or runs the \
                  code it analyzes.",
    after_help = AFTER_HELP,
    disable_help_subcommand = true
)]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

/// Plugins and trusted mode (`onus help plugins`).
#[derive(Debug, Clone, clap::Args)]
struct ProviderArgs {
    /// A plugins file listing language, framework, SCIP and LSP plugins
    /// (default: $ONUS_PLUGINS). Never read from the analyzed repository.
    #[arg(long, value_name = "FILE")]
    plugins: Option<PathBuf>,
    /// Allow plugins that may run code from the repository (indexers,
    /// language servers). They run in a sandbox.
    #[arg(long)]
    trusted: bool,
    /// With --trusted: allow them even when no sandbox is available.
    #[arg(long, requires = "trusted")]
    allow_unsandboxed: bool,
}

impl ProviderArgs {
    fn load(&self) -> Result<onus_cli::Providers> {
        onus_cli::Providers::load(
            self.plugins.as_deref(),
            self.trusted,
            self.allow_unsandboxed,
        )
    }
}

#[derive(Debug, Subcommand)]
enum Cmd {
    /// Build the codebase map of a directory.
    #[command(
        long_about = "Build the codebase map of a directory: its components, public contracts, \
                      relationships, events, data access, external services, tests and \
                      anything Onus could not resolve. Prints a summary, or the whole map \
                      with --json.",
        after_long_help = "\
Examples:
  onus map .
  onus map path/to/repo --json > map.json
  onus map . --config other/onus.yaml"
    )]
    Map {
        /// The repository or directory to map.
        dir: PathBuf,
        /// Print the full map as JSON (schemas/codebase-map.schema.json).
        #[arg(long)]
        json: bool,
        /// Use this onus.yaml instead of the directory's own.
        #[arg(long, value_name = "FILE")]
        config: Option<PathBuf>,
        /// Import a SCIP index of this directory (repeatable). Runs nothing.
        #[arg(long, value_name = "FILE")]
        scip: Vec<PathBuf>,
        #[command(flatten)]
        providers: ProviderArgs,
    },
    /// Report the changes in meaning between two directories.
    #[command(
        long_about = "Report the changes in meaning between two directories: the base (before) \
                      and the head (after). Both are mapped with the base directory's \
                      onus.yaml, unless --config says otherwise.",
        after_long_help = DIFF_HELP
    )]
    Diff {
        /// The directory before the change.
        base: PathBuf,
        /// The directory after the change.
        head: PathBuf,
        /// Markdown for people, or the full JSON report (schemas/semantic-report.schema.json).
        #[arg(long, value_enum, default_value = "md")]
        format: Format,
        /// A YAML intent file, or Markdown (such as a pull request body) containing an
        /// `onus-intent` block. Changes outside it are ranked first. See `onus help intent`.
        #[arg(long, value_name = "FILE")]
        intent: Option<PathBuf>,
        /// Use this onus.yaml for both trees (default: the base tree's).
        #[arg(long, value_name = "FILE")]
        config: Option<PathBuf>,
        /// Exit with code 2 when this is found. Repeat the flag or separate values with commas.
        #[arg(long, value_enum, value_delimiter = ',', value_name = "WHAT")]
        fail_on: Vec<FailOn>,
        /// Import a SCIP index of the base directory (repeatable).
        #[arg(long, value_name = "FILE")]
        base_scip: Vec<PathBuf>,
        /// Import a SCIP index of the head directory (repeatable).
        #[arg(long, value_name = "FILE")]
        head_scip: Vec<PathBuf>,
        #[command(flatten)]
        providers: ProviderArgs,
    },
    /// Report the changes in meaning between two git refs.
    #[command(
        long_about = "Report the changes in meaning between two git refs. Each ref is extracted \
                      with `git archive` into a temporary directory that is removed afterwards; \
                      the repository itself is never modified.",
        after_long_help = REPORT_HELP
    )]
    Report {
        /// The ref before the change: a branch, tag or commit.
        #[arg(long, value_name = "REF")]
        base: String,
        /// The ref after the change: a branch, tag or commit.
        #[arg(long, value_name = "REF")]
        head: String,
        /// The git repository.
        #[arg(long, default_value = ".", value_name = "DIR")]
        repo: PathBuf,
        /// Markdown for people, or the full JSON report (schemas/semantic-report.schema.json).
        #[arg(long, value_enum, default_value = "md")]
        format: Format,
        /// A YAML intent file, or Markdown (such as a pull request body) containing an
        /// `onus-intent` block. Changes outside it are ranked first. See `onus help intent`.
        #[arg(long, value_name = "FILE")]
        intent: Option<PathBuf>,
        /// Use this onus.yaml for both refs (default: the base ref's).
        #[arg(long, value_name = "FILE")]
        config: Option<PathBuf>,
        /// Exit with code 2 when this is found. Repeat the flag or separate values with commas.
        #[arg(long, value_enum, value_delimiter = ',', value_name = "WHAT")]
        fail_on: Vec<FailOn>,
        /// Import a SCIP index of the base ref (repeatable). Runs nothing.
        #[arg(long, value_name = "FILE")]
        base_scip: Vec<PathBuf>,
        /// Import a SCIP index of the head ref (repeatable). Runs nothing.
        #[arg(long, value_name = "FILE")]
        head_scip: Vec<PathBuf>,
        #[command(flatten)]
        providers: ProviderArgs,
    },
    /// Write a starter onus.yaml inferred from the repository.
    #[command(
        long_about = "Write a starter onus.yaml inferred from workspaces and CODEOWNERS. \
                      Sensitivity labels are only suggested, as comments, until you confirm \
                      them. See `onus help configuration`.",
        after_long_help = "\
Examples:
  onus init
  onus init path/to/repo --stdout
  onus init --force"
    )]
    Init {
        /// The repository to inspect and write onus.yaml into.
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
    #[command(after_long_help = "\
Examples:
  onus schema --out schemas
  onus schema")]
    Schema {
        /// Directory to write the schema files into (default: print them).
        #[arg(long, value_name = "DIR")]
        out: Option<PathBuf>,
    },
    /// Show help for a command, or a topic from the guide.
    #[command(after_long_help = "\
Examples:
  onus help
  onus help diff
  onus help configuration")]
    Help {
        /// A command (map, diff, report, init, schema) or a guide topic.
        topic: Option<String>,
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
        Cmd::Map {
            dir,
            json,
            config,
            scip,
            providers,
        } => {
            if !dir.is_dir() {
                bail!("{} is not a directory", dir.display());
            }
            let cfg = match config {
                Some(p) => Some(onus_map::config::load(&p)?),
                None => onus_map::config::load_from_tree(&dir)?,
            };
            let map = onus_cli::build_with(&dir, cfg, None, &providers.load()?, &scip)?;
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
            base_scip,
            head_scip,
            providers,
        } => {
            let opts = DiffOptions {
                config,
                intent: intent(intent_path.as_ref())?,
                base_label: "base".into(),
                head_label: "head".into(),
                base_commit: None,
                head_commit: None,
                providers: providers.load()?,
                base_scip,
                head_scip,
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
            base_scip,
            head_scip,
            providers,
        } => {
            let providers = providers.load()?;
            let b = onus_cli::materialize(&repo, &base)?;
            let h = onus_cli::materialize(&repo, &head)?;
            let opts = DiffOptions {
                config,
                intent: intent(intent_path.as_ref())?,
                base_label: onus_cli::ref_label(&base, &b.sha),
                head_label: onus_cli::ref_label(&head, &h.sha),
                base_commit: Some(b.sha.clone()),
                head_commit: Some(h.sha.clone()),
                providers,
                base_scip,
                head_scip,
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
        Cmd::Help { topic } => help(topic.as_deref()),
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

/// `onus help [command | topic]`.
fn help(topic: Option<&str>) -> Result<i32> {
    let mut cmd = Cli::command();
    cmd.build();
    let Some(name) = topic else {
        // The topic table below replaces the short pointer to it.
        let mut top = cmd.clone().after_help(None::<&str>);
        print!("{}", top.render_long_help());
        println!("\n{}", onus_cli::guide::topic_list());
        println!("{EXIT_CODES}");
        return Ok(0);
    };
    if let Some(sub) = cmd.find_subcommand_mut(name) {
        print!("{}", sub.render_long_help());
        return Ok(0);
    }
    if let Some(t) = onus_cli::guide::find(name) {
        print!("{}", t.text);
        return Ok(0);
    }
    eprintln!("onus: no command or guide topic named `{name}`\n");
    let commands: Vec<String> = cmd
        .get_subcommands()
        .map(|c| c.get_name().to_string())
        .collect();
    eprintln!("Commands: {}\n", commands.join(", "));
    eprint!("{}", onus_cli::guide::topic_list());
    Ok(EXIT_ERROR)
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
