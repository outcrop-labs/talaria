// Reading media out of agent containers — shared by the inline-image route
// and "save to artifacts". Guardrails live HERE so every consumer gets them:
// absolute paths inside /opt/data only (the agent's own volume, never the
// host), image types only, size-capped, slot-aware container resolution.
//
// Port of ui/src/server/agent-media.ts.

use sqlx::PgPool;

use crate::fleet_docker::managed_container;

fn mime_for(ext: &str) -> Option<&'static str> {
    match ext {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        _ => None,
    }
}

const MAX_BYTES: usize = 25 * 1024 * 1024;

/// A typed error the route passes through with its own status.
pub struct MediaError {
    pub error: &'static str,
    pub status: u16,
}

pub struct AgentImage {
    pub bytes: Vec<u8>,
    pub mime: &'static str,
}

/// Read one image from the agent's container. Returns bytes + mime, or a
/// typed error the route can pass through.
pub async fn read_agent_image(
    pg: &PgPool,
    model: &str,
    path: &str,
) -> Result<AgentImage, MediaError> {
    if !path.starts_with("/opt/data/") || path.contains("..") || path.contains('\0') {
        return Err(MediaError {
            error: "only files under /opt/data",
            status: 400,
        });
    }
    let ext = path.rsplit('.').next().unwrap_or("").to_lowercase();
    let Some(mime) = mime_for(&ext) else {
        return Err(MediaError {
            error: "images only (png/jpg/gif/webp)",
            status: 415,
        });
    };

    let department: Option<(String,)> =
        sqlx::query_as("select department from agent_defs where model = $1 and managed")
            .bind(model)
            .fetch_optional(pg)
            .await
            .map_err(|_| MediaError {
                error: "unknown agent",
                status: 404,
            })?;
    let Some((department,)) = department else {
        return Err(MediaError {
            error: "unknown agent",
            status: 404,
        });
    };

    let read = async {
        let name = managed_container(pg, &department).await;
        tokio::time::timeout(std::time::Duration::from_millis(30_000), async {
            let out = tokio::process::Command::new("docker")
                .args(["exec", &name, "cat", path])
                .stdin(std::process::Stdio::null())
                .output()
                .await
                .map_err(|e| e.to_string())?;
            if !out.status.success() {
                return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
            }
            if out.stdout.len() > MAX_BYTES {
                // node's maxBuffer kill, same refusal.
                return Err("output exceeds maxBuffer".to_string());
            }
            Ok(out.stdout)
        })
        .await
        .map_err(|_| "timed out".to_string())?
    }
    .await;
    match read {
        Ok(bytes) => Ok(AgentImage { bytes, mime }),
        Err(_) => Err(MediaError {
            error: "file not found in the agent container",
            status: 404,
        }),
    }
}
