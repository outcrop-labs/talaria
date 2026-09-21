use futures_util::future::BoxFuture;
use serde_json::{Map, Value};
use sqlx::PgPool;
use std::sync::{Arc, OnceLock};
use talaria_secretbox::SecretBox;

pub static CALL_MCP_TOOL: OnceLock<
    Arc<
        dyn Fn(
                PgPool,
                SecretBox,
                String,
                String,
                Map<String, Value>,
            ) -> BoxFuture<'static, Result<(String, Option<Value>), String>>
            + Send
            + Sync,
    >,
> = OnceLock::new();

pub mod defs;
pub mod registry;
