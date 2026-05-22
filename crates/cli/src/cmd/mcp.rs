//! MCP server configuration listing.

use crate::cmd::util::print_json;
use nca_common::config::NcaConfig;

pub fn list_mcp_servers(config: &NcaConfig, json: bool) -> anyhow::Result<()> {
    if json {
        print_json(&config.mcp, false)?;
    } else if config.mcp.servers.is_empty() {
        println!("No MCP servers configured");
    } else {
        for server in config.mcp.servers.iter().filter(|server| server.enabled) {
            println!(
                "{}  command={} {}",
                server.name,
                server.command,
                server.args.join(" ")
            );
        }
    }
    Ok(())
}
