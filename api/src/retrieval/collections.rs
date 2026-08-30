// The RAG collection registry. Port of ui/src/server/retrieval/collections.ts.
// Talaria spins up as many collections as needed; two are auto-provisioned
// and undeletable:
//   activity  — the ambient workspace index (chats/channels/plans/research/),
//               a retrieval TOOL agents call on demand (never auto-loaded)
//   org-kb    — the curated knowledgebase; grounds agents by default
// Others are custom (departmental etc.), bound to users/agents/groups.
// EVERY collection — auto ones included — is reached through a row in
// rag_collection_access; there is no unconditional path to any of them.

use std::sync::atomic::{AtomicBool, Ordering};

use sqlx::{PgPool, Row};

use crate::retrieval::embed::{EmbedDeps, embed_dim};
use crate::retrieval::qdrant::{
    QdrantDeps, delete_collection, ensure_collection, ensure_hybrid_collection,
};

/// Ensure a collection's Qdrant collection exists in its registered shape.
pub async fn ensure_qdrant_for(
    qd: &QdrantDeps,
    qdrant_name: &str,
    schema_version: i64,
    dim: i64,
) -> Result<(), String> {
    if schema_version >= 2 {
        ensure_hybrid_collection(qd, qdrant_name, dim).await
    } else {
        ensure_collection(qd, qdrant_name, dim).await
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RagCollection {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub qdrant_name: String,
    pub description: Option<String>,
    pub auto: bool,
    pub created_by: Option<String>,
    pub created_at: String,
    /// 1 = legacy unnamed dense; 2 = hybrid (named dense + IDF sparse).
    pub schema_version: i64,
}

/// Every query below reads the same nine columns; one parser so no two of
/// them can disagree about what a row is.
fn col_of(row: &sqlx::postgres::PgRow) -> RagCollection {
    RagCollection {
        id: row.get("id"),
        name: row.get("name"),
        kind: row.get("kind"),
        qdrant_name: row.get("qdrant_name"),
        description: row.get("description"),
        auto: row.get("auto"),
        created_by: row.get("created_by"),
        created_at: row.get("created_at"),
        schema_version: row.get("schema_version"),
    }
}

const COL_SELECT: &str = "select id::text, name, kind, qdrant_name, description, auto, \
                          created_by, created_at::text, schema_version";

#[derive(Debug, Clone, PartialEq)]
pub struct AccessBinding {
    pub principal_type: String, // 'all' | 'user' | 'agent' | 'team'
    pub principal_id: Option<String>,
}

/// The two auto collections, in insert order.
const AUTO: &[(&str, &str, &str, &str)] = &[
    (
        "Workspace activity",
        "activity",
        "talaria_activity",
        "Ambient index of chats, channels, plans, research, and ticket discussion. Searched on demand.",
    ),
    (
        "Organization knowledge",
        "org-kb",
        "talaria_org_kb",
        "The curated knowledgebase: official docs and artifacts. Grounds agents by default.",
    ),
];

/// `Date.now().toString(36)` — the clash-avoidance suffix, base 36 lowercase.
fn base36(millis: u128) -> String {
    if millis == 0 {
        return "0".into();
    }
    const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut out = Vec::new();
    let mut n = millis;
    while n > 0 {
        out.push(DIGITS[(n % 36) as usize]);
        n /= 36;
    }
    out.reverse();
    String::from_utf8(out).expect("base-36 digits are ascii")
}

/// `autoBindingsChecked` — the process flag that keeps the binding repair off
/// the hot path once it has run once. Set only by a successful pass: a failed
/// one retries on the next call, exactly the TS catch-and-move-on.
static AUTO_BINDINGS_CHECKED: AtomicBool = AtomicBool::new(false);

/// Create the two auto collections + their Qdrant collections if missing.
/// Skips (and retries next call) while the embedding service is down — a
/// guessed dimension would poison the registry (it happened: rows stamped
/// 1024 while the live collections were 384).
pub async fn ensure_auto_collections(
    pg: &PgPool,
    qd: &QdrantDeps,
    ed: &EmbedDeps,
) -> Result<(), String> {
    let dim = embed_dim(ed).await.unwrap_or_default();
    if dim == 0 {
        return Ok(());
    }
    for (name, kind, qdrant_name, description) in AUTO {
        let _ = ensure_hybrid_collection(qd, qdrant_name, dim as i64).await;
        sqlx::query(
            "insert into rag_collections (name, kind, qdrant_name, description, auto, embed_dim, schema_version) \
             values ($1, $2, $3, $4, true, $5, 2) \
             on conflict (qdrant_name) do nothing",
        )
        .bind(name)
        .bind(kind)
        .bind(qdrant_name)
        .bind(description)
        .bind(dim as i64)
        .execute(pg)
        .await
        .map_err(|e| e.to_string())?;
    }
    ensure_auto_bindings(pg).await
}

/// Auto collections are reachable through a REAL binding like everything
/// else: principal_type 'all' — i.e. every KNOWN member of this workspace (a
/// resolved user or a registered agent), which is not the same as "any caller
/// holding the fleet key". Access used to be an unconditional `c.auto = true`
/// in the accessible-collections query, which handed both auto collections to
/// any x-agent-name at all. Repair is one idempotent statement, so it also
/// fixes registries created before this became a binding.
pub async fn ensure_auto_bindings(pg: &PgPool) -> Result<(), String> {
    // `unique (collection_id, principal_type, principal_id)` doesn't dedupe
    // rows with a NULL principal_id, so guard on not-exists rather than on
    // conflict.
    sqlx::query(
        "insert into rag_collection_access (collection_id, principal_type, principal_id) \
         select c.id, 'all', null from rag_collections c \
         where c.auto \
           and not exists ( \
             select 1 from rag_collection_access a \
             where a.collection_id = c.id and a.principal_type = 'all' \
           )",
    )
    .execute(pg)
    .await
    .map_err(|e| e.to_string())?;
    AUTO_BINDINGS_CHECKED.store(true, Ordering::Relaxed);
    Ok(())
}

pub async fn list_collections(
    pg: &PgPool,
) -> Result<Vec<(RagCollection, Vec<AccessBinding>)>, sqlx::Error> {
    // AssertSqlSafe: the only interpolated fragment is this crate's COL_SELECT column list.
    let cols = sqlx::query(sqlx::AssertSqlSafe(format!(
        "{COL_SELECT} from rag_collections where kind <> 'personal' order by auto desc, name asc"
    )))
    .fetch_all(pg)
    .await?
    .iter()
    .map(col_of)
    .collect::<Vec<_>>();
    let access = sqlx::query(
        "select collection_id::text as collection_id, principal_type, principal_id \
         from rag_collection_access",
    )
    .fetch_all(pg)
    .await?;
    let mut bindings: Vec<(String, AccessBinding)> = Vec::new();
    for row in access {
        bindings.push((
            row.get::<String, _>("collection_id"),
            AccessBinding {
                principal_type: row.get("principal_type"),
                principal_id: row.get("principal_id"),
            },
        ));
    }
    Ok(cols
        .into_iter()
        .map(|c| {
            let mine = bindings
                .iter()
                .filter(|(id, _)| *id == c.id)
                .map(|(_, b)| b.clone())
                .collect();
            (c, mine)
        })
        .collect())
}

pub async fn get_collection(pg: &PgPool, id: &str) -> Result<Option<RagCollection>, sqlx::Error> {
    // AssertSqlSafe: the interpolation is this crate's COL_SELECT column list.
    let row = sqlx::query(sqlx::AssertSqlSafe(format!(
        "{COL_SELECT} from rag_collections where id::text = $1"
    )))
    .bind(id)
    .fetch_optional(pg)
    .await?;
    Ok(row.as_ref().map(col_of))
}

/// `'talaria_' + lower` with non-alphanumeric RUNS folded to a single `_`
/// (the TS class carries a `+` — "Department!! of" is department_of, not
/// department___of), separators trimmed at both ends, 40 chars. A slice that
/// lands mid-word can leave a trailing `_` — the TS doesn't re-trim, so
/// neither does this.
fn slugify(s: &str) -> String {
    let mut folded = String::with_capacity(s.len());
    let mut in_run = false;
    for c in s.to_lowercase().chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            folded.push(c);
            in_run = false;
        } else if !in_run {
            folded.push('_');
            in_run = true;
        }
    }
    let trimmed = folded.trim_matches('_');
    format!("talaria_{}", crate::body::truncate_utf16(trimmed, 40))
}

