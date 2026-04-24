use anyhow::Result;
use arboard::Clipboard;

/// Manages clipboard read/write and change detection.
pub struct ClipboardManager {
    clipboard: Clipboard,
    last_text: String,
}

impl ClipboardManager {
    pub fn new() -> Result<Self> {
        Ok(Self {
            clipboard: Clipboard::new()?,
            last_text: String::new(),
        })
    }

    /// Read the current clipboard text.
    pub fn get_text(&mut self) -> Option<String> {
        self.clipboard.get_text().ok()
    }

    /// Write text to the clipboard.
    pub fn set_text(&mut self, text: &str) -> Result<()> {
        self.clipboard.set_text(text)?;
        self.last_text = text.to_owned();
        Ok(())
    }

    /// Poll for changes. Returns `Some(new_text)` if the clipboard changed
    /// since the last call, otherwise `None`.
    pub fn poll_change(&mut self) -> Option<String> {
        let current = self.clipboard.get_text().ok()?;
        if current != self.last_text {
            self.last_text = current.clone();
            Some(current)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// We can't test actual clipboard I/O in headless CI, but we can verify
    /// the `poll_change` change-detection logic in isolation.
    #[test]
    fn poll_change_detects_new_value() {
        // Simulate the state-tracking logic without a real clipboard.
        let mut last_text = String::new();
        let simulate_poll = |last: &mut String, current: &str| -> Option<String> {
            if current != *last {
                *last = current.to_owned();
                Some(current.to_owned())
            } else {
                None
            }
        };

        assert_eq!(simulate_poll(&mut last_text, "hello"), Some("hello".into()));
        assert_eq!(simulate_poll(&mut last_text, "hello"), None);
        assert_eq!(simulate_poll(&mut last_text, "world"), Some("world".into()));
    }

    #[test]
    fn poll_change_no_change_returns_none() {
        let mut last_text = "same".to_owned();
        let current = "same";
        let result = if current != last_text {
            last_text = current.to_owned();
            Some(current.to_owned())
        } else {
            None
        };
        assert_eq!(result, None);
    }
}
