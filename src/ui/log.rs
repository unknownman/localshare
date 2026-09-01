use crate::tunnel::client::TunnelEvent;

#[derive(Debug, Clone, Copy)]
pub enum MethodColor {
    Get,
    Post,
    Put,
    Patch,
    Delete,
    Other,
}

impl MethodColor {
    pub fn from_method(method: &str) -> Self {
        match method {
            "GET" => Self::Get,
            "POST" => Self::Post,
            "PUT" => Self::Put,
            "PATCH" => Self::Patch,
            "DELETE" => Self::Delete,
            _ => Self::Other,
        }
    }

    pub fn ansi(self) -> &'static str {
        match self {
            Self::Get => "\x1b[36m",
            Self::Post => "\x1b[32m",
            Self::Put | Self::Patch => "\x1b[33m",
            Self::Delete => "\x1b[31m",
            Self::Other => "\x1b[37m",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct StatusInfo {
    pub code: u16,
    pub reason: &'static str,
}

impl StatusInfo {
    pub fn from_code(code: u16) -> Self {
        let reason = match code {
            200 => "OK",
            201 => "Created",
            204 => "No Content",
            301 => "Moved Permanently",
            302 => "Found",
            304 => "Not Modified",
            400 => "Bad Request",
            401 => "Unauthorized",
            403 => "Forbidden",
            404 => "Not Found",
            405 => "Method Not Allowed",
            408 => "Request Timeout",
            413 => "Payload Too Large",
            422 => "Unprocessable Entity",
            429 => "Too Many Requests",
            500 => "Internal Server Error",
            502 => "Bad Gateway",
            503 => "Service Unavailable",
            504 => "Gateway Timeout",
            _ => "",
        };
        Self { code, reason }
    }

    pub fn label(self) -> String {
        if self.reason.is_empty() {
            format!("{}", self.code)
        } else {
            format!("{} {}", self.code, self.reason)
        }
    }

    pub fn ansi(self) -> &'static str {
        match self.code {
            200..=299 => "\x1b[32m",
            300..=399 => "\x1b[36m",
            400..=499 => "\x1b[33m",
            500..=599 => "\x1b[31m",
            _ => "\x1b[37m",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct LatencyInfo(pub std::time::Duration);

impl LatencyInfo {
    pub fn ansi(self) -> &'static str {
        let ms = self.0.as_millis();
        if ms < 100 {
            "\x1b[32m"
        } else if ms <= 500 {
            "\x1b[33m"
        } else {
            "\x1b[31m"
        }
    }

    pub fn label(self) -> String {
        let ms = self.0.as_millis();
        if ms == 0 {
            "<1ms".to_string()
        } else {
            format!("{}ms", ms)
        }
    }
}

#[derive(Debug, Clone)]
pub struct RequestLogEntry {
    pub time: String,
    pub method: String,
    pub path: String,
    pub status: u16,
    pub duration: std::time::Duration,
    pub hint: Option<String>,
}

impl RequestLogEntry {
    pub fn from_event(event: &TunnelEvent) -> Option<Self> {
        match event {
            TunnelEvent::RequestHandled {
                method,
                path,
                status,
                duration,
                hint,
                ..
            } => {
                let time = format_time(*duration);
                Some(Self {
                    time,
                    method: method.clone(),
                    path: path.clone(),
                    status: *status,
                    duration: *duration,
                    hint: hint.clone(),
                })
            }
            _ => None,
        }
    }

    /// Returns a distinct, user-facing warning string when the request failed
    /// in a way that carries an actionable hint (e.g. local server not running).
    pub fn format_hint(&self) -> Option<String> {
        let reset = "\x1b[0m";
        let yellow = "\x1b[33m";
        self.hint
            .as_ref()
            .map(|h| format!("{yellow}  ⚠ {}{reset}", h))
    }

    pub fn format_line(&self, path_width: usize) -> String {
        let method_color = MethodColor::from_method(&self.method).ansi();
        let status = StatusInfo::from_code(self.status);
        let latency_color = LatencyInfo(self.duration).ansi();
        let reset = "\x1b[0m";
        let dim = "\x1b[2m";

        let path = if self.path.chars().count() > path_width {
            let mut truncated = self
                .path
                .chars()
                .take(path_width.saturating_sub(1))
                .collect::<String>();
            truncated.push('…');
            truncated
        } else {
            self.path.clone()
        };

        format!(
            "{}{}{}  {}{:5}{}  {:<width$}  {}{}{}  {}{}{}",
            dim,
            self.time,
            reset,
            method_color,
            self.method,
            reset,
            path,
            status.ansi(),
            status.label(),
            reset,
            latency_color,
            LatencyInfo(self.duration).label(),
            reset,
            width = path_width
        )
    }
}

pub fn format_time(duration: std::time::Duration) -> String {
    let total_secs = duration.as_secs();
    let hours = total_secs / 3600;
    let minutes = (total_secs % 3600) / 60;
    let seconds = total_secs % 60;

    if hours > 0 {
        format!("{:02}:{:02}:{:02}", hours, minutes, seconds)
    } else {
        format!("{:02}:{:02}:{:02}", 0, minutes, seconds)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_color_2xx_is_green() {
        let info = StatusInfo::from_code(200);
        assert_eq!(info.ansi(), "\x1b[32m");
    }

    #[test]
    fn status_color_4xx_is_yellow() {
        let info = StatusInfo::from_code(404);
        assert_eq!(info.ansi(), "\x1b[33m");
    }

    #[test]
    fn status_color_5xx_is_red() {
        let info = StatusInfo::from_code(502);
        assert_eq!(info.ansi(), "\x1b[31m");
    }

    #[test]
    fn latency_color_green_below_100ms() {
        let info = LatencyInfo(std::time::Duration::from_millis(50));
        assert_eq!(info.ansi(), "\x1b[32m");
    }

    #[test]
    fn latency_color_yellow_in_mid_range() {
        let info = LatencyInfo(std::time::Duration::from_millis(200));
        assert_eq!(info.ansi(), "\x1b[33m");
    }

    #[test]
    fn latency_color_red_above_500ms() {
        let info = LatencyInfo(std::time::Duration::from_millis(600));
        assert_eq!(info.ansi(), "\x1b[31m");
    }

    #[test]
    fn request_log_line_formatting() {
        let entry = RequestLogEntry {
            time: "18:42:01".into(),
            method: "GET".into(),
            path: "/api/v1/health".into(),
            status: 200,
            duration: std::time::Duration::from_millis(4),
            hint: None,
        };
        let line = entry.format_line(20);
        assert!(line.contains("GET"));
        assert!(line.contains("/api/v1/health"));
        assert!(line.contains("200"));
        assert!(line.contains("4ms"));
    }

    #[test]
    fn request_log_hint_when_present() {
        let entry = RequestLogEntry {
            time: "18:42:01".into(),
            method: "GET".into(),
            path: "/".into(),
            status: 502,
            duration: std::time::Duration::from_millis(4),
            hint: Some("Is your local server running on 127.0.0.1:3000?".into()),
        };
        let hint = entry.format_hint().expect("hint should be rendered");
        assert!(hint.contains("Is your local server running on 127.0.0.1:3000?"));
    }

    #[test]
    fn request_log_no_hint_when_absent() {
        let entry = RequestLogEntry {
            time: "18:42:01".into(),
            method: "GET".into(),
            path: "/".into(),
            status: 200,
            duration: std::time::Duration::from_millis(4),
            hint: None,
        };
        assert!(entry.format_hint().is_none());
    }

    #[test]
    fn request_log_line_truncates_path() {
        let entry = RequestLogEntry {
            time: "18:42:01".into(),
            method: "GET".into(),
            path: "/this/is/a/very/long/path/that/should/be/truncated".into(),
            status: 200,
            duration: std::time::Duration::from_millis(4),
            hint: None,
        };
        let line = entry.format_line(10);
        assert!(line.contains('…'));
    }
}