pub struct CreateCollection<'a> {
    pub name: &'a str,
    pub description: Option<&'a str>,
    pub created_by: &'a str,
    pub bindings: Option<&'a [AccessBinding]>,
}

pub async fn create_collection(
    pg: &PgPool,
    qd: &QdrantDeps,
    ed: &EmbedDeps,
    input: &CreateCollection<'_>,
) -> Result<RagCollection, String> {
    // A down embedding service fails the create — same as the TS, which lets
    // embedDim() throw rather than register a collection it cannot fill.
    let dim = embed_dim(ed).await?;
    let mut qdrant_name = slugify(input.name);
    // Avoid a name clash with an existing collection.
    let clash =
        sqlx::query_scalar::<_, i32>("select 1 from rag_collections where qdrant_name = $1")
            .bind(&qdrant_name)
            .fetch_optional(pg)
            .await
            .map_err(|e| e.to_string())?;
    if clash.is_some() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        qdrant_name = format!("{qdrant_name}_{}", base36(now));
    }
    ensure_hybrid_collection(qd, &qdrant_name, dim as i64).await?;
    // `returning` carries the text casts itself — one statement, no subselect
    // (an insert may not sit in a plain subquery in postgres).
    let row = sqlx::query(
        "insert into rag_collections (name, kind, qdrant_name, description, auto, embed_dim, created_by, schema_version) \
         values ($1, 'custom', $2, $3, false, $4, $5, 2) \
         returning id::text, name, kind, qdrant_name, description, auto, created_by, created_at::text, schema_version",
    )
    .bind(input.name)
    .bind(&qdrant_name)
    .bind(input.description)
    .bind(dim as i64)
    .bind(input.created_by)
    .fetch_one(pg)
    .await
    .map_err(|e| e.to_string())?;
    let col = col_of(&row);
    let default = vec![AccessBinding {
        principal_type: "all".into(),
        principal_id: None,
    }];
    set_bindings(pg, &col.id, input.bindings.unwrap_or(&default)).await?;
    Ok(col)
}

