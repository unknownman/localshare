use crate::error::LocalForwardError;

#[derive(Debug, Clone)]
pub struct ErrorView<'a> {
    pub title: &'a str,
    pub detail: &'a str,
    pub error: &'a LocalForwardError,
}

impl<'a> ErrorView<'a> {
    pub fn new(title: &'a str, detail: &'a str, error: &'a LocalForwardError) -> Self {
        Self {
            title,
            detail,
            error,
        }
    }
}

impl<'a> std::fmt::Display for ErrorView<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let error = format!("{}", self.error);
        let hint = self.error.actionable_hint();

        writeln!(f, "✗ {}", self.title)?;
        if !self.detail.is_empty() {
            writeln!(f, "  {}", self.detail)?;
        }
        writeln!(f, "    {}", error)?;
        if let Some(hint) = hint {
            writeln!(f, "    → {}", hint)?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_error_with_hint() {
        let err = LocalForwardError::target_connection_refused("127.0.0.1", 3000);
        let view = ErrorView::new("Could not connect to local server", "", &err);
        let text = view.to_string();
        assert!(text.contains("✗ Could not connect to local server"));
        assert!(text.contains("127.0.0.1:3000"));
        assert!(text.contains("→"));
    }

    #[test]
    fn includes_detail_when_present() {
        let err = LocalForwardError::target_connection_refused("127.0.0.1", 3000);
        let view = ErrorView::new("Failed", "extra context", &err);
        let text = view.to_string();
        assert!(text.contains("extra context"));
    }
}
