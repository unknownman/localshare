use std::fmt;

#[derive(Debug, Clone)]
pub struct ErrorView<'a> {
    pub title: &'a str,
    pub detail: &'a str,
    pub message: String,
    pub hint: Option<String>,
}

impl<'a> ErrorView<'a> {
    pub fn new(title: &'a str, detail: &'a str, message: impl fmt::Display) -> Self {
        Self {
            title,
            detail,
            message: message.to_string(),
            hint: None,
        }
    }
}

impl<'a> fmt::Display for ErrorView<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "✗ {}", self.title)?;
        if !self.detail.is_empty() {
            writeln!(f, "  {}", self.detail)?;
        }
        writeln!(f, "    {}", self.message)?;
        if let Some(hint) = &self.hint {
            writeln!(f, "    → {}", hint)?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_error_with_title_and_message() {
        let view = ErrorView::new(
            "Could not connect to local server",
            "",
            "connection refused to 127.0.0.1:3000",
        );
        let text = view.to_string();
        assert!(text.contains("✗ Could not connect to local server"));
        assert!(text.contains("127.0.0.1:3000"));
    }

    #[test]
    fn includes_detail_when_present() {
        let view = ErrorView::new("Failed", "extra context", "oops");
        let text = view.to_string();
        assert!(text.contains("extra context"));
    }

    #[test]
    fn no_hint_rendered_when_absent() {
        let view = ErrorView::new("Connection failed", "", "no route to host");
        let text = view.to_string();
        assert!(!text.contains("→"));
    }
}
