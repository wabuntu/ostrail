use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogLevel {
    Debug,
    Info,
    Warning,
    Error,
    Unknown,
}

impl LogLevel {
    fn from_first_word(word: &str) -> LogLevel {
        match word {
            "TRACE" | "DEBUG" => LogLevel::Debug,
            "INFO" | "AUDIT" => LogLevel::Info,
            "WARNING" | "WARN" => LogLevel::Warning,
            "ERROR" | "CRITICAL" => LogLevel::Error,
            _ => LogLevel::Unknown,
        }
    }
}

#[derive(Debug, Clone)]
pub struct LogLine {
    pub timestamp_micros: u64,
    pub host: String,
    pub unit: String,
    pub level: LogLevel,
    pub message: String,
}

/// A host ostrail couldn't get anything from, and why - reported
/// separately from `LogLine`s so a quiet host (no matches) and a
/// broken one (no SSH, no sudo, no journalctl) never look the same.
#[derive(Debug)]
pub struct HostError {
    pub host: String,
    pub reason: String,
}

/// SSHes to `host` and pulls its entire systemd journal for the given
/// time window, then filters for `search_id` *after* decoding each
/// line - no assumption about which unit name any OpenStack service
/// runs under, since that varies too much across DevStack/RDO/
/// Ubuntu-packaged deployments to guess reliably.
///
/// The filtering can't happen server-side with a plain `grep` on the
/// raw JSON: journald's JSON encoder represents `MESSAGE` as an array
/// of byte values instead of a string whenever the line contains raw
/// control bytes, which happens in practice because some OpenStack
/// services colorize their log output with ANSI escapes even when
/// journald - not a real terminal - is capturing them. A search string
/// that's plainly visible in the decoded text (e.g. a UUID) never
/// appears as that substring in the *raw* JSON for such a line, since
/// each character is spelled out as a separate decimal byte value
/// instead. So every line in the window gets decoded first, the same
/// way `parse_journal_json_line` always has, and the ID is matched
/// against that decoded text.
pub fn fetch_from_host(
    host: &str,
    search_id: &str,
    since: &str,
    until: &str,
) -> Result<Vec<LogLine>, String> {
    let remote_cmd = format!(
        "sudo journalctl -o json --since {} --until {}",
        shell_quote(since),
        shell_quote(until),
    );

    let output = std::process::Command::new("ssh")
        .arg("-o")
        .arg("ConnectTimeout=10")
        .arg("-o")
        .arg("BatchMode=yes")
        .arg(host)
        .arg(&remote_cmd)
        .output()
        .map_err(|e| format!("couldn't run ssh: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "ssh to {host} failed: {}",
            stderr.trim().lines().last().unwrap_or("unknown error")
        ));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut lines = Vec::new();
    for raw_line in stdout.lines() {
        if let Some(parsed) = parse_journal_json_line(raw_line, host)
            && parsed.message.contains(search_id)
        {
            lines.push(parsed);
        }
    }
    Ok(lines)
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Parses one `journalctl -o json` line. `MESSAGE` comes back as a plain
/// JSON string when it's clean UTF-8, but as a JSON array of byte values
/// when it contains raw bytes journald won't put in a string - which
/// happens in practice, since some OpenStack services colorize their log
/// output with ANSI escapes even when journald (not a real terminal) is
/// capturing them. Both shapes are handled, and the ANSI codes are
/// stripped either way so the message reads cleanly.
fn parse_journal_json_line(raw_line: &str, fallback_host: &str) -> Option<LogLine> {
    let v: Value = serde_json::from_str(raw_line).ok()?;

    let timestamp_micros = v
        .get("__REALTIME_TIMESTAMP")
        .and_then(|t| t.as_str())
        .and_then(|t| t.parse::<u64>().ok())?;

    let host = v
        .get("_HOSTNAME")
        .and_then(|h| h.as_str())
        .unwrap_or(fallback_host)
        .to_string();

    let unit = v
        .get("SYSLOG_IDENTIFIER")
        .or_else(|| v.get("_SYSTEMD_UNIT"))
        .or_else(|| v.get("_COMM"))
        .and_then(|u| u.as_str())
        .unwrap_or("-")
        .to_string();

    let message = strip_ansi(&decode_message(v.get("MESSAGE")?));
    let level = message
        .split_whitespace()
        .next()
        .map(LogLevel::from_first_word)
        .unwrap_or(LogLevel::Unknown);

    Some(LogLine {
        timestamp_micros,
        host,
        unit,
        level,
        message,
    })
}

fn decode_message(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Array(bytes) => {
            let raw: Vec<u8> = bytes
                .iter()
                .filter_map(|b| b.as_u64())
                .map(|b| b as u8)
                .collect();
            String::from_utf8_lossy(&raw).into_owned()
        }
        _ => String::new(),
    }
}

/// Strips `ESC [ ... letter` CSI sequences (SGR color codes, the only
/// kind oslo.log emits) so a message that came through as a byte array
/// reads as plain text instead of `\x1b[01;36mtext\x1b[00m`.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_plain_string_message() {
        let raw = r#"{"__REALTIME_TIMESTAMP":"1787753290293534","_HOSTNAME":"devstack","SYSLOG_IDENTIFIER":"devstack@n-api.service","MESSAGE":"DEBUG nova.api something happened"}"#;
        let line = parse_journal_json_line(raw, "fallback").unwrap();
        assert_eq!(line.timestamp_micros, 1787753290293534);
        assert_eq!(line.host, "devstack");
        assert_eq!(line.unit, "devstack@n-api.service");
        assert_eq!(line.level, LogLevel::Debug);
        assert_eq!(line.message, "DEBUG nova.api something happened");
    }

    #[test]
    fn decodes_a_byte_array_message_and_strips_ansi_color_codes() {
        // "\x1b[01;36mERROR\x1b[00m nova.scheduler boom"
        let mut bytes: Vec<u8> = vec![0x1b, b'[', b'0', b'1', b';', b'3', b'6', b'm'];
        bytes.extend_from_slice(b"ERROR");
        bytes.extend_from_slice(&[0x1b, b'[', b'0', b'0', b'm']);
        bytes.extend_from_slice(b" nova.scheduler boom");
        let arr: Vec<serde_json::Value> = bytes.into_iter().map(|b| (b as u64).into()).collect();
        let raw = serde_json::json!({
            "__REALTIME_TIMESTAMP": "1787753290293534",
            "_HOSTNAME": "devstack",
            "MESSAGE": arr,
        })
        .to_string();

        let line = parse_journal_json_line(&raw, "fallback").unwrap();
        assert_eq!(line.message, "ERROR nova.scheduler boom");
        assert_eq!(line.level, LogLevel::Error);
    }

    #[test]
    fn missing_timestamp_is_rejected() {
        let raw = r#"{"_HOSTNAME":"devstack","MESSAGE":"hi"}"#;
        assert!(parse_journal_json_line(raw, "fallback").is_none());
    }

    #[test]
    fn falls_back_to_provided_host_and_dash_unit() {
        let raw = r#"{"__REALTIME_TIMESTAMP":"123","MESSAGE":"hi there"}"#;
        let line = parse_journal_json_line(raw, "myhost").unwrap();
        assert_eq!(line.host, "myhost");
        assert_eq!(line.unit, "-");
        assert_eq!(line.level, LogLevel::Unknown);
    }

    #[test]
    fn shell_quote_escapes_embedded_single_quotes() {
        assert_eq!(shell_quote("abc"), "'abc'");
        assert_eq!(shell_quote("a'b"), r"'a'\''b'");
    }
}
