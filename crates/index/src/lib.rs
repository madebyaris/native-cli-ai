//! Feature-flagged BM25 code/text index for the workspace.
//!
//! Enabled via the `semantic-index` feature. When disabled, this crate exposes
//! a small no-op API so the rest of the workspace can reference it
//! unconditionally without paying the dependency cost of `tantivy`.
//!
//! There are explicitly *no* Python or ONNX dependencies here — the "semantic"
//! label refers to structured indexing (code-aware tokenisation + BM25 + path
//! facets), not neural embeddings.

#![allow(clippy::pedantic, clippy::result_large_err)]

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SearchHit {
    pub path: PathBuf,
    pub score: f32,
    pub line: Option<u64>,
    pub snippet: String,
}

#[derive(Debug, thiserror::Error)]
pub enum IndexError {
    #[error("semantic index feature is disabled (rebuild with `--features semantic-index`)")]
    FeatureDisabled,
    #[error("index io: {0}")]
    Io(#[from] std::io::Error),
    #[error("index error: {0}")]
    Other(String),
}

/// Sync handle used by the search_semantic tool.
pub struct Index {
    #[allow(dead_code)]
    root: PathBuf,
    #[cfg(feature = "semantic-index")]
    inner: imp::TantivyIndex,
}

impl Index {
    pub fn open(root: &Path) -> Result<Self, IndexError> {
        #[cfg(feature = "semantic-index")]
        {
            let inner = imp::TantivyIndex::open_or_create(root)?;
            return Ok(Self {
                root: root.to_path_buf(),
                inner,
            });
        }
        #[cfg(not(feature = "semantic-index"))]
        {
            let _ = root;
            Err(IndexError::FeatureDisabled)
        }
    }

    /// Rebuild the index from the workspace. On feature-disabled builds this
    /// returns `FeatureDisabled` so the caller can surface a clear CLI hint.
    pub fn rebuild(&mut self, include_globs: &[String]) -> Result<usize, IndexError> {
        #[cfg(feature = "semantic-index")]
        {
            self.inner.rebuild(include_globs)
        }
        #[cfg(not(feature = "semantic-index"))]
        {
            let _ = include_globs;
            Err(IndexError::FeatureDisabled)
        }
    }

    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>, IndexError> {
        #[cfg(feature = "semantic-index")]
        {
            self.inner.search(query, limit)
        }
        #[cfg(not(feature = "semantic-index"))]
        {
            let _ = (query, limit);
            Err(IndexError::FeatureDisabled)
        }
    }

    pub fn is_available() -> bool {
        cfg!(feature = "semantic-index")
    }
}

#[cfg(feature = "semantic-index")]
mod imp {
    use super::{IndexError, SearchHit};
    use std::path::{Path, PathBuf};
    use tantivy::collector::TopDocs;
    use tantivy::query::QueryParser;
    use tantivy::schema::{Field, STORED, STRING, Schema, TEXT, Value};
    use tantivy::{Index as TIndex, IndexReader, IndexWriter, doc};

    pub struct TantivyIndex {
        root: PathBuf,
        index: TIndex,
        reader: IndexReader,
        f_path: Field,
        f_body: Field,
    }

    impl TantivyIndex {
        pub fn open_or_create(root: &Path) -> Result<Self, IndexError> {
            let dir = root.join(".nca").join("index");
            std::fs::create_dir_all(&dir)?;

            let mut schema_builder = Schema::builder();
            let f_path = schema_builder.add_text_field("path", STRING | STORED);
            let f_body = schema_builder.add_text_field("body", TEXT | STORED);
            let schema = schema_builder.build();

            let index = if dir.join("meta.json").exists() {
                TIndex::open_in_dir(&dir).map_err(|e| IndexError::Other(e.to_string()))?
            } else {
                TIndex::create_in_dir(&dir, schema.clone())
                    .map_err(|e| IndexError::Other(e.to_string()))?
            };
            let reader = index
                .reader()
                .map_err(|e| IndexError::Other(e.to_string()))?;
            Ok(Self {
                root: root.to_path_buf(),
                index,
                reader,
                f_path,
                f_body,
            })
        }

