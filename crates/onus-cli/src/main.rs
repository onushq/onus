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
  onus report --base \"$BASE_SHA\" --head \"$HEAD_SHA\" --cache-dir ~/.cache/onus

",
    exit_codes!()
);

#[derive(Debug, Parser)]
#[command(
    name = "onus",
    bin_name = "onus",
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
        /// Keep the base ref's map in this folder and reuse it on the next run against the
        /// same base commit. Safe to share between runs: the file name covers everything that
        /// shapes the map.
        #[arg(long, value_name = "DIR")]
        cache_dir: Option<PathBuf>,
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
    /// Serve the codebase map to coding agents over MCP (stdio).
    #[command(
        long_about = "Serve the codebase map of this worktree to a coding agent as MCP tools, on \
                      stdin and stdout. Tools: onus_status, onus_find, onus_symbol, \
                      onus_dependents, onus_dependencies, onus_tests_for, onus_owners, \
                      onus_component, onus_file and onus_check. All agents and worktrees of a \
                      repository share one map server, started on first use, which follows \
                      file changes and rebuilds only what changed. See `onus help agents`.",
        after_long_help = "\
Examples:
  onus mcp
  claude mcp add onus -- onus mcp
  onus mcp --repo ../worktrees/feature-a"
    )]
    Mcp {
        /// The worktree to serve (default: the one containing the current directory).
        #[arg(long, default_value = ".", value_name = "DIR")]
        repo: PathBuf,
        /// Build the map in this process instead of the shared map server.
        #[arg(long)]
        no_server: bool,
    },
    /// Ask the codebase map a question; the same answers as the MCP tools.
    #[command(after_long_help = "\
Examples:
  onus query status
  onus query find format phone
  onus query dependents UserPreferences --depth 2
  onus query tests-for services/billing/src/payments.ts
  onus query check --base main")]
    Query {
        #[command(subcommand)]
        question: QueryCmd,
        /// The worktree to ask about (default: the one containing the current directory).
        #[arg(long, global = true, default_value = ".", value_name = "DIR")]
        repo: PathBuf,
        /// Build the map in this process instead of the shared map server.
        #[arg(long, global = true)]
        no_server: bool,
    },
    /// Run a repository's map server (started automatically by `onus mcp`).
    #[command(hide = true)]
    Serve {
        /// The Unix socket to listen on.
        #[arg(long, value_name = "PATH")]
        socket: PathBuf,
        /// Exit after this many seconds without requests.
        #[arg(long, default_value_t = 1800, value_name = "SECONDS")]
        idle: u64,
    },
    /// Show help for a command, or a topic from the guide.
    #[command(after_long_help = "\
Examples:
  onus help
  onus help diff
  onus help configuration")]
    Help {
        /// A command (map, diff, report, init, schema, mcp, query) or a guide topic.
        topic: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
enum QueryCmd {
    /// What the map holds.
    Status,
    /// Symbols whose names match some words, best first.
    Find {
        #[arg(required = true, num_args = 1..)]
        words: Vec<String>,
        #[arg(long, default_value_t = 0)]
        limit: usize,
    },
    /// One symbol: kind, location, shape, invariants and counts.
    Symbol { id: String },
    /// What depends on a symbol, file, module or component.
    Dependents {
        target: String,
        #[arg(long, default_value_t = 1)]
        depth: u32,
        #[arg(long, default_value_t = 0)]
        limit: usize,
    },
    /// What a symbol, file, module or component depends on.
    Dependencies {
        target: String,
        #[arg(long, default_value_t = 1)]
        depth: u32,
        #[arg(long, default_value_t = 0)]
        limit: usize,
    },
    /// The tests that exercise a symbol, file, module or component.
    TestsFor {
        target: String,
        #[arg(long, default_value_t = 0)]
        limit: usize,
    },
    /// Who owns a symbol, file or component.
    Owners { target: String },
    /// A component: owners, public symbols, what it uses and what uses it.
    Component { id: String },
    /// A file: component, symbols, imports and importers.
    File { path: String },
    /// Changes in meaning between a commit and the worktree as it is now.
    Check {
        /// The commit to compare with.
        #[arg(long, default_value = "HEAD")]
        base: String,
    },
    /// The map server's worktrees and cache use.
    Stats,
}

impl QueryCmd {
    fn op(self) -> onus_cli::daemon::Op {
        use onus_cli::daemon::Op;
        use onus_index::Query;
        let q = |query| Op::Query { query };
        match self {
            QueryCmd::Status => q(Query::Status),
            QueryCmd::Find { words, limit } => q(Query::Find {
                text: words.join(" "),
                limit,
            }),
            QueryCmd::Symbol { id } => q(Query::Symbol { id }),
            QueryCmd::Dependents {
                target,
                depth,
                limit,
            } => q(Query::Dependents {
                target,
                depth,
                limit,
            }),
            QueryCmd::Dependencies {
                target,
                depth,
                limit,
            } => q(Query::Dependencies {
                target,
                depth,
                limit,
            }),
            QueryCmd::TestsFor { target, limit } => q(Query::TestsFor { target, limit }),
            QueryCmd::Owners { target } => q(Query::Owners { target }),
            QueryCmd::Component { id } => q(Query::Component { id }),
            QueryCmd::File { path } => q(Query::File { path }),
            QueryCmd::Check { base } => Op::Check { base: Some(base) },
            QueryCmd::Stats => Op::Stats,
        }
    }
}

/// The shared map server of the repository holding `root`, or this process.
fn backend(root: &std::path::Path, no_server: bool) -> std::sync::Arc<dyn onus_cli::mcp::Backend> {
    #[cfg(unix)]
    if !no_server {
        return std::sync::Arc::new(onus_cli::daemon::Client::new(
            onus_cli::daemon::socket_path(root),
        ));
    }
    let _ = (root, no_server);
    std::sync::Arc::new(onus_cli::daemon::Server::new())
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
                cache_dir: None,
            };
            let outcome = onus_cli::diff_dirs(&base, &head, &opts)?;
            for note in &outcome.notes {
                eprintln!("onus: {note}");
            }
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
            cache_dir,
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
                cache_dir,
            };
            let outcome = onus_cli::diff_dirs(b.dir.path(), h.dir.path(), &opts)?;
            for note in &outcome.notes {
                eprintln!("onus: {note}");
            }
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
        Cmd::Mcp { repo, no_server } => {
            let root = onus_cli::daemon::worktree_root(&repo);
            onus_cli::mcp::serve_stdio(root.clone(), backend(&root, no_server))?;
            Ok(0)
        }
        Cmd::Query {
            question,
            repo,
            no_server,
        } => {
            let root = onus_cli::daemon::worktree_root(&repo);
            let req = onus_cli::daemon::Request {
                root: root.clone(),
                op: question.op(),
            };
            let response = backend(&root, no_server).call(&req)?;
            if let Some(error) = response.error {
                bail!("{error}");
            }
            let body = serde_json::json!({ "result": response.ok, "map": response.map });
            println!("{}", serde_json::to_string_pretty(&body)?);
            Ok(0)
        }
        Cmd::Serve { socket, idle } => {
            #[cfg(unix)]
            onus_cli::daemon::serve(&socket, std::time::Duration::from_secs(idle))?;
            #[cfg(not(unix))]
            {
                let _ = (socket, idle);
                bail!("the map server needs Unix sockets; use --no-server");
            }
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
