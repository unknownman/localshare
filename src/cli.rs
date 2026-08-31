use clap::{builder::styling::*, ArgAction, Parser};
use std::fmt;

/// Instantly share a local HTTP server with the internet.
#[derive(Parser, Debug)]
#[command(
    name = "localshare",
    version,
    about = "Instantly share a local HTTP server with the internet",
    after_help = "EXAMPLES:\n    localshare 3000                     Share local port 3000 instantly\n    localshare 127.0.0.1:8080           Share a specific local host and port\n    localshare 5173 -s my-preview       Request subdomain 'my-preview'\n    localshare 3000 --no-qr             Display URL without QR code\n    localshare 3000 --json              Output machine-readable JSON for scripting\n    localshare 3000 -r customrelay.io   Use a self-hosted custom relay",
    styles = Styles::styled()
        .header(AnsiColor::Yellow.on_default() | Effects::BOLD)
        .usage(AnsiColor::Yellow.on_default() | Effects::BOLD)
        .literal(AnsiColor::Green.on_default() | Effects::BOLD)
        .placeholder(AnsiColor::Cyan.on_default())
)]
pub struct Cli {
    /// Local port or host:port to share
    #[arg(value_parser = parse_target)]
    pub target: LocalTarget,

    /// Relay server address for public tunneling
    #[arg(
        short = 'r',
        long = "relay",
        default_value = "relay.localshare.dev",
        env = "LOCALSHARE_RELAY"
    )]
    pub relay: String,

    /// Request a custom subdomain prefix on the relay
    #[arg(short = 's', long = "subdomain", value_parser = validate_subdomain)]
    pub subdomain: Option<String>,

    /// Suppress the terminal QR code
    #[arg(long = "no-qr")]
    pub no_qr: bool,

    /// Output tunnel metadata as JSON for scripting
    #[arg(long = "json")]
    pub json: bool,

    /// Enable verbose logging (-v, -vv, -vvv for increasing verbosity)
    #[arg(short = 'v', long = "verbose", action = ArgAction::Count)]
    pub verbose: u8,

    /// Suppress non-error console output
    #[arg(short = 'q', long = "quiet")]
    pub quiet: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalTarget {
    pub host: String,
    pub port: u16,
}

impl fmt::Display for LocalTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.host, self.port)
    }
}

fn parse_target(s: &str) -> Result<LocalTarget, String> {
    let s = s.trim();

    if s.is_empty() {
        return Err("target cannot be empty".to_string());
    }

    // Try to parse as port-only (no colon present)
    if !s.contains(':') {
        let port = s.parse::<u16>().map_err(|_| {
            format!(
                "invalid target '{}': expected a port number (1-65535) or host:port",
                s
            )
        })?;
        if port == 0 {
            return Err("port must be between 1 and 65535".to_string());
        }
        return Ok(LocalTarget {
            host: "127.0.0.1".to_string(),
            port,
        });
    }

    // Parse as host:port — split on the last colon to handle IPv4 correctly
    let (host, port_str) = s
        .rsplit_once(':')
        .ok_or_else(|| format!("invalid target '{}': expected host:port", s))?;

    if host.is_empty() {
        return Err(format!("invalid target '{}': missing host before port", s));
    }

    let port = port_str.parse::<u16>().map_err(|_| {
        if port_str.is_empty() {
            format!("invalid target '{}': missing port after colon", s)
        } else {
            format!(
                "invalid port '{}': must be a number between 1 and 65535",
                port_str
            )
        }
    })?;

    if port == 0 {
        return Err("port must be between 1 and 65535".to_string());
    }

    Ok(LocalTarget {
        host: host.to_string(),
        port,
    })
}

