mod auth;
mod client;
mod discover;
mod journal;

use clap::Parser;
use client::Session;
use journal::{HostError, LogLevel, LogLine};

/// Give it a request ID or resource UUID; it SSHes into the hosts running
/// your OpenStack services, greps their journals, and prints every
/// matching log line from every service in one merged, color-coded
/// timeline.
#[derive(Parser, Debug)]
#[clap(
    name = env!("CARGO_PKG_NAME"),
    version = env!("CARGO_PKG_VERSION"),
    about = env!("CARGO_PKG_DESCRIPTION"),
)]
struct Args {
    /// Request ID (req-...) or resource UUID (server, volume, network, ...)
    /// to search for
    id: String,

    /// How far back to search, in journalctl's own time syntax
    /// (e.g. "10 minutes ago", "2026-08-26 14:00:00")
    #[arg(long, default_value = "10 minutes ago")]
    since: String,

    /// End of the search window, in journalctl's own time syntax
    #[arg(long, default_value = "now")]
    until: String,

    /// Search these hosts instead of auto-discovering via Nova/Neutron
    /// (comma-separated)
    #[arg(long, value_delimiter = ',')]
    hosts: Vec<String>,

    /// Hide log lines below this level (debug/info/warning/error).
    /// Lines ostrail couldn't classify are always shown regardless.
    #[arg(long, default_value = "warning", value_parser = parse_min_level)]
    min_level: LogLevel,
}

fn parse_min_level(s: &str) -> Result<LogLevel, String> {
    match s.to_lowercase().as_str() {
        "debug" => Ok(LogLevel::Debug),
        "info" => Ok(LogLevel::Info),
        "warning" | "warn" => Ok(LogLevel::Warning),
        "error" => Ok(LogLevel::Error),
        other => Err(format!(
            "'{other}' isn't a level (expected debug, info, warning, or error)"
        )),
    }
}

fn main() {
    let args = Args::parse();

    let hosts = if !args.hosts.is_empty() {
        args.hosts.clone()
    } else {
        discover_hosts_or_exit()
    };

    eprintln!(
        "Searching {} host(s) for '{}' (since {}, until {}) ...",
        hosts.len(),
        args.id,
        args.since,
        args.until
    );

    let handles: Vec<_> = hosts
        .into_iter()
        .map(|host| {
            let id = args.id.clone();
            let since = args.since.clone();
            let until = args.until.clone();
            std::thread::spawn(move || {
                let result = journal::fetch_from_host(&host, &id, &since, &until);
                (host, result)
            })
        })
        .collect();

    let mut all_lines: Vec<LogLine> = Vec::new();
    let mut errors: Vec<HostError> = Vec::new();
    for handle in handles {
        let (host, result) = handle.join().expect("worker thread panicked");
        match result {
            Ok(lines) => all_lines.extend(lines),
            Err(reason) => errors.push(HostError { host, reason }),
        }
    }

    all_lines.retain(|l| l.level >= args.min_level);
    all_lines.sort_by_key(|l| l.timestamp_micros);

    for err in &errors {
        eprintln!(
            "warning: couldn't fetch logs from {}: {}",
            err.host, err.reason
        );
    }

    if all_lines.is_empty() {
        println!("No matching log lines found.");
        if args.min_level != LogLevel::Debug {
            println!("(try --min-level debug to include everything, not just warning+)");
        }
        return;
    }

    println!("{} matching line(s):\n", all_lines.len());
    for line in &all_lines {
        println!("{}", format_line(line));
    }
}

fn discover_hosts_or_exit() -> Vec<String> {
    let cloud_auth = match auth::discover() {
        Some(a) => a,
        None => {
            eprintln!(
                "Error: no OpenStack credentials found. Source your openrc.sh (or set \
                 OS_* env vars / clouds.yaml), or pass --hosts explicitly."
            );
            std::process::exit(1);
        }
    };

    eprintln!(
        "Logging in to {} to discover hosts ...",
        cloud_auth.auth_url
    );
    let session = match Session::login(&cloud_auth) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Error: login failed: {e}");
            std::process::exit(1);
        }
    };

    let hosts = discover::discover_hosts(&session);
    if hosts.is_empty() {
        eprintln!(
            "Error: no hosts found via Nova/Neutron. Pass --hosts explicitly \
             (e.g. --hosts controller,compute-1)."
        );
        std::process::exit(1);
    }
    hosts
}

const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const RED: &str = "\x1b[31m";
const YELLOW: &str = "\x1b[33m";
const CYAN: &str = "\x1b[36m";
const DIM: &str = "\x1b[2m";

fn level_style(level: LogLevel) -> (&'static str, &'static str) {
    match level {
        LogLevel::Error => (RED, "ERROR"),
        LogLevel::Warning => (YELLOW, "WARN "),
        LogLevel::Info => (DIM, "INFO "),
        LogLevel::Debug => (DIM, "DEBUG"),
        LogLevel::Unknown => (CYAN, "?    "),
    }
}

fn format_line(line: &LogLine) -> String {
    let (color, label) = level_style(line.level);
    let time = format_timestamp(line.timestamp_micros);
    format!(
        "{DIM}{time}{RESET}  {BOLD}{:<12}{RESET} {:<24} {color}{label}{RESET}  {}",
        truncate(&line.host, 12),
        truncate(&line.unit, 24),
        line.message,
    )
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(max.saturating_sub(1)).collect();
        t.push('…');
        t
    }
}

fn format_timestamp(micros: u64) -> String {
    let secs = (micros / 1_000_000) as i64;
    match time::OffsetDateTime::from_unix_timestamp(secs) {
        Ok(t) => format!(
            "{:02}-{:02} {:02}:{:02}:{:02}",
            u8::from(t.month()),
            t.day(),
            t.hour(),
            t.minute(),
            t.second(),
        ),
        Err(_) => "??-?? ??:??:??".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_min_level_accepts_known_levels_case_insensitively() {
        assert_eq!(parse_min_level("Warning").unwrap(), LogLevel::Warning);
        assert_eq!(parse_min_level("error").unwrap(), LogLevel::Error);
        assert_eq!(parse_min_level("DEBUG").unwrap(), LogLevel::Debug);
    }

    #[test]
    fn parse_min_level_rejects_unknown_input() {
        assert!(parse_min_level("critical").is_err());
    }

    #[test]
    fn truncate_leaves_short_strings_alone() {
        assert_eq!(truncate("short", 12), "short");
    }

    #[test]
    fn truncate_shortens_and_marks_long_strings() {
        let t = truncate("this-is-a-very-long-hostname", 12);
        assert_eq!(t.chars().count(), 12);
        assert!(t.ends_with('…'));
    }

    #[test]
    fn format_timestamp_renders_a_known_instant() {
        // 2026-08-26T14:08:11Z
        let micros = 1_787_753_291_000_000u64;
        assert_eq!(format_timestamp(micros), "08-26 14:08:11");
    }
}
