use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "tattle",
    version,
    about = "Catches AI coding agents that fake task completion",
    long_about = "tattle re-runs the tests your agent swore it passed — and shows exactly where it cheated.\n\nDetects deleted tests, skipped tests, weakened assertions, over-mocking,\nstubs, and runs your original test suite against the agent's modified code."
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Create tattle.yaml config and .tattle/ state directory
    Init,

    /// Record the task and snapshot held-out tests before the agent starts
    Start {
        /// The task or mission the agent is supposed to complete
        task: String,
        /// Agent name label (optional, stored in report)
        #[arg(long)]
        agent: Option<String>,
        /// Override test command(s); repeatable
        #[arg(long = "test-cmd", value_name = "CMD")]
        test_cmd: Vec<String>,
    },

    /// Run integrity checks and held-out test replay
    Check {
        /// Emit machine-readable JSON
        #[arg(long)]
        json: bool,
        /// Emit PR-comment-ready markdown
        #[arg(long)]
        markdown: bool,
        /// Skip the held-out test replay
        #[arg(long)]
        no_replay: bool,
        /// Treat SUSPICIOUS as failure (non-zero exit)
        #[arg(long)]
        strict: bool,
    },

    /// Run only the held-out test replay
    Replay {
        /// Emit machine-readable JSON
        #[arg(long)]
        json: bool,
    },

    /// Ask the optional LLM judge for a second opinion
    Judge {
        /// Provider: ollama, claude, openai, gemini, openrouter
        #[arg(short = 'p', long)]
        provider: Option<String>,
        /// Model name
        #[arg(short = 'm', long)]
        model: Option<String>,
    },

    /// Print the last check report
    Report {
        /// Emit PR-comment-ready markdown
        #[arg(long)]
        markdown: bool,
    },

    /// Run the minimal stdio JSON-RPC MCP server
    Mcp,

    /// Config management
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
}

#[derive(Debug, Subcommand)]
enum ConfigAction {
    /// Show current config as YAML
    Show,
    /// Set a config value (key=value pairs, edit tattle.yaml for structured changes)
    Set { key: String, value: String },
    /// Open tattle.yaml in $EDITOR
    Edit,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let cwd = std::env::current_dir()?;

    match cli.command {
        Commands::Init => {
            tattle::config::init(&cwd)?;
            println!("initialized tattle.yaml");
            println!("next: tattle start \"<task description>\"");
        }

        Commands::Start {
            task,
            agent,
            test_cmd,
        } => {
            let session = tattle::session::start(&cwd, task, agent, test_cmd)?;
            let base_short = &session.git_baseline[..7.min(session.git_baseline.len())];
            println!("started  task: {}", session.mission);
            println!("         base: {base_short}");
            if session.heldout_snapshotted {
                println!("         held-out tests snapshotted ✓");
            } else {
                eprintln!(
                    "WARNING: no test files found to snapshot — held-out replay will be skipped"
                );
            }
            if session.test_cmds.is_empty() {
                eprintln!("WARNING: no test command detected. Use --test-cmd or set replay.commands in tattle.yaml");
            } else {
                println!("         test cmd: {}", session.test_cmds.join(" && "));
            }
        }

        Commands::Check {
            json,
            markdown,
            no_replay,
            strict,
        } => {
            let config = tattle::config::load(&cwd)?;
            let session = tattle::session::load(&cwd)?;
            let report =
                tattle::report::run_check(&cwd, &config, session.as_ref(), no_replay, strict)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else if markdown {
                println!("{}", tattle::report::render_markdown(&report));
            } else {
                tattle::report::print_terminal(&report);
            }
            std::process::exit(report.exit_code);
        }

        Commands::Replay { json } => {
            let config = tattle::config::load(&cwd)?;
            let session = tattle::session::load(&cwd)?;
            let test_cmds = session
                .as_ref()
                .map(|s| s.test_cmds.clone())
                .unwrap_or_else(|| config.replay.commands.clone());
            let results =
                tattle::replay::run_held_out(&cwd, &test_cmds, config.replay.timeout_secs);
            if json {
                println!("{}", serde_json::to_string_pretty(&results)?);
            } else {
                for r in &results {
                    if let Some(reason) = &r.skipped_reason {
                        println!("SKIP  {} — {reason}", r.command);
                    } else if r.passed {
                        println!("PASS  {}", r.command);
                    } else {
                        println!("FAIL  {} (exit {:?})", r.command, r.exit_code);
                        if !r.stderr.is_empty() {
                            println!("{}", r.stderr);
                        }
                    }
                }
                if results.iter().any(|r| r.ran && !r.passed) {
                    std::process::exit(1);
                }
            }
        }

        Commands::Judge { provider, model } => {
            tattle::judge::run_judge(provider, model).await?;
        }

        Commands::Report { markdown } => {
            tattle::report::print_last(&cwd, markdown)?;
        }

        Commands::Mcp => {
            tattle::mcp::run_server().await?;
        }

        Commands::Config { action } => match action {
            ConfigAction::Show => {
                let config = tattle::config::load(&cwd)?;
                print!("{}", serde_yaml::to_string(&config)?);
            }
            ConfigAction::Set { key, value } => {
                println!("config set {key}={value}");
                println!("(edit tattle.yaml directly for structured config changes)");
            }
            ConfigAction::Edit => {
                let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());
                std::process::Command::new(&editor)
                    .arg(cwd.join(tattle::config::CONFIG_FILE))
                    .status()?;
            }
        },
    }

    Ok(())
}
