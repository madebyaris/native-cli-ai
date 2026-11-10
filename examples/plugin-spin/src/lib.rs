//! Test fixture: an agent hook that spins forever, plus an allocator hog, to
//! prove the host's epoch timeout and memory cap hold.

wit_bindgen::generate!({
    path: "../../crates/plugin/wit",
    world: "plugin",
});

use exports::nca::plugin::{agent, auth, cosmetic, subagent};
use nca::plugin::types::{Credential, SpawnDecision, SpawnRequest, Theme, ToolDecision};

struct Spin;

impl auth::Guest for Spin {
    fn resolve_credential(_: String) -> Result<Option<Credential>, String> {
        Ok(None)
    }
}
impl subagent::Guest for Spin {
    fn before_spawn(r: SpawnRequest) -> Result<SpawnDecision, String> {
        Ok(SpawnDecision::Allow(r))
    }
}
impl cosmetic::Guest for Spin {
    fn resolve_theme() -> Result<Theme, String> {
        Ok(Vec::new())
    }
}
impl agent::Guest for Spin {
    fn transform_system_prompt(_: String) -> Option<String> {
        None
    }
    fn before_tool_use(tool: String, _: String) -> ToolDecision {
        match tool.as_str() {
            // Allocate until the host's memory limit traps us.
            "hog" => {
                let mut hog: Vec<Vec<u8>> = Vec::new();
                loop {
                    hog.push(vec![1u8; 16 * 1024 * 1024]);
                    std::hint::black_box(&hog);
                }
            }
            _ => {
                let mut i: u64 = 0;
                loop {
                    i = std::hint::black_box(i.wrapping_add(1));
                }
            }
        }
    }
}

export!(Spin);
