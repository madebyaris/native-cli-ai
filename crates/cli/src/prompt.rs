/// Reedline-based prompt with history, completions, and mode indicator.
pub struct NcaPrompt {
    pub safe_mode: bool,
    pub run_mode: bool,
}

impl NcaPrompt {
    pub fn new(safe_mode: bool, run_mode: bool) -> Self {
        Self {
            safe_mode,
            run_mode,
        }
    }

    pub fn prompt_string(&self) -> &str {
        if self.safe_mode && self.run_mode {
            "nca(run,safe)> "
        } else if self.safe_mode {
            "nca(safe)> "
        } else if self.run_mode {
            "nca(run)> "
        } else {
            "nca> "
        }
    }
}
