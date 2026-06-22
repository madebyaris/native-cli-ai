//! OpenCode-style "Connect a provider" list (search + sections).
//!
//! Only providers that `nca` can actually use are listed; layout mirrors common
//! "Popular / Other" grouping from tools like OpenCode.

use nca_common::config::ProviderKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectSection {
    Popular,
    Other,
}

#[derive(Debug, Clone, Copy)]
pub struct CatalogEntry {
    pub section: ConnectSection,
    pub kind: ProviderKind,
    pub title: &'static str,
    pub subtitle: &'static str,
}

/// Ordered catalog (OpenCode-like: recommended first, then popular APIs, then routing).
pub const CONNECT_CATALOG: &[CatalogEntry] = &[
    CatalogEntry {
        section: ConnectSection::Popular,
        kind: ProviderKind::MiniMax,
        title: "MiniMax",
        subtitle: "Recommended · M2.5 (API key)",
    },
    CatalogEntry {
        section: ConnectSection::Popular,
        kind: ProviderKind::OpenAi,
        title: "OpenAI",
        subtitle: "GPT models (API key)",
    },
    CatalogEntry {
        section: ConnectSection::Popular,
        kind: ProviderKind::Anthropic,
        title: "Anthropic",
        subtitle: "Claude (API key)",
    },
    CatalogEntry {
        section: ConnectSection::Popular,
        kind: ProviderKind::ZhipuAI,
        title: "ZhipuAI",
        subtitle: "GLM-5 Turbo (API key)",
    },
    CatalogEntry {
        section: ConnectSection::Popular,
        kind: ProviderKind::DeepSeek,
        title: "DeepSeek",
        subtitle: "V4 Flash / V4 Pro (API key)",
    },
    CatalogEntry {
        section: ConnectSection::Other,
        kind: ProviderKind::OpenRouter,
        title: "OpenRouter",
        subtitle: "Multi-model routing (API key)",
    },
];

#[derive(Debug, Clone)]
pub enum ConnectRow {
    SectionHeader(&'static str),
    Provider {
        kind: ProviderKind,
        title: &'static str,
        subtitle: &'static str,
    },
}

fn matches_filter(entry: &CatalogEntry, q: &str) -> bool {
    if q.is_empty() {
        return true;
    }
    let q = q.to_ascii_lowercase();
    entry.title.to_ascii_lowercase().contains(&q)
        || entry.subtitle.to_ascii_lowercase().contains(&q)
        || entry.kind.display_name().to_ascii_lowercase().contains(&q)
}

/// Build flat rows: section headers (only when section has matches) + provider lines.
pub fn build_connect_rows(search: &str) -> Vec<ConnectRow> {
    let q = search.trim();
    let mut out = Vec::new();

    for section in [ConnectSection::Popular, ConnectSection::Other] {
        let label = match section {
            ConnectSection::Popular => "Popular",
            ConnectSection::Other => "Other",
        };
        let matches: Vec<&CatalogEntry> = CONNECT_CATALOG
            .iter()
            .filter(|e| e.section == section && matches_filter(e, q))
            .collect();
        if matches.is_empty() {
            continue;
        }
        out.push(ConnectRow::SectionHeader(label));
        for e in matches {
            out.push(ConnectRow::Provider {
                kind: e.kind,
                title: e.title,
                subtitle: e.subtitle,
            });
        }
    }

    out
}

/// Indices into `rows` that are selectable providers (skip section headers).
pub fn selectable_row_indices(rows: &[ConnectRow]) -> Vec<usize> {
    rows.iter()
        .enumerate()
        .filter_map(|(i, r)| matches!(r, ConnectRow::Provider { .. }).then_some(i))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_openai_shows_only_openai_under_popular() {
        let rows = build_connect_rows("openai");
        assert!(
            rows.iter()
                .any(|r| matches!(r, ConnectRow::SectionHeader("Popular")))
        );
        assert!(rows.iter().any(|r| matches!(
            r,
            ConnectRow::Provider {
                title: "OpenAI",
                ..
            }
        )));
        assert!(!rows.iter().any(|r| matches!(
            r,
            ConnectRow::Provider {
                title: "MiniMax",
                ..
            }
        )));
    }

    #[test]
    fn empty_search_includes_all_providers() {
        let rows = build_connect_rows("");
        let n = selectable_row_indices(&rows).len();
        assert_eq!(n, CONNECT_CATALOG.len());
    }
}
