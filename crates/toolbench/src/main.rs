use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use toolbench::{config, report, result, runner, scenario, snapshot};

#[derive(Parser)]
#[command(
    name = "toolbench",
    about = "Tool-call reliability benchmark for local model servers"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// List a snapshot's tools with their required and optional arguments.
    Tools { snapshot: PathBuf },
    /// Run every scenario against one endpoint.
    Run {
        #[arg(long, default_value = "toolbench.toml")]
        config: PathBuf,
        #[arg(long)]
        endpoint: String,
        #[arg(long, default_value = "scenarios")]
        scenarios: PathBuf,
        #[arg(long, default_value = "fixtures/tools")]
        snapshots: PathBuf,
        #[arg(long, default_value_t = 5)]
        repeat: u32,
        #[arg(long, default_value_t = 120)]
        timeout: u64,
        #[arg(long)]
        out: PathBuf,
    },
    /// Re-apply the checks to stored results.
    Rescore {
        input: PathBuf,
        #[arg(long)]
        out: PathBuf,
        #[arg(long, default_value = "scenarios")]
        scenarios: PathBuf,
        #[arg(long, default_value = "fixtures/tools")]
        snapshots: PathBuf,
    },
    /// Markdown comparison and verdict from result files.
    Report {
        inputs: Vec<PathBuf>,
        #[arg(long, default_value = "toolbench.toml")]
        config: PathBuf,
        #[arg(long)]
        out: Option<PathBuf>,
        /// Use this run for an endpoint in the verdict: NAME=RUN_ID (repeatable).
        #[arg(long = "run", value_parser = parse_pick)]
        picks: Vec<(String, String)>,
        /// Use each endpoint's newest run in the verdict.
        #[arg(long)]
        latest: bool,
    },
    /// Fake server that saves a product's first request with tools as a snapshot.
    Capture {
        #[arg(long, default_value = "127.0.0.1:18080")]
        listen: std::net::SocketAddr,
        #[arg(long)]
        product: String,
        #[arg(long)]
        source: String,
        #[arg(long)]
        out: PathBuf,
        /// Save without asking (after reviewing the printed summary).
        #[arg(long)]
        accept: bool,
        /// Skip redaction of home paths, user name, emails and IPs.
        #[arg(long)]
        no_redact: bool,
        /// Allow a non-loopback --listen address.
        #[arg(long)]
        allow_remote: bool,
    },
}

fn parse_pick(s: &str) -> Result<(String, String), String> {
    s.split_once('=')
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .ok_or_else(|| format!("expected NAME=RUN_ID, got `{s}`"))
}

/// `--run` picks as a selection; naming one endpoint twice is an error.
fn selection(picks: Vec<(String, String)>, latest: bool) -> Result<report::RunSelection> {
    let mut map = std::collections::BTreeMap::new();
    for (name, id) in picks {
        if let Some(prev) = map.insert(name.clone(), id) {
            anyhow::bail!("--run {name} given twice (already `{prev}`)");
        }
    }
    Ok(report::RunSelection { picks: map, latest })
}

#[tokio::main]
async fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Tools { snapshot } => {
            let snap = snapshot::Snapshot::load(&snapshot)?;
            for t in &snap.tools {
                let props: Vec<&str> = t
                    .input_schema
                    .get("properties")
                    .and_then(|p| p.as_object())
                    .map(|o| o.keys().map(String::as_str).collect())
                    .unwrap_or_default();
                let required: Vec<&str> = t
                    .input_schema
                    .get("required")
                    .and_then(|r| r.as_array())
                    .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
                    .unwrap_or_default();
                println!(
                    "{}  required: [{}]  all: [{}]",
                    t.name,
                    required.join(", "),
                    props.join(", ")
                );
            }
        }
        Command::Run {
            config,
            endpoint,
            scenarios,
            snapshots,
            repeat,
            timeout,
            out,
        } => {
            anyhow::ensure!(repeat > 0, "--repeat must be at least 1");
            let cfg = config::Config::load(&config)?;
            let ep = cfg.endpoint(&endpoint)?;
            let snaps = snapshot::load_dir(&snapshots)?;
            let scs = scenario::load_dir(&scenarios, &snaps)?;
            // Check reachability before touching `--out`, so an unreachable
            // endpoint never truncates or replaces earlier results.
            toolbench::client::reachable(&reqwest::Client::new(), ep).await?;
            if let Some(parent) = out.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut file = std::fs::File::create(&out)
                .with_context(|| format!("creating {}", out.display()))?;
            let opts = runner::RunOptions {
                repeat,
                timeout: Duration::from_secs(timeout),
            };
            let summary = runner::run_endpoint(ep, &scs, &snaps, &opts, &mut file).await?;
            if summary.grammar_unsupported {
                eprintln!(
                    "{endpoint}: the server rejected the grammar request (HTTP 400); see {}",
                    out.display()
                );
            } else {
                eprintln!(
                    "{endpoint}: {} replies → {}",
                    summary.replies,
                    out.display()
                );
            }
        }
        Command::Rescore {
            input,
            out,
            scenarios,
            snapshots,
        } => {
            let snaps = snapshot::load_dir(&snapshots)?;
            let scs = scenario::load_dir(&scenarios, &snaps)?;
            let lines = runner::rescore(&result::read_lines_tagged(&input)?, &scs, &snaps)?;
            if let Some(parent) = out.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut file = std::fs::File::create(&out)?;
            for line in &lines {
                result::write_line(&mut file, line)?;
            }
        }
        Command::Report {
            inputs,
            config,
            out,
            picks,
            latest,
        } => {
            let cfg = config::Config::load(&config)?;
            let mut lines = Vec::new();
            for path in &inputs {
                lines.extend(result::read_lines_tagged(path)?);
            }
            let stats = report::aggregate(&lines);
            let sel = selection(picks, latest)?;
            let assessment = cfg
                .verdict
                .as_ref()
                .map(|v| report::assess(&stats, v, &sel));
            let md = report::render(&stats, assessment.as_ref());
            match out {
                Some(path) => {
                    if let Some(parent) = path.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    std::fs::write(&path, md)?
                }
                None => print!("{md}"),
            }
        }
        Command::Capture {
            listen,
            product,
            source,
            out,
            accept,
            no_redact,
            allow_remote,
        } => {
            toolbench::capture::check_listen(listen, allow_remote)?;
            let opts = toolbench::capture::CaptureOptions {
                accept,
                redact: !no_redact,
                interactive: std::io::IsTerminal::is_terminal(&std::io::stdin()),
            };
            toolbench::capture::serve(listen, product, source, out, opts).await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_repeated_run_pick_is_rejected() {
        let picks = vec![
            ("a".to_string(), "1".to_string()),
            ("b".to_string(), "2".to_string()),
        ];
        assert_eq!(selection(picks, false).unwrap().picks.len(), 2);
        let dup = vec![
            ("a".to_string(), "1".to_string()),
            ("a".to_string(), "2".to_string()),
        ];
        let err = selection(dup, false).unwrap_err().to_string();
        assert!(err.contains("--run a"), "{err}");
    }
}
