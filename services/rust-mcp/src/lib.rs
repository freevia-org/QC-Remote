mod actions;
mod backend;
mod generated_instructions;
mod generated_result_kinds;
mod schema;
mod server;

pub use actions::{ACTIONS, ActionSpec, Classification};
pub use backend::{BackendError, PrincipalRoute, QcBackend};
pub use generated_instructions::{ASSISTANT_GUIDE, MCP_INSTRUCTIONS};
pub use server::{
    ClientPresence, QcMcp, as_resource_read, mcp_router, mcp_router_with_allowed_hosts,
    resource_rpcs, serving_resource_read,
};
