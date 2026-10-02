//! Werk agent harness core: loop, tools, permissions, compaction.
//! Pure logic only — no Tauri, no globals, no filesystem outside injected roots.

pub mod agent;
pub mod client;
pub mod compact;
pub mod jsonfix;
pub mod lsp;
pub mod mcp;
pub mod memory;
pub mod permissions;
pub mod plugins;
pub mod sandbox;
pub mod skills;
pub mod todos;
pub mod tools;

pub use agent::{AgentEvent, AgentRun, ApprovalGate, ApprovalRequest, Approved, VerifyMode};
pub use permissions::{PermissionEngine, Scope};
pub use sandbox::PathJail;

/// Placeholder until M1 lands the loop. Proves the crate builds and tests run.
pub fn ping() -> &'static str {
    "pong"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ping_replies() {
        assert_eq!(ping(), "pong");
    }
}