        pub fn rebuild(&mut self, include_globs: &[String]) -> Result<usize, IndexError> {
            let mut writer: IndexWriter = self
                .index
                .writer(64_000_000)
                .map_err(|e| IndexError::Other(e.to_string()))?;
            writer
                .delete_all_documents()
                .map_err(|e| IndexError::Other(e.to_string()))?;

            let mut count = 0usize;
            for entry in walkdir::WalkDir::new(&self.root)
                .follow_links(false)
                .into_iter()
                .filter_map(|e| e.ok())
            {
                if !entry.file_type().is_file() {
                    continue;
                }
                let rel = match entry.path().strip_prefix(&self.root) {
                    Ok(r) => r,
                    Err(_) => continue,
                };
                let rel_str = rel.to_string_lossy();
                if rel_str.starts_with(".git/")
                    || rel_str.starts_with("target/")
                    || rel_str.starts_with("node_modules/")
                    || rel_str.starts_with(".nca/")
                {
                    continue;
                }
                if !include_globs.is_empty()
                    && !include_globs.iter().any(|pat| matches_glob(pat, &rel_str))
                {
                    continue;
                }
                let body = match std::fs::read_to_string(entry.path()) {
                    Ok(b) => b,
                    Err(_) => continue,
                };
                if body.len() > 2_000_000 {
                    continue;
                }
                writer
                    .add_document(doc!(
                        self.f_path => rel_str.to_string(),
                        self.f_body => body
                    ))
                    .map_err(|e| IndexError::Other(e.to_string()))?;
                count += 1;
            }
            writer
                .commit()
                .map_err(|e| IndexError::Other(e.to_string()))?;
            self.reader
                .reload()
                .map_err(|e| IndexError::Other(e.to_string()))?;
            Ok(count)
        }

        pub fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>, IndexError> {
            let searcher = self.reader.searcher();
            let parser = QueryParser::for_index(&self.index, vec![self.f_body]);
            let q = parser
                .parse_query(query)
                .map_err(|e| IndexError::Other(e.to_string()))?;
            let top = searcher
                .search(&q, &TopDocs::with_limit(limit.max(1)))
                .map_err(|e| IndexError::Other(e.to_string()))?;
            let mut hits = Vec::with_capacity(top.len());
            for (score, addr) in top {
                let doc: tantivy::TantivyDocument = searcher
                    .doc(addr)
                    .map_err(|e| IndexError::Other(e.to_string()))?;
                let path = doc
                    .get_first(self.f_path)
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let body = doc
                    .get_first(self.f_body)
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                hits.push(SearchHit {
                    path: PathBuf::from(path),
                    score,
                    line: None,
                    snippet: snippet_from_body(body, query),
                });
            }
            Ok(hits)
        }
    }

    fn snippet_from_body(body: &str, query: &str) -> String {
        let needle = query.split_whitespace().next().unwrap_or(query);
        if let Some(pos) = body.to_ascii_lowercase().find(&needle.to_ascii_lowercase()) {
            let start = pos.saturating_sub(40);
            let end = (pos + needle.len() + 80).min(body.len());
            let mut s = start;
            while s > 0 && !body.is_char_boundary(s) {
                s -= 1;
            }
            let mut e = end;
            while e < body.len() && !body.is_char_boundary(e) {
                e += 1;
            }
            return body[s..e].replace('\n', " ");
        }
        body.chars()
            .take(120)
            .collect::<String>()
            .replace('\n', " ")
    }

    fn matches_glob(pat: &str, path: &str) -> bool {
        if let Some(suffix) = pat.strip_prefix("*.") {
            return path.ends_with(&format!(".{suffix}"));
        }
        path.contains(pat)
    }
}
