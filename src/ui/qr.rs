use qrcode::QrCode;
use std::fmt;

#[derive(Debug, Clone)]
pub struct TerminalQr {
    pub lines: Vec<String>,
}

impl fmt::Display for TerminalQr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for line in &self.lines {
            writeln!(f, "{}", line)?;
        }
        Ok(())
    }
}

/// Render a URL as a terminal QR code. Returns a banner-shaped `TerminalQr`
/// even if the URL cannot be encoded, so callers never have to handle a panic.
pub fn render_qr(text: &str) -> TerminalQr {
    // `QrCode::new` only fails when the content exceeds the maximum capacity
    // of the largest QR version; public URLs are far below that, but guard
    // anyway instead of panicking on user-supplied data.
    let code = match QrCode::new(text) {
        Ok(code) => code,
        Err(_) => {
            return TerminalQr {
                lines: vec![format!("QR unavailable for: {}", text)],
            }
        }
    };
    let colors = code.to_colors();
    let qr_size = code.width();

    let quiet = 2;
    let total_width = qr_size + quiet * 2;
    let total_height = qr_size + quiet * 2;

    let mut lines = Vec::with_capacity(total_height.div_ceil(2));

    for row in (0..total_height).step_by(2) {
        let mut line = String::with_capacity(total_width);
        for col in 0..total_width {
            let in_quiet =
                row < quiet || row >= quiet + qr_size || col < quiet || col >= quiet + qr_size;
            let top_black = in_quiet || qr_module_dark(&colors, qr_size, row - quiet, col - quiet);
            let bottom_black = row + 1 < total_height
                && !(row + 1 < quiet
                    || row + 1 >= quiet + qr_size
                    || col < quiet
                    || col >= quiet + qr_size)
                && qr_module_dark(&colors, qr_size, row + 1 - quiet, col - quiet);

            let ch = match (top_black, bottom_black) {
                (true, true) => '█',
                (true, false) => '▀',
                (false, true) => '▄',
                (false, false) => ' ',
            };
            line.push(ch);
        }
        lines.push(line);
    }

    TerminalQr { lines }
}

fn qr_module_dark(colors: &[qrcode::Color], qr_size: usize, row: usize, col: usize) -> bool {
    if row >= qr_size || col >= qr_size {
        return false;
    }
    let idx = row * qr_size + col;
    colors.get(idx).copied() == Some(qrcode::Color::Dark)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_known_data_with_quiet_zone() {
        let qr = render_qr("https://example.com");
        let width = qr.lines.iter().map(|l| l.chars().count()).max().unwrap();
        assert!(width >= 21 + 4);
        assert!(qr.lines.iter().all(|line| line.chars().count() == width));
        assert!((qr.lines.len() * 2) >= 21 + 4);
    }

    #[test]
    fn output_is_valid_utf8_and_printable() {
        let qr = render_qr("https://example.com");
        for line in &qr.lines {
            assert!(line.chars().all(|c| { matches!(c, '█' | '▀' | '▄' | ' ') }));
        }
    }

    #[test]
    fn empty_content_still_renders() {
        let qr = render_qr("");
        assert!(!qr.lines.is_empty());
    }
}
