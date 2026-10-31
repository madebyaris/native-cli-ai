//! Option search matcher supporting regex with case-insensitive substring fallback.

use regex::RegexBuilder;

/// Precompiled pattern matcher for option filtering in pickers.
#[derive(Debug, Clone)]
pub struct OptionMatcher {
    raw: String,
    regex: Option<regex::Regex>,
}

impl OptionMatcher {
    /// Build a new matcher from a query string.
    pub fn new(query: &str) -> Self {
        let trimmed = query.trim();
        if trimmed.is_empty() {
            return Self {
                raw: String::new(),
                regex: None,
            };
        }

        let regex = RegexBuilder::new(trimmed)
            .case_insensitive(true)
            .build()
            .ok();

        Self {
            raw: trimmed.to_ascii_lowercase(),
            regex,
        }
    }

    /// Whether the query is empty.
    pub fn is_empty(&self) -> bool {
        self.raw.is_empty()
    }

    /// Match a target text against regex or substring.
    pub fn is_match(&self, target: &str) -> bool {
        if self.raw.is_empty() {
            return true;
        }

        if let Some(re) = &self.regex {
            return re.is_match(target);
        }

        target.to_ascii_lowercase().contains(&self.raw)
    }

    /// Match against multiple candidate strings (returns true if any match).
    pub fn matches_any(&self, targets: &[&str]) -> bool {
        if self.raw.is_empty() {
            return true;
        }
        targets.iter().any(|target| self.is_match(target))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_query_matches_everything() {
        let matcher = OptionMatcher::new("");
        assert!(matcher.is_empty());
        assert!(matcher.is_match("any string"));
        assert!(matcher.matches_any(&["a", "b"]));
    }

    #[test]
    fn plain_substring_case_insensitive() {
        let matcher = OptionMatcher::new("gpt");
        assert!(matcher.is_match("gpt-4o"));
        assert!(matcher.is_match("GPT-4O-MINI"));
        assert!(!matcher.is_match("claude-3-sonnet"));
    }

    #[test]
    fn regex_pattern_matching() {
        let matcher = OptionMatcher::new(r"gpt.*mini");
        assert!(matcher.is_match("gpt-4o-mini"));
        assert!(matcher.is_match("GPT-4.5-MINI"));
        assert!(!matcher.is_match("gpt-4o"));
    }

    #[test]
    fn regex_does_not_fall_through_to_substring_on_miss() {
        let matcher = OptionMatcher::new("^main$");
        assert!(matcher.is_match("main"));
        assert!(!matcher.is_match("domain"));
    }

    #[test]
    fn invalid_regex_falls_back_to_substring() {
        let matcher = OptionMatcher::new("[unclosed-bracket");
        assert!(matcher.regex.is_none());
        assert!(matcher.is_match("text with [unclosed-bracket inside"));
        assert!(!matcher.is_match("text with other stuff"));
    }

    #[test]
    fn matches_any_checks_all_candidates() {
        let matcher = OptionMatcher::new("mini");
        assert!(matcher.matches_any(&["gpt-4o", "minimax-m2.5"]));
        assert!(!matcher.matches_any(&["claude-3", "sonnet"]));
    }
}