/// The user's personal RAG collection, if they have one.
pub async fn personal_collection_for(
    pg: &PgPool,
    user_id: &str,
) -> Result<Option<RagCollection>, sqlx::Error> {
    // AssertSqlSafe: the interpolation is this crate's COL_SELECT column list.
    let row = sqlx::query(sqlx::AssertSqlSafe(format!(
        "{COL_SELECT} from rag_collections where kind = 'personal' and owner_user_id = $1::uuid limit 1"
    )))
    .bind(user_id)
    .fetch_optional(pg)
    .await?;
    Ok(row.as_ref().map(col_of))
}

pub struct PersonalOpts<'a> {
    pub name: Option<&'a str>,
    pub agent_model: Option<&'a str>,
}

/// Create (or return) a user's personal RAG collection, bound to them and —
/// when given — their personal agent. Their private KB docs sync here; nobody
/// else is bound, so nobody else can retrieve from it.
pub async fn ensure_personal_collection(
    pg: &PgPool,
    qd: &QdrantDeps,
    ed: &EmbedDeps,
    user_id: &str,
    opts: &PersonalOpts<'_>,
) -> Result<RagCollection, String> {
    if let Some(existing) = personal_collection_for(pg, user_id)
        .await
        .map_err(|e| e.to_string())?
    {
        if let Some(agent) = opts.agent_model {
            add_binding(
                pg,
                &existing.id,
                &AccessBinding {
                    principal_type: "agent".into(),
                    principal_id: Some(agent.into()),
                },
            )
            .await?;
        }
        return Ok(existing);
    }
    let dim = embed_dim(ed).await?;
    // `-` stripped from the uuid so the qdrant name is a legal identifier
    // prefix; truncated to keep the whole name well under qdrant's limit.
    let bare: String = user_id.chars().filter(|c| *c != '-').collect();
    let qdrant_name = format!(
        "talaria_personal_{}",
        crate::body::truncate_utf16(&bare, 24)
    );
    ensure_hybrid_collection(qd, &qdrant_name, dim as i64).await?;
    let row = sqlx::query(
        "insert into rag_collections (name, kind, qdrant_name, description, auto, embed_dim, created_by, owner_user_id, schema_version) \
         values ($1, 'personal', $2, 'Your private docs, visible only to you and your personal assistant.', false, $3, $4::uuid, $4::uuid, 2) \
         returning id::text, name, kind, qdrant_name, description, auto, created_by, created_at::text, schema_version",
    )
    .bind(opts.name.unwrap_or("My knowledge"))
    .bind(&qdrant_name)
    .bind(dim as i64)
    .bind(user_id)
    .fetch_one(pg)
    .await
    .map_err(|e| e.to_string())?;
    let col = col_of(&row);
    let mut bindings = vec![AccessBinding {
        principal_type: "user".into(),
        principal_id: Some(user_id.into()),
    }];
    if let Some(agent) = opts.agent_model {
        bindings.push(AccessBinding {
            principal_type: "agent".into(),
            principal_id: Some(agent.into()),
        });
    }
    set_bindings(pg, &col.id, &bindings).await?;
    Ok(col)
}

