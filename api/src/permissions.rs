// Fine-grained permissions — port of ui/src/server/permissions.ts. Three
// layers, most specific wins:
//   1. per-user overrides        (user_permissions rows — allow or deny)
//   2. org-wide member defaults  (app_settings 'member_default_permissions')
//   3. the catalog's shipped defaults below
// The label/hint/group strings are the admin console's copy — byte-parity with
// the TS literals, and the 13-entry ORDER is pinned twice over: it is the wire
// order of the GET catalog, of a member's resolved perms array, and of the
// z.enum option list in the PUT body's error message.

use crate::gateway::settings::{get_setting, set_setting};
use sqlx::PgPool;
use std::collections::HashMap;

/// One catalog entry (permissions.ts PERMISSIONS) — wire order is the field
/// order the SPA consumes.
#[derive(serde::Serialize)]
pub struct PermCat {
    pub id: &'static str,
    pub label: &'static str,
    pub hint: &'static str,
    pub group: &'static str,
    #[serde(rename = "memberDefault")]
    pub member_default: bool,
}

pub const PERMISSIONS: [PermCat; 13] = [
    PermCat {
        id: "agents.manage",
        label: "Manage agents",
        hint: "Hire, retire, and configure org agents: souls, skills, crons, start/stop. Agent secrets and infrastructure stay admin-only.",
        group: "Agents",
        member_default: false,
    },
    PermCat {
        id: "research.run",
        label: "Run research",
        hint: "Start research runs (recon, briefs, expeditions).",
        group: "Work",
        member_default: true,
    },
    PermCat {
        id: "plans.create",
        label: "Create plans",
        hint: "Start plan conversations and their living documents.",
        group: "Work",
        member_default: true,
    },
    PermCat {
        id: "boards.create",
        label: "Create boards",
        hint: "Create new boards. Working on boards they belong to is membership, not this.",
        group: "Work",
        member_default: true,
    },
    PermCat {
        id: "comms.channels",
        label: "Create channels",
        hint: "Create persistent channels. Joining and posting is membership.",
        group: "Comms",
        member_default: true,
    },
    PermCat {
        id: "comms.relays",
        label: "Start relays",
        hint: "Spin up relays — ephemeral working groups that conclude and archive.",
        group: "Comms",
        member_default: true,
    },
    PermCat {
        id: "kb.edit",
        label: "Edit knowledge",
        hint: "Create and edit knowledge docs (per-doc/space ACLs still apply).",
        group: "Content",
        member_default: true,
    },
    PermCat {
        id: "kb.official",
        label: "Curate knowledge",
        hint: "Create spaces and mark docs OFFICIAL — content that grounds every agent.",
        group: "Content",
        member_default: false,
    },
    PermCat {
        id: "artifacts.create",
        label: "Create documents",
        hint: "Create documents and artifacts.",
        group: "Content",
        member_default: true,
    },
    PermCat {
        id: "artifacts.publish",
        label: "Publish to the web",
        hint: "Make artifacts PUBLIC — reachable by anyone with the link, outside the org.",
        group: "Content",
        member_default: false,
    },
    PermCat {
        id: "files.upload",
        label: "Upload files",
        hint: "Attach files and images to chats, channels, and tickets.",
        group: "Content",
        member_default: true,
    },
    PermCat {
        id: "templates.manage",
        label: "Manage templates",
        hint: "Edit the org-wide ticket and plan templates everyone starts from.",
        group: "Content",
        member_default: false,
    },
    PermCat {
        id: "models.mint-keys",
        label: "Mint API keys",
        hint: "Mint personal LLM-gateway API keys for external tools.",
        group: "Models",
        member_default: false,
    },
];

pub const PERM_IDS: [&str; 13] = [
    "agents.manage",
    "research.run",
    "plans.create",
    "boards.create",
    "comms.channels",
    "comms.relays",
    "kb.edit",
    "kb.official",
    "artifacts.create",
    "artifacts.publish",
    "files.upload",
    "templates.manage",
    "models.mint-keys",
];

const ORG_DEFAULTS_KEY: &str = "member_default_permissions";

/// Org-tuned member defaults (admin-editable), sparse over the catalog. The
/// RAW setting value rides the wire — getOrgDefaultPerms is a passthrough, so
/// whatever jsonb is stored (order included) is what the admin console sees.
pub async fn get_org_default_perms(pg: &PgPool) -> serde_json::Value {
    get_setting(
        pg,
        ORG_DEFAULTS_KEY,
        serde_json::Value::Object(serde_json::Map::new()),
    )
    .await
}

