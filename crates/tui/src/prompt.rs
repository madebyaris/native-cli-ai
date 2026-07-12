use reedline::Prompt;

pub struct NcaPrompt {
    pub safe_mode: bool,
    pub run_mode: bool,
    pub agent_label: String,
}

impl NcaPrompt {
    pub fn new(safe_mode: bool, run_mode: bool) -> Self {
        Self {
            safe_mode,
            run_mode,
            agent_label: "@build".to_string(),
        }
    }

    /// Set the current agent label (e.g., "@build", "@plan", "@review")
    pub fn set_agent(&mut self, label: &str) {
        self.agent_label = label.to_string();
    }

    pub fn prompt_string(&self) -> String {
        let mut parts = vec!["nca"];

        if !self.agent_label.is_empty() && self.agent_label != "@build" {
            parts.push(&self.agent_label);
        }
        if self.safe_mode {
            parts.push("safe");
        }
        if self.run_mode {
            parts.push("run");
        }

        let base = parts.join("( ");
        format!("{base})> ",)
    }
}

impl Prompt for NcaPrompt {
    fn render_prompt_left(&self) -> std::borrow::Cow<'_, str> {
        self.prompt_string().into()
    }

    fn render_prompt_right(&self) -> std::borrow::Cow<'_, str> {
        std::borrow::Cow::Borrowed("")
    }

    fn render_prompt_indicator(
        &self,
        _edit_mode: reedline::PromptEditMode,
    ) -> std::borrow::Cow<'_, str> {
        std::borrow::Cow::Borrowed("")
    }

    fn render_prompt_multiline_indicator(&self) -> std::borrow::Cow<'_, str> {
        "... ".into()
    }

    fn render_prompt_history_search_indicator(
        &self,
        _search: reedline::PromptHistorySearch,
    ) -> std::borrow::Cow<'_, str> {
        std::borrow::Cow::Borrowed("(search) ")
    }
}
