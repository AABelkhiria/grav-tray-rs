use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

const QUOTA_RPC_PATH: &str =
    "/exa.language_server_pb.LanguageServerService/RetrieveUserQuotaSummary";
const LOG_PORT_PREFIX: &str = "Language server listening on random port at ";
const LOG_PORT_SUFFIX: &str = " for HTTP";

#[derive(Clone, Debug, Deserialize)]
pub struct QuotaResponseEnvelope {
    pub response: QuotaSummary,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaSummary {
    pub groups: Vec<QuotaGroup>,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaGroup {
    pub display_name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub buckets: Vec<QuotaBucket>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaBucket {
    pub bucket_id: String,
    pub display_name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub window: Option<String>,
    #[serde(default)]
    pub remaining_fraction: Option<f64>,
    #[serde(default)]
    pub disabled: Option<bool>,
    #[serde(default)]
    pub reset_time: Option<String>,
}

impl QuotaBucket {
    pub fn is_enabled(&self) -> bool {
        self.disabled != Some(true)
    }

    pub fn percent(&self) -> Option<u8> {
        self.remaining_fraction
            .map(|fraction| (fraction.clamp(0.0, 1.0) * 100.0).round() as u8)
    }

    pub fn reset_label(&self, now: SystemTime) -> String {
        let Some(reset_time) = self.reset_time.as_deref() else {
            return "Reset time unavailable".to_owned();
        };
        let Ok(reset_at) = DateTime::parse_from_rfc3339(reset_time) else {
            return "Reset time unavailable".to_owned();
        };
        let now: DateTime<Utc> = now.into();
        let seconds = (reset_at.with_timezone(&Utc) - now).num_seconds();
        format_reset_duration(seconds)
    }
}

pub fn selection_key(group: &QuotaGroup, bucket: &QuotaBucket) -> String {
    format!("{}|{}", group.display_name, bucket.bucket_id)
}

pub fn enabled_buckets(summary: &QuotaSummary) -> Vec<(&QuotaGroup, &QuotaBucket)> {
    summary
        .groups
        .iter()
        .flat_map(|group| {
            group
                .buckets
                .iter()
                .filter(|bucket| bucket.is_enabled())
                .map(move |bucket| (group, bucket))
        })
        .collect()
}

pub fn selected_fraction(summary: &QuotaSummary, selection: &str) -> Option<f64> {
    let buckets = enabled_buckets(summary);
    buckets
        .iter()
        .find(|(group, bucket)| selection_key(group, bucket) == selection)
        .or_else(|| buckets.first())
        .and_then(|(_, bucket)| bucket.remaining_fraction)
}

pub fn validate_selection(summary: &QuotaSummary, selection: &mut String) {
    let buckets = enabled_buckets(summary);
    if buckets
        .iter()
        .any(|(group, bucket)| selection_key(group, bucket) == *selection)
    {
        return;
    }
    *selection = buckets
        .first()
        .map(|(group, bucket)| selection_key(group, bucket))
        .unwrap_or_default();
}

pub fn fetch_quota(
    home_directory: &Path,
    preferred_port: Option<u16>,
) -> Result<(QuotaSummary, u16), String> {
    if let Some(port) = preferred_port {
        if let Ok(summary) = fetch_from_port(port) {
            return Ok((summary, port));
        }
    }

    let ports = candidate_http_ports(home_directory);
    if ports.is_empty() {
        return Err("No Antigravity sessions were found. Open agy and sign in.".to_owned());
    }

    for port in ports
        .into_iter()
        .filter(|port| Some(*port) != preferred_port)
    {
        if let Ok(summary) = fetch_from_port(port) {
            return Ok((summary, port));
        }
    }

    Err("No authenticated agy session is reachable. Open or restart agy.".to_owned())
}

pub fn candidate_http_ports(home_directory: &Path) -> Vec<u16> {
    let log_directory = home_directory
        .join(".gemini")
        .join("antigravity-cli")
        .join("log");
    candidate_http_ports_in(&log_directory)
}

fn candidate_http_ports_in(log_directory: &Path) -> Vec<u16> {
    let Ok(entries) = fs::read_dir(log_directory) else {
        return Vec::new();
    };

    let mut logs: Vec<(SystemTime, PathBuf)> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            (path.extension().and_then(|value| value.to_str()) == Some("log")).then(|| {
                let modified = entry
                    .metadata()
                    .and_then(|metadata| metadata.modified())
                    .unwrap_or(SystemTime::UNIX_EPOCH);
                (modified, path)
            })
        })
        .collect();
    logs.sort_unstable_by_key(|right| std::cmp::Reverse(right.0));

    let mut seen = HashSet::new();
    logs.into_iter()
        .take(20)
        .filter_map(|(_, path)| read_log_header(&path))
        .filter_map(|header| http_port(&header))
        .filter(|port| seen.insert(*port))
        .collect()
}

pub fn http_port(log_contents: &str) -> Option<u16> {
    log_contents.lines().find_map(|line| {
        let start = line.find(LOG_PORT_PREFIX)? + LOG_PORT_PREFIX.len();
        let remaining = &line[start..];
        let digit_count = remaining
            .bytes()
            .take_while(|byte| byte.is_ascii_digit())
            .count();
        if digit_count == 0 || &remaining[digit_count..] != LOG_PORT_SUFFIX {
            return None;
        }
        remaining[..digit_count].parse().ok()
    })
}

fn read_log_header(path: &Path) -> Option<String> {
    let file = File::open(path).ok()?;
    let mut buffer = String::new();
    file.take(8_192).read_to_string(&mut buffer).ok()?;
    Some(buffer)
}

fn fetch_from_port(port: u16) -> Result<QuotaSummary, String> {
    let url = format!("http://127.0.0.1:{port}{QUOTA_RPC_PATH}");
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(3))
        .timeout_read(Duration::from_secs(5))
        .timeout_write(Duration::from_secs(3))
        .build();
    let response = agent
        .post(&url)
        .set("Content-Type", "application/json")
        .set("Connect-Protocol-Version", "1")
        .send_string("{}")
        .map_err(|error| error.to_string())?;
    let envelope: QuotaResponseEnvelope = serde_json::from_reader(response.into_reader())
        .map_err(|error| format!("Antigravity returned an invalid quota response: {error}"))?;
    Ok(envelope.response)
}

