//! Conversion of ACP client `mcpServers` into nca MCP server config.
//!
//! ACP lets a client hand the agent the MCP servers it already has running. Those are the
//! tools the user sees in their editor — search, browsers, issue trackers — so connecting
//! them is what makes "use the desktop's search" work at all.

use agent_client_protocol::schema::v1::McpServer;
use nca_common::config::McpServerConfig;
use std::collections::BTreeMap;

/// Convert one ACP server. Returns `None` for transports nca cannot speak, so the caller
/// can skip it deliberately instead of registering a server that will never answer.
pub fn to_server_config(server: &McpServer) -> Option<McpServerConfig> {
    match server {
        McpServer::Stdio(stdio) => Some(McpServerConfig {
            name: stdio.name.clone(),
            command: stdio.command.to_string_lossy().into_owned(),
            args: stdio.args.clone(),
            env: stdio
                .env
                .iter()
                .map(|variable| (variable.name.clone(), variable.value.clone()))
                .collect::<BTreeMap<_, _>>(),
            cwd: None,
            url: None,
            headers: BTreeMap::new(),
            bearer_token_env: None,
            enabled: true,
        }),
        McpServer::Http(http) => Some(McpServerConfig {
            name: http.name.clone(),
            command: String::new(),
            args: Vec::new(),
            env: BTreeMap::new(),
            cwd: None,
            url: Some(http.url.clone()),
            headers: header_map(http.headers.iter()),
            bearer_token_env: None,
            enabled: true,
        }),
        // nca's remote client speaks streamable HTTP. An SSE endpoint is a different
        // transport, so pretending it is HTTP would produce a server that never answers.
        McpServer::Sse(sse) => {
            tracing::warn!(
                server = %sse.name,
                url = %sse.url,
                "skipping client MCP server: nca supports streamable HTTP, not SSE"
            );
            None
        }
        // The enum is non-exhaustive: a future transport is skipped rather than guessed at.
        other => {
            tracing::warn!("skipping client MCP server with an unsupported transport: {other:?}");
            None
        }
    }
}

fn header_map<'a>(
    headers: impl Iterator<Item = &'a agent_client_protocol::schema::v1::HttpHeader>,
) -> BTreeMap<String, String> {
    headers
        .map(|header| (header.name.clone(), header.value.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::schema::v1::{
        EnvVariable, HttpHeader, McpServerHttp, McpServerSse, McpServerStdio,
    };

    #[test]
    fn stdio_server_keeps_command_args_and_env() {
        let mut stdio = McpServerStdio::new("search", "/usr/local/bin/search-mcp");
        stdio.args = vec!["--stdio".into()];
        stdio.env = vec![EnvVariable::new("SEARCH_KEY", "secret")];

        let config = to_server_config(&McpServer::Stdio(stdio)).expect("stdio is supported");

        assert_eq!(config.name, "search");
        assert_eq!(config.command, "/usr/local/bin/search-mcp");
        assert_eq!(config.args, vec!["--stdio".to_string()]);
        assert_eq!(
            config.env.get("SEARCH_KEY").map(String::as_str),
            Some("secret")
        );
        assert!(config.url.is_none());
        assert!(config.enabled);
    }

    #[test]
    fn http_server_becomes_a_url_server_with_headers() {
        let http = McpServerHttp::new("remote-search", "https://mcp.example/rpc")
            .headers(vec![HttpHeader::new("X-Tenant", "acme")]);

        let config = to_server_config(&McpServer::Http(http)).expect("http is supported");

        assert_eq!(config.name, "remote-search");
        assert!(config.command.is_empty());
        assert_eq!(config.url.as_deref(), Some("https://mcp.example/rpc"));
        assert_eq!(
            config.headers.get("X-Tenant").map(String::as_str),
            Some("acme")
        );
    }

    #[test]
    fn sse_servers_are_skipped_rather_than_misregistered() {
        let sse = McpServer::Sse(McpServerSse::new("legacy", "https://mcp.example/sse"));

        assert!(to_server_config(&sse).is_none());
    }
}
