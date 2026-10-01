//! kimi-statusline: a status line command for Kimi Code CLI (>= 0.30.0).
//!
//! The TUI spawns `[status_line].command` at most once a second with a JSON
//! snapshot on stdin, kills it after 300ms, and renders the first stdout line
//! in place of footer line 1. We add what the snapshot lacks — session token
//! usage, cache hit rate, sub-agent usage, swarm/tower modes, thinking effort,
//! goal and task badges, git diff stats, the PR badge, /dance — from the
//! session files on disk.

mod collect;
mod config;
mod install;
mod kimi_config;
mod paths;
mod payload;
mod probe;
mod quota;
mod render;
mod session;
mod themes;
mod tui;
mod update;

use clap::{Parser, Subcommand};
use config::{Config, SegmentId};
use std::io::IsTerminal;
use std::time::{Duration, Instant};

#[derive(Parser)]
#[command(
    name = "kimi-statusline",
    version,
    about = "High-performance status line for Kimi Code CLI"
)]
struct Cli {
    /// Render with a theme instead of the saved config
    #[arg(short, long, global = true)]
    theme: Option<String>,
    /// Render width for status line mode (default: detect the terminal)
    #[arg(long, global = true)]
    width: Option<usize>,
    /// Running as the kimi-statusline plugin: stand down once the plugin is
    /// disabled or removed
    #[arg(long, hide = true)]
    plugin: bool,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Interactive configurator (running with no arguments opens the menu)
    Config,
    /// Set this binary as [status_line].command in tui.toml
    Install {
        /// Command to write instead of this binary's path
        #[arg(long)]
        command: Option<String>,
        /// Replace an existing non-kimi-statusline command
        #[arg(long)]
        force: bool,
        /// Plugin hook mode: silent, and the command polices the plugin state
        #[arg(long)]
        plugin: bool,
    },
    /// Remove our [status_line].command from tui.toml
    Uninstall,
    /// Write the config file from a theme (default: kimi)
    Init {
        #[arg(long)]
        force: bool,
    },
    /// List themes
    Themes,
    /// Fetch plan quota now and print it (5h / 7d / monthly)
    Quota,
    /// Check GitHub for a newer release and install it
    Update {
        /// Only report whether an update is available
        #[arg(long)]
        check: bool,
    },
    /// Background quota refresh, spawned by the status line
    #[command(hide = true)]
    FetchQuota,
    /// Background git status probe, spawned by the status line
    #[command(hide = true)]
    ProbeGit {
        #[arg(long)]
        cwd: String,
    },
    /// Render for the newest session in a directory, without the TUI
    Preview {
        /// Working directory of the session (default: current dir)
        #[arg(long)]
        cwd: Option<String>,
        /// Session id (default: newest session for --cwd)
        #[arg(long)]
        session: Option<String>,
        /// Render width (default: full line)
        #[arg(long)]
        width: Option<usize>,
    },
}

