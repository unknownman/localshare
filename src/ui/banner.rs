use colored::Colorize;

use crate::tunnel::client::TunnelSession;

#[derive(Debug, Clone, Copy)]
pub enum ConnectionStatus {
    Live,
    Connecting,
    Reconnecting { attempt: u32 },
}

impl ConnectionStatus {
    /// Plain (unstyled) human-readable label.
    pub fn label(self) -> String {
        match self {
            Self::Live => "● LIVE".to_string(),
            Self::Connecting => "◌ CONNECTING...".to_string(),
            Self::Reconnecting { attempt } => format!("▲ RECONNECTING (Attempt {attempt})..."),
        }
    }

    /// Accent colour for the label; `None` keeps the default foreground.
    pub fn color(self) -> Option<colored::Color> {
        match self {
            Self::Live => Some(colored::Color::Green),
            Self::Connecting => Some(colored::Color::Cyan),
            Self::Reconnecting { .. } => Some(colored::Color::Yellow),
        }
    }
}

/// Renders a status label in bold + its accent colour. Styling is driven by
/// `colored`'s global override, so `NO_COLOR` and piped output drop the escapes.
pub fn styled_label(status: ConnectionStatus) -> colored::ColoredString {
    let mut label = status.label().bold();
    if let Some(color) = status.color() {
        label = label.color(color);
    }
    label
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

/// Length of `s` in terminal columns, ignoring ANSI escape sequences so the
/// box-drawing alignment is computed from what the user *sees*.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            if chars.next() == Some('[') {
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

impl<'a> std::fmt::Display for Banner<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let header = format!(
            "localshare {} • Share your local server instantly",
            self.version
        );
        writeln!(f, "{}", header)?;

        let content = format!(
            "{}  {}\n{}  {}\n{}  {}  ({})",
            "Public URL :".cyan().bold(),
            self.public_url,
            "Forwarding :".white().bold(),
            self.forwarding,
            "Status     :".white().bold(),
            styled_label(self.status),
            self.relay
        );

        let lines: Vec<&str> = content.lines().collect();
        let max_len = lines
            .iter()
            .map(|l| strip_ansi(l).chars().count())
            .max()
            .unwrap_or(0);
        let horizontal = "─".repeat(max_len);

        writeln!(f, "┌{}┐", horizontal)?;
        for line in &lines {
            let gap = max_len - strip_ansi(line).chars().count();
            let padding = " ".repeat(gap);
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

    /// With ANSI colouring *enabled*, every visible line of the banner box must
    /// still have the same width: alignment is computed from the display width
    /// (escaping stripped), not from the raw byte/char count.
    #[test]
    fn banner_box_width_is_computed_from_visible_chars_when_colored() {
        colored::control::set_override(true);
        let text = Banner::new(
            "https://foo.relay.localshare.dev",
            "http://127.0.0.1:3000",
            ConnectionStatus::Live,
            "relay.localshare.dev",
            "0.1.0",
        )
        .to_string();
        colored::control::set_override(false);

        assert!(
            text.contains('\x1b'),
            "colored override should have injected escape codes"
        );
        let widths: Vec<usize> = text
            .lines()
            .map(|l| strip_ansi(l).chars().count())
            .collect();
        // Skip line 0 (the header above the box); all *box* rows must align.
        let box_widths = &widths[1..];
        let first = box_widths[0];
        assert!(
            box_widths.iter().all(|&w| w == first),
            "box lines must align once ANSI is stripped, got {box_widths:?} for: {text:?}"
        );
    }

    #[test]
    fn strip_ansi_removes_color_and_style_codes() {
        assert_eq!(strip_ansi("plain"), "plain");
        assert_eq!(strip_ansi("\x1b[1;36mcyan\x1b[0m"), "cyan");
        assert_eq!(strip_ansi("a\x1b[31mb\x1b[0mc"), "abc");
    }
}
