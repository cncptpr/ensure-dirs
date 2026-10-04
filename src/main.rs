//! ensure-dirs: ensure directories exist with the configured owner, group
//! and mode, using an optional cached state to minimize filesystem
//! interaction between runs.

mod config;
mod ops;
mod state;

use config::Config;
use state::{Action, State};
use std::collections::HashMap;
use std::path::PathBuf;

const USAGE: &str = "\
ensure-dirs - ensure directories exist with the configured owner, group and mode

USAGE:
    ensure-dirs --config <file> [OPTIONS]

OPTIONS:
    --config <file>   JSON configuration (in Nix: builtins.toJSON ensureDirs)
    --check-config    Parse and validate the configuration, then exit
                      (no root required, no filesystem interaction)
    --force           Ignore any cached state and check every directory
    --dry-run         Only report what would be done (does not require root)
    --verbose, -v     Log every directory, including unchanged ones
    --help, -h        Print this help
    --version, -V     Print the version
";

#[derive(Debug)]
struct Args {
    config: Option<String>,
    check_config: bool,
    force: bool,
    dry_run: bool,
    verbose: bool,
    help: bool,
    version: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        config: None,
        check_config: false,
        force: false,
        dry_run: false,
        verbose: false,
        help: false,
        version: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--config" => {
                args.config = Some(it.next().ok_or("--config requires a value")?);
            }
            "--check-config" => args.check_config = true,
            "--force" => args.force = true,
            "--dry-run" => args.dry_run = true,
            "--verbose" | "-v" => args.verbose = true,
            "--help" | "-h" => args.help = true,
            "--version" | "-V" => args.version = true,
            other if other.starts_with("--config=") => {
                args.config = Some(other["--config=".len()..].to_string());
            }
            other => return Err(format!("unknown argument '{other}'")),
        }
    }
    Ok(args)
}

fn main() {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("ensure-dirs: error: {e}");
            eprint!("{USAGE}");
            std::process::exit(2);
        }
    };
    if args.help {
        print!("{USAGE}");
        return;
    }
    if args.version {
        println!("ensure-dirs {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    if let Err(e) = run(&args) {
        eprintln!("ensure-dirs: error: {e}");
        std::process::exit(1);
    }
}

fn run(args: &Args) -> Result<(), String> {
    let config_path = args
        .config
        .as_deref()
        .ok_or_else(|| format!("--config is required\n\n{USAGE}"))?;
    let raw = std::fs::read_to_string(config_path)
        .map_err(|e| format!("cannot read config {config_path}: {e}"))?;
    let cfg: Config = serde_json::from_str(&raw)
        .map_err(|e| format!("cannot parse config {config_path}: {e}"))?;
    let cfg = config::validate(cfg)?;

    if args.check_config {
        let n = cfg.dirs.len();
        println!(
            "ensure-dirs: configuration OK ({n} director{})",
            if n == 1 { "y" } else { "ies" }
        );
        return Ok(());
    }

    if !args.dry_run && !ops::is_root() {
        return Err("must run as root (or use --dry-run)".to_string());
    }

    // Load the cached state (unless caching is disabled or bypassed).
    let cache_path = PathBuf::from(&cfg.cache_dir).join("state.json");
    let use_cache = cfg.use_cache && !args.force;
    let mut previous = State::empty();
    if use_cache {
        match state::load(&cache_path) {
            Ok(Some(s)) => previous = s,
            Ok(None) => {}
            Err(e) => eprintln!("ensure-dirs: warning: ignoring cache: {e}"),
        }
    }

    // Diff: unchanged entries are skipped without touching the filesystem.
    let plan = state::plan(&cfg.dirs, &previous);
    let mut new_state = State::empty();
    let mut failures: Vec<String> = Vec::new();
    let mut lookups: HashMap<(String, String), Result<(u32, u32), String>> = HashMap::new();
    let (mut ensured, mut skipped) = (0usize, 0usize);

    for work in &plan {
        if work.action == Action::Skip {
            skipped += 1;
            if args.verbose {
                eprintln!("ensure-dirs: unchanged: {}", work.path);
            }
            new_state.dirs.insert(work.path.clone(), work.spec.clone());
            continue;
        }

        // Resolve user/group (cached across directories) before touching anything.
        let ids = lookups
            .entry((work.spec.user.clone(), work.spec.group.clone()))
            .or_insert_with_key(|(u, g)| ops::lookup_ids(u, g))
            .clone();
        let (uid, gid) = match ids {
            Ok(ids) => ids,
            Err(e) => {
                failures.push(format!("{}: {e}", work.path));
                continue;
            }
        };

        if args.dry_run {
            eprintln!(
                "ensure-dirs: would ensure {} ({}:{}, mode={})",
                work.path,
                work.spec.user,
                work.spec.group,
                work.spec.mode.as_deref().unwrap_or("-")
            );
            ensured += 1;
            continue;
        }

        // Mode was validated above, so this cannot fail in practice.
        let mode = work
            .spec
            .mode
            .as_deref()
            .map(config::parse_mode)
            .transpose()
            .map_err(|e| format!("{}: {e}", work.path))?;

        match ops::ensure_dir(&work.path, uid, gid, mode) {
            Ok(()) => {
                ensured += 1;
                if args.verbose {
                    eprintln!("ensure-dirs: ensured: {}", work.path);
                }
                new_state.dirs.insert(work.path.clone(), work.spec.clone());
            }
            Err(e) => failures.push(format!("{}: {e}", work.path)),
        }
    }

    // Persist the new state (only if caching is on, something actually
    // changed, and this is not a dry run). Failed directories are left out
    // of the state so the next run retries them.
    if cfg.use_cache && !args.dry_run && new_state != previous {
        if let Err(e) = state::store(&cache_path, &new_state) {
            failures.push(format!("cache: {e}"));
        }
    }

    let verb = if args.dry_run { "to ensure" } else { "ensured" };
    let summary = format!(
        "{ensured} {verb}, {skipped} unchanged, {} failed",
        failures.len()
    );
    if !failures.is_empty() {
        for f in &failures {
            eprintln!("ensure-dirs: error: {f}");
        }
        return Err(summary);
    }
    println!("ensure-dirs: {summary}");
    Ok(())
}
