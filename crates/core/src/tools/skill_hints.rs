use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

const MAX_TRACKED_SKILLS: usize = 8;

#[derive(Clone, Default)]
pub struct RecentSkillHints {
    inner: Arc<Mutex<VecDeque<String>>>,
}

impl RecentSkillHints {
    pub fn record(&self, skill: &str) {
        let skill = skill.trim();
        if skill.is_empty() {
            return;
        }

        let mut guard = self.inner.lock().expect("recent skill hints lock");
        if let Some(index) = guard.iter().position(|existing| existing == skill) {
            guard.remove(index);
        }
        guard.push_back(skill.to_string());
        while guard.len() > MAX_TRACKED_SKILLS {
            guard.pop_front();
        }
    }

    pub fn recent(&self, max: usize) -> Vec<String> {
        if max == 0 {
            return Vec::new();
        }

        let guard = self.inner.lock().expect("recent skill hints lock");
        let mut out: Vec<String> = guard.iter().rev().take(max).cloned().collect();
        out.reverse();
        out
    }
}

#[cfg(test)]
mod tests {
    use super::RecentSkillHints;

    #[test]
    fn record_moves_existing_skill_to_end() {
        let hints = RecentSkillHints::default();
        hints.record("review");
        hints.record("testing");
        hints.record("review");

        assert_eq!(hints.recent(2), vec!["testing", "review"]);
    }

    #[test]
    fn recent_returns_latest_items_in_original_order() {
        let hints = RecentSkillHints::default();
        hints.record("a");
        hints.record("b");
        hints.record("c");

        assert_eq!(hints.recent(2), vec!["b", "c"]);
    }
}
