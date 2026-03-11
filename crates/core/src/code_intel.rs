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
        let symbol_pattern = format!(r"(fn|struct|enum|trait|impl)\s+{query}");
        let mut cmd = tokio::process::Command::new("rg");
        cmd.arg("--line-number")
            .arg("--color=never")
            .arg("--no-heading")
            .arg(&symbol_pattern)
            .current_dir(&self.workspace_root);

        if let Some(glob) = glob {
            cmd.arg("--glob").arg(glob);
        } else {
            cmd.arg("--glob").arg("*.rs");
        }

        let output = cmd
            .output()
            .await
            .map_err(|err| CodeIntelError::Execution(err.to_string()))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut matches = Vec::new();
        for line in stdout.lines() {
            let mut parts = line.splitn(3, ':');
            let Some(file) = parts.next() else { continue };
            let Some(line_no) = parts.next() else { continue };
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
