use crate::tunnel::client::TunnelSession;

#[derive(Debug, Clone, Copy)]
pub enum ConnectionStatus {
    Live,
    Connecting,
    Reconnecting { attempt: u32 },
}

impl ConnectionStatus {
    pub fn as_label(self) -> (&'static str, &'static str) {
        match self {
            Self::Live => ("● LIVE", "\x1b[32m"),
            Self::Connecting => ("◌ CONNECTING...", "\x1b[36m"),
            Self::Reconnecting { attempt } => {
                let label = format!("▲ RECONNECTING (Attempt {})...", attempt);
                (Box::leak(label.into_boxed_str()), "\x1b[33m")
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct Banner<'a> {
    pub public_url: &'a str,
    pub forwarding: &'a str,
    pub status: ConnectionStatus,
    pub relay: &'a str,
    pub version: &'a str,
}

impl<'a> Banner<'a> {
    pub fn new(
        public_url: &'a str,
        forwarding: &'a str,
        status: ConnectionStatus,
        relay: &'a str,
        version: &'a str,
    ) -> Self {
        Self {
            public_url,
            forwarding,
            status,
            relay,
            version,
        }
    }
}

impl<'a> std::fmt::Display for Banner<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let header = format!(
            "localshare {} • Share your local server instantly",
            self.version
        );
        writeln!(f, "{}", header)?;

        let (status_text, status_color) = self.status.as_label();
        let _reset = "\x1b[0m";
        let bold = "\x1b[1m";

        let content = format!(
            "\x1b[1;36mPublic URL :\x1b[0m  {}\n\x1b[1;37mForwarding :\x1b[0m  {}\n\x1b[1;37mStatus     :\x1b[0m  {}{}{}  ({})",
            self.public_url, self.forwarding, status_color, bold, status_text, self.relay
        );

        let lines: Vec<&str> = content.lines().collect();
        let max_len = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0);
        let horizontal = "─".repeat(max_len);

        writeln!(f, "┌{}┐", horizontal)?;
        for line in &lines {
            let padding = " ".repeat(max_len - line.chars().count());
            writeln!(f, "│{}{}│", line, padding)?;
        }
        writeln!(f, "└{}┘", horizontal)?;

        Ok(())
    }
}

pub fn session_to_banner<'a>(
    session: &'a TunnelSession,
    forwarding: &'a str,
    relay: &'a str,
    version: &'a str,
) -> Banner<'a> {
    Banner::new(
        &session.public_url,
        forwarding,
        ConnectionStatus::Live,
        relay,
        version,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_banner_contains_url() {
        let banner = Banner::new(
            "https://foo.relay.localshare.dev",
            "http://127.0.0.1:3000",
            ConnectionStatus::Live,
            "relay.localshare.dev",
            "0.1.0",
        );
        let text = banner.to_string();
        assert!(text.contains("https://foo.relay.localshare.dev"));
        assert!(text.contains("http://127.0.0.1:3000"));
        assert!(text.contains("● LIVE"));
        assert!(text.contains("relay.localshare.dev"));
    }

    #[test]
    fn reconnecting_label_includes_attempt() {
        let banner = Banner::new(
            "https://foo.relay.localshare.dev",
            "http://127.0.0.1:3000",
            ConnectionStatus::Reconnecting { attempt: 3 },
            "relay.localshare.dev",
            "0.1.0",
        );
        let text = banner.to_string();
        assert!(text.contains("▲ RECONNECTING (Attempt 3)..."));
    }
}