fn main() {
    let cli = Cli::parse();
    let result = match cli.cmd {
        Some(Cmd::Config) => tui::run_configurator(),
        Some(Cmd::Install {
            command,
            force,
            plugin,
        }) => install_cmd(command, force, plugin),
        Some(Cmd::Uninstall) => install::uninstall().map(|changed| {
            if changed {
                println!("Removed kimi-statusline from tui.toml; run /reload-tui to apply.");
            } else {
                println!("status_line.command is not kimi-statusline; nothing to do.");
            }
        }),
        Some(Cmd::Init { force }) => init(cli.theme.as_deref(), force),
        Some(Cmd::Themes) => {
            for name in themes::list() {
                let desc = themes::BUILTIN
                    .iter()
                    .find(|(n, _)| *n == name)
                    .map_or("saved theme", |(_, d)| d);
                println!("{name:24} {desc}");
            }
            Ok(())
        }
        Some(Cmd::Quota) => quota::fetch_and_store()
            .map(|q| {
                let show = |name: &str, e: &Option<quota::Entry>| match e {
                    Some(e) => println!(
                        "{name:8} {:>5.1}%   resets {}",
                        e.used_ratio * 100.0,
                        e.reset_at.as_deref().unwrap_or("-")
                    ),
                    None => println!("{name:8} -"),
                };
                show("5h", &q.limit_5h);
                show("7d", &q.limit_7d);
                show("monthly", &q.month);
            })
            .map_err(|e| match quota::last_error() {
                Some(last) if last != e => format!("{e} (previous: {last})"),
                _ => e,
            }),
        Some(Cmd::Update { check }) => update_cmd(check),
        Some(Cmd::FetchQuota) => {
            let _ = quota::fetch_and_store();
            // off the status line's clock: a good time to tidy up
            paths::sweep_cache();
            Ok(())
        }
        Some(Cmd::ProbeGit { cwd }) => {
            let _ = probe::refresh_git(&cwd);
            Ok(())
        }
        Some(Cmd::Preview {
            cwd,
            session,
            width,
        }) => {
            let cwd = cwd.unwrap_or_else(current_dir);
            let payload = collect::sample_payload(&cwd, session);
            let ctx = collect::collect(payload, load_config(cli.theme.as_deref()), false);
            println!("{}", render::render(&ctx, width.or(cli.width)));
            Ok(())
        }
        None if std::io::stdin().is_terminal() => tui::run_menu(),
        None => {
            run_statusline(cli.theme.as_deref(), cli.plugin, cli.width);
            Ok(())
        }
    };
    if let Err(e) = result {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn update_cmd(check_only: bool) -> Result<(), String> {
    let latest = update::check_now()?;
    if !update::is_newer(&latest, update::CURRENT) {
        println!("kimi-statusline {} is up to date", update::CURRENT);
        return Ok(());
    }
    println!("Update available: {} → {latest}", update::CURRENT);
    if check_only {
        return Ok(());
    }
    let kind = update::install_kind();
    if let Some(how) = update::manual_instructions(&kind) {
        println!("This copy was {how}");
        return Ok(());
    }
    let update::InstallKind::Standalone(exe) = kind else {
        unreachable!()
    };
    let v = update::install(&latest, &exe)?;
    println!("Updated {} to {v} (checksum verified)", exe.display());
    println!("The status line uses it on its next refresh.");
    Ok(())
}

fn current_dir() -> String {
    std::env::current_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn load_config(theme: Option<&str>) -> Config {
    match theme {
        Some(t) => themes::get(t),
        None => Config::load(),
    }
}

fn install_cmd(command: Option<String>, force: bool, plugin: bool) -> Result<(), String> {
    match install::install(command, force, plugin) {
        // hook stdout may end up in model context: say nothing
        _ if plugin => Ok(()),
        Ok(o) => {
            if o.changed {
                println!("Installed: [status_line].command = {:?}", o.command);
                println!("Run /reload-tui in Kimi Code (or restart it) to apply.");
            } else {
                println!("Already installed.");
            }
            Ok(())
        }
        Err(e) => Err(e),
    }
}

fn init(theme: Option<&str>, force: bool) -> Result<(), String> {
    let path = config::config_path();
    if path.exists() && !force {
        println!(
            "{} already exists (use --force to overwrite)",
            path.display()
        );
        return Ok(());
    }
    themes::get(theme.unwrap_or("kimi")).save()?;
    println!("Wrote {}", path.display());
    Ok(())
}

/// Leave this much of the TUI's 300ms for rendering and exit.
const BUDGET: Duration = Duration::from_millis(200);

fn run_statusline(theme: Option<&str>, plugin: bool, width_flag: Option<usize>) {
    let started = Instant::now();
    let payload = payload::read_stdin(Duration::from_millis(150));
    // any panic still prints a line: a nonzero exit would make the TUI
    // freeze on the previous output with no hint of what went wrong
    let line = std::panic::catch_unwind(|| {
        let mut config = load_config(theme);
        if plugin && install::plugin_inactive() {
            // The plugin was disabled or removed: take our command out of
            // tui.toml so the next /reload-tui gets the built-in footer, and
            // until then render the built-in look without the usage half
            // instead of freezing on a stale line.
            paths::debug("plugin inactive; removing tui.toml command");
            let _ = install::uninstall();
            config = themes::get("kimi");
            config
                .segments
                .retain(|s| !matches!(s.id, SegmentId::Usage | SegmentId::Subagent));
        }
        let width = if let Some(w) = width_flag {
            Some(w)
        } else if config.style.width > 0 {
            Some(config.style.width)
        } else if started.elapsed() < BUDGET {
            // the footer sits inside a one-column gutter on each side
            probe::terminal_width().map(|w| w.saturating_sub(2).max(1))
        } else {
            None
        };
        let ctx = collect::collect(payload, config, true);
        render::render(&ctx, width)
    })
    .unwrap_or_else(|_| "kimi-statusline: error (see kimi-statusline-debug.log)".into());
    println!("{line}");
    paths::debug(&format!("done in {}ms", started.elapsed().as_millis()));
}
