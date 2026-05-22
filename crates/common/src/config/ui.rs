//! CLI/TUI preferences persisted in config.

use serde::{Deserialize, Serialize};

/// User interface preferences persisted in config.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiConfig {
    /// Shell command to launch the external editor (e.g. `vim` or `code --wait`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub editor: Option<String>,
    /// Theme name (future: "default", "tokyonight", etc.).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<String>,
    /// Hide hint text in the composer area.
    #[serde(default)]
    pub hide_tips: bool,
    /// Lines per scroll event (default 3).
    #[serde(default = "default_scroll_speed")]
    pub scroll_speed: u16,
    /// Whether the user has completed the first-run onboarding flow.
    #[serde(default)]
    pub onboarding_completed: bool,
}

fn default_scroll_speed() -> u16 {
    3
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            editor: None,
            theme: None,
            hide_tips: false,
            scroll_speed: default_scroll_speed(),
            onboarding_completed: false,
        }
    }
}

impl UiConfig {
    pub(super) fn merge(&mut self, partial: PartialUiConfig) {
        if let Some(editor) = partial.editor {
            self.editor = Some(editor);
        }
        if let Some(theme) = partial.theme {
            self.theme = Some(theme);
        }
        if let Some(hide_tips) = partial.hide_tips {
            self.hide_tips = hide_tips;
        }
        if let Some(scroll_speed) = partial.scroll_speed {
            self.scroll_speed = scroll_speed;
        }
        if let Some(onboarding_completed) = partial.onboarding_completed {
            self.onboarding_completed = onboarding_completed;
        }
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
pub(super) struct PartialUiConfig {
    editor: Option<String>,
    theme: Option<String>,
    hide_tips: Option<bool>,
    scroll_speed: Option<u16>,
    onboarding_completed: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn onboarding_completed_defaults_to_false() {
        let config = UiConfig::default();
        assert!(!config.onboarding_completed);
    }

    #[test]
    fn merge_applies_partial_ui_fields() {
        let mut config = UiConfig::default();
        config.merge(PartialUiConfig {
            editor: Some("vim".into()),
            theme: Some("tokyonight".into()),
            hide_tips: Some(true),
            scroll_speed: Some(5),
            onboarding_completed: Some(true),
        });
        assert_eq!(config.editor.as_deref(), Some("vim"));
        assert_eq!(config.theme.as_deref(), Some("tokyonight"));
        assert!(config.hide_tips);
        assert_eq!(config.scroll_speed, 5);
        assert!(config.onboarding_completed);
    }

    #[test]
    fn onboarding_completed_merges_from_partial_toml() {
        let toml_str = "onboarding_completed = true";
        let partial: PartialUiConfig = toml::from_str(toml_str).unwrap();
        let mut config = UiConfig::default();
        config.merge(partial);
        assert!(config.onboarding_completed);
    }
}
