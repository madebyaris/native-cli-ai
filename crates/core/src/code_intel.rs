use regex::escape;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy)]
pub enum CodeIntelMode {
    FastLocal,
    LanguageServer,
}

#[derive(Debug, Clone)]
pub struct SymbolMatch {
    pub file: PathBuf,
    pub line: u32,
    pub text: String,
}

#[async_trait::async_trait]
pub trait CodeIntel: Send + Sync {
    async fn query_symbols(
        &self,
        query: &str,
        glob: Option<&str>,
    ) -> Result<Vec<SymbolMatch>, CodeIntelError>;
}

pub struct FastLocalCodeIntel {
    workspace_root: PathBuf,
}

impl FastLocalCodeIntel {
    pub fn new(workspace_root: impl AsRef<Path>) -> Self {
        Self {
            workspace_root: workspace_root.as_ref().to_path_buf(),
        }
    }
}

#[async_trait::async_trait]
impl CodeIntel for FastLocalCodeIntel {
    async fn query_symbols(
        &self,
        query: &str,
        glob: Option<&str>,
    ) -> Result<Vec<SymbolMatch>, CodeIntelError> {
        let query = query.trim();
        if query.is_empty() {
            return Err(CodeIntelError::Execution(
                "query_symbols requires a non-empty literal symbol name".into(),
            ));
        }
        let escaped_query = escape(query);
        let symbol_pattern = format!(r"(fn|struct|enum|trait|impl)\s+{escaped_query}\b");
        let current_dir = self
            .workspace_root
            .canonicalize()
            .unwrap_or_else(|_| self.workspace_root.clone());
        let mut cmd = tokio::process::Command::new("rg");
        cmd.arg("--line-number")
            .arg("--color=never")
            .arg("--no-heading")
            .arg(&symbol_pattern)
            .arg(".")
            .current_dir(&current_dir);

        if let Some(glob) = glob {
            cmd.arg("--glob").arg(glob);
        } else {
            cmd.arg("--glob").arg("*.rs");
        }

        let output = cmd
            .output()
            .await
            .map_err(|err| CodeIntelError::Execution(err.to_string()))?;

        let exit_code = output.status.code().unwrap_or(-1);
        if !matches!(exit_code, 0 | 1) {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return Err(CodeIntelError::Execution(if stderr.is_empty() {
                format!("ripgrep failed with exit code {exit_code}")
            } else {
                format!("ripgrep failed with exit code {exit_code}: {stderr}")
            }));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut matches = Vec::new();
        for line in stdout.lines() {
            let mut parts = line.splitn(3, ':');
            let Some(file) = parts.next() else { continue };
            let Some(line_no) = parts.next() else {
                continue;
            };
            let Some(text) = parts.next() else { continue };
            matches.push(SymbolMatch {
                file: PathBuf::from(file),
                line: line_no.parse().unwrap_or(0),
                text: text.to_string(),
            });
        }
        Ok(matches)
    }
}

pub struct LanguageServerCodeIntel;

#[async_trait::async_trait]
impl CodeIntel for LanguageServerCodeIntel {
    async fn query_symbols(
        &self,
        _query: &str,
        _glob: Option<&str>,
    ) -> Result<Vec<SymbolMatch>, CodeIntelError> {
        Err(CodeIntelError::Unsupported(
            "language-server mode is not wired yet".into(),
        ))
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CodeIntelError {
    #[error("code-intel execution failed: {0}")]
    Execution(String),
    #[error("code-intel mode unsupported: {0}")]
    Unsupported(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn query_symbols_treats_query_as_literal() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("lib.rs"), "fn foo_bar() {}\nfn foo() {}\n").unwrap();

        let intel = FastLocalCodeIntel::new(dir.path());
        let matches = intel.query_symbols("foo.bar", Some("*.rs")).await.unwrap();
        assert!(matches.is_empty());
    }

    #[tokio::test]
    async fn query_symbols_rejects_empty_queries() {
        let dir = tempfile::tempdir().unwrap();
        let intel = FastLocalCodeIntel::new(dir.path());
        let err = intel.query_symbols("   ", Some("*.rs")).await.unwrap_err();
        assert!(err.to_string().contains("non-empty literal symbol name"));
    }
}