fn validate_subdomain(s: &str) -> Result<String, String> {
    let s = s.trim();

    if s.is_empty() {
        return Err("subdomain cannot be empty".to_string());
    }

    if s.len() > 63 {
        return Err("subdomain must be 63 characters or less".to_string());
    }

    if s.starts_with('-') || s.ends_with('-') {
        return Err("subdomain cannot start or end with a hyphen".to_string());
    }

    if !s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err(
            "subdomain can only contain lowercase alphanumeric characters and hyphens".to_string(),
        );
    }

    Ok(s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── LocalTarget parsing ──────────────────────────────────────────

    #[test]
    fn port_only() {
        assert_eq!(
            parse_target("3000").unwrap(),
            LocalTarget {
                host: "127.0.0.1".into(),
                port: 3000,
            }
        );
    }

    #[test]
    fn port_only_8080() {
        assert_eq!(
            parse_target("8080").unwrap(),
            LocalTarget {
                host: "127.0.0.1".into(),
                port: 8080,
            }
        );
    }

    #[test]
    fn host_port_ipv4() {
        assert_eq!(
            parse_target("127.0.0.1:8080").unwrap(),
            LocalTarget {
                host: "127.0.0.1".into(),
                port: 8080,
            }
        );
    }

    #[test]
    fn host_port_localhost() {
        assert_eq!(
            parse_target("localhost:5173").unwrap(),
            LocalTarget {
                host: "localhost".into(),
                port: 5173,
            }
        );
    }

    #[test]
    fn host_port_custom_ip() {
        assert_eq!(
            parse_target("192.168.1.50:4000").unwrap(),
            LocalTarget {
                host: "192.168.1.50".into(),
                port: 4000,
            }
        );
    }

    #[test]
    fn port_boundary_min() {
        assert_eq!(
            parse_target("1").unwrap(),
            LocalTarget {
                host: "127.0.0.1".into(),
                port: 1,
            }
        );
    }

    #[test]
    fn port_boundary_max() {
        assert_eq!(
            parse_target("65535").unwrap(),
            LocalTarget {
                host: "127.0.0.1".into(),
                port: 65535,
            }
        );
    }

    #[test]
    fn port_zero_rejected() {
        assert_eq!(
            parse_target("0").unwrap_err(),
            "port must be between 1 and 65535"
        );
    }

    #[test]
    fn port_too_large() {
        assert!(parse_target("99999").is_err());
    }

    #[test]
    fn invalid_port_letters() {
        assert!(parse_target("abc").is_err());
    }

    #[test]
    fn missing_port_after_colon() {
        assert!(parse_target("127.0.0.1:").is_err());
    }

    #[test]
    fn missing_host_before_colon() {
        assert!(parse_target(":3000").is_err());
    }

    #[test]
    fn empty_input() {
        assert!(parse_target("").is_err());
    }

    #[test]
    fn whitespace_trimmed() {
        assert_eq!(
            parse_target("  3000  ").unwrap(),
            LocalTarget {
                host: "127.0.0.1".into(),
                port: 3000,
            }
        );
    }

    #[test]
    fn display_impl() {
        let target = LocalTarget {
            host: "127.0.0.1".into(),
            port: 3000,
        };
        assert_eq!(target.to_string(), "127.0.0.1:3000");
    }

    // ── Subdomain validation ─────────────────────────────────────────

    #[test]
    fn subdomain_valid_simple() {
        assert_eq!(validate_subdomain("my-preview").unwrap(), "my-preview");
    }

    #[test]
    fn subdomain_valid_alphanumeric() {
        assert_eq!(validate_subdomain("test123").unwrap(), "test123");
    }

    #[test]
    fn subdomain_valid_single_char() {
        assert_eq!(validate_subdomain("a").unwrap(), "a");
    }

    #[test]
    fn subdomain_valid_max_length() {
        let max_len = "a".repeat(63);
        assert_eq!(validate_subdomain(&max_len).unwrap(), max_len);
    }

    #[test]
    fn subdomain_empty_rejected() {
        assert!(validate_subdomain("").is_err());
    }

    #[test]
    fn subdomain_too_long() {
        let long = "a".repeat(64);
        assert!(validate_subdomain(&long).is_err());
    }

    #[test]
    fn subdomain_starts_with_hyphen() {
        assert!(validate_subdomain("-test").is_err());
    }

    #[test]
    fn subdomain_ends_with_hyphen() {
        assert!(validate_subdomain("test-").is_err());
    }

    #[test]
    fn subdomain_underscore_rejected() {
        assert!(validate_subdomain("test_underscore").is_err());
    }

    #[test]
    fn subdomain_space_rejected() {
        assert!(validate_subdomain("test space").is_err());
    }

    #[test]
    fn subdomain_dot_rejected() {
        assert!(validate_subdomain("test.dot").is_err());
    }
}