/// Add a single access binding without disturbing the others.
async fn add_binding(
    pg: &PgPool,
    collection_id: &str,
    binding: &AccessBinding,
) -> Result<(), String> {
    sqlx::query(
        "insert into rag_collection_access (collection_id, principal_type, principal_id) \
         values ($1::uuid, $2, $3) on conflict do nothing",
    )
    .bind(collection_id)
    .bind(&binding.principal_type)
    .bind(&binding.principal_id)
    .execute(pg)
    .await
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Ok(()) on deleted-or-absent; Err carries the refusal sentence.
pub async fn delete_collection_by_id(pg: &PgPool, qd: &QdrantDeps, id: &str) -> Result<(), String> {
    let Some(col) = get_collection(pg, id).await.map_err(|e| e.to_string())? else {
        return Ok(());
    };
    if col.auto {
        return Err("auto collections cannot be deleted".into());
    }
    delete_collection(qd, &col.qdrant_name).await;
    sqlx::query("delete from rag_collections where id::text = $1")
        .bind(id)
        .execute(pg)
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

pub async fn set_bindings(
    pg: &PgPool,
    collection_id: &str,
    bindings: &[AccessBinding],
) -> Result<(), String> {
    let mut tx = pg.begin().await.map_err(|e| e.to_string())?;
    sqlx::query("delete from rag_collection_access where collection_id = $1::uuid")
        .bind(collection_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
    for b in bindings {
        sqlx::query(
            "insert into rag_collection_access (collection_id, principal_type, principal_id) \
             values ($1::uuid, $2, $3) on conflict do nothing",
        )
        .bind(collection_id)
        .bind(&b.principal_type)
        .bind(&b.principal_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
    }
    tx.commit().await.map_err(|e| e.to_string())?;
    Ok(())
}

/// Resolve a principal against the directories. The fleet key authenticates
/// the FLEET, not an identity: x-agent-name is self-declared, so a name that
/// isn't a registered agent must resolve to nothing at all. Same for a user
/// id that isn't a user. Returns the sentinels the access query compares
/// against — '' meaning "unresolved", which no binding can match.
async fn resolve_principal(
    pg: &PgPool,
    user_id: Option<&str>,
    agent_model: Option<&str>,
) -> Result<(String, String), sqlx::Error> {
    let uid = match user_id {
        Some(uid) => {
            let known = sqlx::query_scalar::<_, i32>("select 1 from users where id::text = $1")
                .bind(uid)
                .fetch_optional(pg)
                .await?
                .is_some();
            if known {
                uid.to_string()
            } else {
                String::new()
            }
        }
        None => String::new(),
    };
    let agent = match agent_model {
        Some(agent) => {
            let known = sqlx::query_scalar::<_, i32>("select 1 from agent_defs where model = $1")
                .bind(agent)
                .fetch_optional(pg)
                .await?
                .is_some();
            if known {
                agent.to_string()
            } else {
                String::new()
            }
        }
        None => String::new(),
    };
    Ok((uid, agent))
}

/// The collections a principal may search — every one of them via an explicit
/// binding: 'all' (any resolved member of the workspace, which is how the two
/// auto collections are reachable) + those bound to this user, their teams,
/// or this agent. An unresolvable principal gets NOTHING; item-level
/// filtering in index::search_for_principal then narrows within each.
pub async fn collections_for_principal(
    pg: &PgPool,
    user_id: Option<&str>,
    agent_model: Option<&str>,
) -> Result<Vec<RagCollection>, sqlx::Error> {
    if !AUTO_BINDINGS_CHECKED.load(Ordering::Relaxed) {
        let _ = ensure_auto_bindings(pg).await;
    }
    // Empty-string sentinels never match a real user id / agent model.
    let (uid, agent) = resolve_principal(pg, user_id, agent_model).await?;
    if uid.is_empty() && agent.is_empty() {
        return Ok(Vec::new());
    }
    // AssertSqlSafe: the interpolation is this crate's COL_SELECT column list.
    let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
        "{COL_SELECT} from ( \
           select distinct c.id, c.name, c.kind, c.qdrant_name, c.description, c.auto, \
                  c.created_by, c.created_at, c.schema_version \
           from rag_collections c \
           join rag_collection_access a on a.collection_id = c.id \
           where a.principal_type = 'all' \
              or (a.principal_type = 'user' and $1 <> '' and a.principal_id = $1) \
              or (a.principal_type = 'agent' and $2 <> '' and a.principal_id = $2) \
              or (a.principal_type = 'team' and $1 <> '' and exists ( \
                    /* text-side compare: the '' sentinel must not hit a uuid cast */ \
                    select 1 from team_members tm where tm.team_id::text = a.principal_id and tm.user_id::text = $1 \
                  )) \
           order by c.name asc \
         ) t"
    )))
    .bind(&uid)
    .bind(&agent)
    .fetch_all(pg)
    .await?;
    Ok(rows.iter().map(col_of).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_folds_trims_and_caps_at_forty() {
        assert_eq!(slugify("Research Brain"), "talaria_research_brain");
        assert_eq!(
            slugify("  --Department!! of  Stuff--  "),
            "talaria_department_of_stuff"
        );
        // Long names cap at 40 (the cap is on the folded part, after the
        // 'talaria_' prefix is decided) and keep a trailing `_` when the cut
        // lands there — the TS does not re-trim.
        let long = "a".repeat(80);
        assert_eq!(slugify(&long), format!("talaria_{}", "a".repeat(40)));
        assert_eq!(slugify(""), "talaria_");
        assert_eq!(slugify("---"), "talaria_");
    }

    #[test]
    fn base36_matches_the_js_spelling() {
        // Date.now().toString(36) — lowercase, no padding.
        assert_eq!(base36(0), "0");
        assert_eq!(base36(35), "z");
        assert_eq!(base36(36), "10");
        assert_eq!(base36(1_759_000_000_000), "mg2n6mf4"); // a plausible 2026 millis
    }

    #[test]
    fn the_auto_pair_carries_its_descriptions() {
        assert_eq!(AUTO.len(), 2);
        assert_eq!(AUTO[0].1, "activity");
        assert_eq!(AUTO[1].1, "org-kb");
        // The descriptions are admin-visible copy — pin them.
        assert!(AUTO[0].3.contains("Searched on demand"));
        assert!(AUTO[1].3.contains("Grounds agents by default"));
    }
}
