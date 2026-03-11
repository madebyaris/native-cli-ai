/// Reedline-based prompt with history, completions, and mode indicator.
pub struct NcaPrompt {
    pub safe_mode: bool,
}

impl NcaPrompt {
    pub fn new(safe_mode: bool) -> Self {
        Self { safe_mode }
    }

    pub fn prompt_string(&self) -> &str {
        if self.safe_mode {
            "nca(safe)> "
        } else {
            "nca> "
        }
    }
}
