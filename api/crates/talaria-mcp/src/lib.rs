use serde_json::Value;
use std::sync::{Arc, OnceLock};

pub static WORKBENCH_TOOLS: OnceLock<Arc<dyn Fn() -> Vec<Value> + Send + Sync>> = OnceLock::new();

pub fn workbench_tools() -> Vec<Value> {
    WORKBENCH_TOOLS.get().map(|f| f()).unwrap_or_default()
}

pub mod pkg;
pub mod registry;