/// Tune one member default; `None` is the TS null = back to the shipped
/// default (key deleted from the sparse map). Key order of the stored object
/// is preserved for the other entries — delete/set on the same Map, write the
/// whole thing back.
pub async fn set_org_default_perm(
    pg: &PgPool,
    perm: &str,
    enabled: Option<bool>,
) -> Result<(), sqlx::Error> {
    let mut cur = get_org_default_perms(pg)
        .await
        .as_object()
        .cloned()
        .unwrap_or_default();
    match enabled {
        None => {
            cur.remove(perm);
        }
        Some(v) => {
            cur.insert(perm.to_string(), serde_json::Value::Bool(v));
        }
    }
    set_setting(pg, ORG_DEFAULTS_KEY, &serde_json::Value::Object(cur)).await
}

/// One user's overrides as a wire-ready {perm: allowed} object, in row order
/// (Object.fromEntries — later duplicate rows win, same as the map insert).
pub async fn get_user_perm_overrides(
    pg: &PgPool,
    user_id: &str,
) -> Result<serde_json::Value, sqlx::Error> {
    let rows: Vec<(String, bool)> =
        sqlx::query_as("select perm, allowed from user_permissions where user_id = $1::uuid")
            .bind(user_id)
            .fetch_all(pg)
            .await?;
    let mut out = serde_json::Map::new();
    for (perm, allowed) in rows {
        out.insert(perm, serde_json::Value::Bool(allowed));
    }
    Ok(serde_json::Value::Object(out))
}

/// Set or clear one override (None = delete the row, back to org default).
pub async fn set_user_perm_override(
    pg: &PgPool,
    user_id: &str,
    perm: &str,
    allowed: Option<bool>,
) -> Result<(), sqlx::Error> {
    match allowed {
        None => {
            sqlx::query("delete from user_permissions where user_id = $1::uuid and perm = $2")
                .bind(user_id)
                .bind(perm)
                .execute(pg)
                .await?;
        }
        Some(v) => {
            sqlx::query(
                "insert into user_permissions (user_id, perm, allowed) values ($1::uuid, $2, $3) \
                 on conflict (user_id, perm) do update set allowed = $3",
            )
            .bind(user_id)
            .bind(perm)
            .bind(v)
            .execute(pg)
            .await?;
        }
    }
    Ok(())
}

/// The user's effective permission set: per-user override → org default → the
/// catalog's shipped default. Admins: everything, in catalog order.
pub async fn user_permissions(
    pg: &PgPool,
    user_id: &str,
    role: &str,
) -> Result<Vec<&'static str>, sqlx::Error> {
    if role == "admin" {
        return Ok(PERM_IDS.to_vec());
    }
    let org = get_org_default_perms(pg).await;
    let org = org.as_object().cloned().unwrap_or_default();
    let rows: Vec<(String, bool)> =
        sqlx::query_as("select perm, allowed from user_permissions where user_id = $1::uuid")
            .bind(user_id)
            .fetch_all(pg)
            .await?;
    // Object.fromEntries: later duplicate rows win, same as the map insert.
    let overrides: HashMap<String, bool> = rows.into_iter().collect();
    Ok(PERMISSIONS
        .iter()
        .filter(|p| {
            overrides
                .get(p.id)
                .copied()
                .or_else(|| org.get(p.id).and_then(|v| v.as_bool()))
                .unwrap_or(p.member_default)
        })
        .map(|p| p.id)
        .collect())
}

/// One permission, resolved through the same override → org-default →
/// catalog chain (permissions.ts hasPerm). Admins: everything.
pub async fn has_perm(
    pg: &PgPool,
    user_id: &str,
    role: &str,
    perm: &str,
) -> Result<bool, sqlx::Error> {
    if role == "admin" {
        return Ok(true);
    }
    let rows: Vec<(String, bool)> =
        sqlx::query_as("select perm, allowed from user_permissions where user_id = $1::uuid")
            .bind(user_id)
            .fetch_all(pg)
            .await?;
    let overrides: HashMap<String, bool> = rows.into_iter().collect();
    let org = get_org_default_perms(pg).await;
    Ok(overrides
        .get(perm)
        .copied()
        .or_else(|| {
            org.as_object()
                .and_then(|o| o.get(perm))
                .and_then(|v| v.as_bool())
        })
        .or_else(|| {
            PERMISSIONS
                .iter()
                .find(|p| p.id == perm)
                .map(|p| p.member_default)
        })
        .unwrap_or(false))
}