fn format_reset_duration(seconds: i64) -> String {
    if seconds <= 0 {
        return "Reset due".to_owned();
    }

    let total_minutes = seconds / 60;
    let days = total_minutes / (24 * 60);
    let hours = (total_minutes % (24 * 60)) / 60;
    let minutes = total_minutes % 60;

    if days > 0 {
        if hours > 0 {
            format!("Resets in {days}d {hours}h")
        } else {
            format!("Resets in {days}d")
        }
    } else if hours > 0 {
        format!("Resets in {hours}h {minutes}m")
    } else {
        format!("Resets in {}m", minutes.max(1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::thread;

    #[test]
    fn extracts_http_port_from_lf_and_crlf_logs() {
        assert_eq!(
            http_port("before\nLanguage server listening on random port at 43123 for HTTP\nafter"),
            Some(43123)
        );
        assert_eq!(
            http_port("Language server listening on random port at 8080 for HTTP\r\n"),
            Some(8080)
        );
    }

    #[test]
    fn rejects_malformed_or_non_http_port_lines() {
        assert_eq!(http_port("random port at 123 for HTTP"), None);
        assert_eq!(
            http_port("Language server listening on random port at 123 for gRPC"),
            None
        );
        assert_eq!(
            http_port(
                "Language server listening on random port at 55528 for HTTPS (gRPC)\n\
                 Language server listening on random port at 55529 for HTTP"
            ),
            Some(55529)
        );
        assert_eq!(
            http_port("Language server listening on random port at nope for HTTP"),
            None
        );
    }

    #[test]
    fn clamps_and_rounds_percentages() {
        let bucket = |remaining_fraction| QuotaBucket {
            bucket_id: "id".to_owned(),
            display_name: "window".to_owned(),
            description: None,
            window: None,
            remaining_fraction,
            disabled: None,
            reset_time: None,
        };
        assert_eq!(bucket(Some(0.824)).percent(), Some(82));
        assert_eq!(bucket(Some(1.2)).percent(), Some(100));
        assert_eq!(bucket(Some(-0.1)).percent(), Some(0));
        assert_eq!(bucket(None).percent(), None);
    }

    #[test]
    fn formats_reset_countdowns() {
        assert_eq!(format_reset_duration(-1), "Reset due");
        assert_eq!(format_reset_duration(20), "Resets in 1m");
        assert_eq!(format_reset_duration(90 * 60), "Resets in 1h 30m");
        assert_eq!(format_reset_duration(49 * 60 * 60), "Resets in 2d 1h");
    }

    #[test]
    fn calls_connect_rpc_and_decodes_quota() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let (response_consumed_tx, response_consumed_rx) = mpsc::channel();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 2048];
            let bytes_read = stream.read(&mut request).unwrap();
            let request = String::from_utf8_lossy(&request[..bytes_read]);
            assert!(request.starts_with(&format!("POST {QUOTA_RPC_PATH} HTTP/1.1")));
            assert!(
                request
                    .to_ascii_lowercase()
                    .contains("connect-protocol-version: 1")
            );

            let body = r#"{"response":{"groups":[{"displayName":"Gemini","buckets":[{"bucketId":"five","displayName":"5-hour","remainingFraction":0.75}]}]}}"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
            stream.flush().unwrap();

            // Keep the peer alive until ureq has consumed the fixed-length body
            // and reset its socket timeouts. Closing sooner races that reset on
            // macOS and can make ureq fail with EINVAL.
            let _ = response_consumed_rx.recv();
        });

        let summary = fetch_from_port(port);
        response_consumed_tx.send(()).unwrap();
        server.join().unwrap();
        let summary = summary.unwrap();
        assert_eq!(summary.groups[0].display_name, "Gemini");
        assert_eq!(summary.groups[0].buckets[0].percent(), Some(75));
    }
}
