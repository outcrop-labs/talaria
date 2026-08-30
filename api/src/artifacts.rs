// Artifacts — versioned work products (doc / sheet / microsite / file) with
// their own hosting, sharing, and version history. Sharing reuses the KB
// permission model (kb_editors with item_type='artifact' + visibility / edit
// policy); versioning reuses internal_versions (kind 'artifact'). Flat, so no
// folder inheritance — an artifact carries its own audience.
//
// Port of ui/src/server/artifacts.ts, read half: the row shape, the reader,
// and the markdown rendering the refs cone clips into chips. The write plane
// (create/save/versions/the official→KB mirror) is batch 5 and extends this
// file in place.

use sqlx::PgPool;

use crate::agent_auth::epoch_ms_to_iso;
use crate::kb_perms::Guarded;

/// An artifact (artifacts.ts Artifact) — full row shape, so batch 5 only adds
/// functions, never fields.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Artifact {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub icon: Option<String>,
    pub body: String,
    pub content_type: Option<String>,
    pub storage_ref: Option<String>,
    pub visibility: String,
    pub edit_policy: String,
    pub public_slug: Option<String>,
    pub official: bool,
    pub kb_doc_id: Option<String>,
    pub folder_id: Option<String>,
    pub owner_user_id: Option<String>,
    /// RAG routing: 'auto' (plan/research/official flows) | 'none' | a brain
    /// id.
    pub rag_routing: String,
    pub google_file_id: Option<String>,
    pub google_file_url: Option<String>,
    pub created_by: Option<String>,
    pub updated_by: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

const COLS: &str = "id::text, kind, title, icon, body, content_type, storage_ref, \
                    visibility, edit_policy, public_slug, official, kb_doc_id::text, folder_id::text, \
                    owner_user_id::text, rag_routing, google_file_id, google_file_url, \
                    created_by::text, updated_by::text, \
                    (trunc(extract(epoch from created_at) * 1000))::bigint as created_ms, \
                    (trunc(extract(epoch from updated_at) * 1000))::bigint as updated_ms";

#[derive(sqlx::FromRow)]
struct ArtifactRow {
    id: String,
    kind: String,
    title: String,
    icon: Option<String>,
    body: String,
    content_type: Option<String>,
    storage_ref: Option<String>,
    visibility: String,
    edit_policy: String,
    public_slug: Option<String>,
    official: bool,
    kb_doc_id: Option<String>,
    folder_id: Option<String>,
    owner_user_id: Option<String>,
    rag_routing: String,
    google_file_id: Option<String>,
    google_file_url: Option<String>,
    created_by: Option<String>,
    updated_by: Option<String>,
    created_ms: i64,
    updated_ms: i64,
}

impl From<ArtifactRow> for Artifact {
    fn from(r: ArtifactRow) -> Self {
        let ArtifactRow {
            id,
            kind,
            title,
            icon,
            body,
            content_type,
            storage_ref,
            visibility,
            edit_policy,
            public_slug,
            official,
            kb_doc_id,
            folder_id,
            owner_user_id,
            rag_routing,
            google_file_id,
            google_file_url,
            created_by,
            updated_by,
            created_ms,
            updated_ms,
        } = r;
        Artifact {
            id,
            kind,
            title,
            icon,
            body,
            content_type,
            storage_ref,
            visibility,
            edit_policy,
            public_slug,
            official,
            kb_doc_id,
            folder_id,
            owner_user_id,
            rag_routing,
            google_file_id,
            google_file_url,
            created_by,
            updated_by,
            created_at: epoch_ms_to_iso(created_ms),
            updated_at: epoch_ms_to_iso(updated_ms),
        }
    }
}

/// The same columns for queries that join (artifacts.ts COLS_A): the table
/// needs its alias, the row shape does not change.
const COLS_A: &str = "a.id::text, a.kind, a.title, a.icon, a.body, a.content_type, a.storage_ref, \
                      a.visibility, a.edit_policy, a.public_slug, a.official, a.kb_doc_id::text, \
                      a.folder_id::text, a.owner_user_id::text, a.rag_routing, a.google_file_id, \
                      a.google_file_url, a.created_by::text, a.updated_by::text, \
                      (trunc(extract(epoch from a.created_at) * 1000))::bigint as created_ms, \
                      (trunc(extract(epoch from a.updated_at) * 1000))::bigint as updated_ms";

/// Every artifact linked to a target (artifacts.ts artifactsForTarget) — the
/// join a plan surface walks to find its document, newest link first.
pub async fn artifacts_for_target(
    pg: &PgPool,
    target_type: &str,
    target_id: &str,
) -> Result<Vec<Artifact>, sqlx::Error> {
    // AssertSqlSafe: the interpolation is this crate's COLS_A column list.
    let sql = format!(
        "select {COLS_A} from artifacts a \
         join artifact_links l on l.artifact_id = a.id \
         where l.target_type = $1 and l.target_id = $2::uuid \
         order by l.created_at desc"
    );
    let rows: Vec<ArtifactRow> = sqlx::query_as(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(target_type)
        .bind(target_id)
        .fetch_all(pg)
        .await?;
    Ok(rows.into_iter().map(Artifact::from).collect())
}

/// The plan's linked document (plan-doc.ts planDocFor): the first `doc`
/// among the plan's linked artifacts. The document IS that artifact — there
/// is no separate model — and the draft reads it as seed context. The rest
/// of plan-doc.ts (ensure/sync/mentions) crosses with the chat family in
/// batch 5; this read crosses now because the plan-draft transcript wants
/// the document's current body beside the conversation.
pub async fn plan_doc_for(
    pg: &PgPool,
    conversation_id: &str,
) -> Result<Option<Artifact>, sqlx::Error> {
    Ok(artifacts_for_target(pg, "plan", conversation_id)
        .await?
        .into_iter()
        .find(|a| a.kind == "doc"))
}

/// The Guarded view a permission check needs.
pub fn guarded(a: &Artifact) -> Guarded {
    Guarded {
        owner_user_id: a.owner_user_id.clone(),
        created_by: a.created_by.clone(),
        visibility: a.visibility.clone(),
        edit_policy: a.edit_policy.clone(),
    }
}

pub async fn get_artifact(pg: &PgPool, id: &str) -> Result<Option<Artifact>, sqlx::Error> {
    // AssertSqlSafe: the interpolation is this crate's COLS column list.
    let sql = format!("select {COLS} from artifacts where id = $1::uuid");
    let row: Option<ArtifactRow> = sqlx::query_as(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(id)
        .fetch_optional(pg)
        .await?;
    Ok(row.map(Artifact::from))
}

/// Render an artifact's content as markdown for the KB mirror.
pub fn artifact_to_markdown(a: &Artifact) -> String {
    match a.kind.as_str() {
        "file" => String::new(), // no text body
        "sheet" => sheet_to_markdown_table(&a.body),
        _ => a.body.clone(), // doc + microsite are already text/markdown
    }
}

// ── The write half the brief's mirror exercises ───────────────────────────────
//
// Ported now because the mirror (daily-brief-artifact.ts) is a scheduled job
// with no route in front of it. The rest of the write plane (icon, storage,
// visibility + public slug, edit policy, delete) lands with the artifacts
// routes in batch 5 and extends this file in place.

/// createArtifact — kind/title carry their defaults ('doc', 'Untitled').
pub async fn create_artifact(
    pg: &PgPool,
    kind: Option<&str>,
    title: Option<&str>,
    created_by: &str,
    owner_user_id: Option<&str>,
    folder_id: Option<&str>,
) -> Result<Artifact, sqlx::Error> {
    // AssertSqlSafe: the interpolation is this crate's COLS column list.
    let sql = format!(
        "insert into artifacts (kind, title, created_by, updated_by, owner_user_id, folder_id) \
         values ($1, $2, $3::text, $3::text, $4::uuid, $5::uuid) returning {COLS}"
    );
    let row: ArtifactRow = sqlx::query_as(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(kind.unwrap_or("doc"))
        .bind(title.unwrap_or("Untitled"))
        .bind(created_by)
        .bind(owner_user_id)
        .bind(folder_id)
        .fetch_one(pg)
        .await?;
    Ok(Artifact::from(row))
}

/// createFolder — the shape the agent cabinet path uses: ownerless and
/// org-visible (the workspace's).
async fn create_folder(
    pg: &PgPool,
    name: &str,
    parent_id: Option<&str>,
    created_by: &str,
) -> Result<String, sqlx::Error> {
    let id: (String,) = sqlx::query_as(
        "insert into artifact_folders (name, parent_id, created_by, owner_user_id, visibility) \
         values ($1, $2::uuid, $3, null, 'org') returning id::text",
    )
    .bind(name)
    .bind(parent_id)
    .bind(created_by)
    .fetch_one(pg)
    .await?;
    Ok(id.0)
}

/// Find-or-create a folder by name under a parent (case-insensitive).
async fn find_or_create_folder(
    pg: &PgPool,
    name: &str,
    parent_id: Option<&str>,
    created_by: &str,
) -> Result<String, sqlx::Error> {
    let existing: Option<(String,)> = sqlx::query_as(
        "select id::text from artifact_folders \
         where lower(name) = $1 and parent_id is not distinct from $2::uuid limit 1",
    )
    .bind(name.to_lowercase())
    .bind(parent_id)
    .fetch_optional(pg)
    .await?;
    if let Some((id,)) = existing {
        return Ok(id);
    }
    create_folder(pg, name, parent_id, created_by).await
}

/// The single root every agent cabinet hangs under. One folder per agent at the
/// ROOT buried the user's own work under a wall of agent names the moment the
/// fleet grew; the Files browser now opens on your folders, with the whole
/// fleet's output one click away.
pub const AGENTS_ROOT: &str = "Agents";

/// The agent's filing cabinet: "Agents/&lt;Agent label&gt;/&lt;Category&gt;",
/// created on demand. Auto-created artifacts (plan docs, research reports,
/// agent documents, media saves, chat summaries, brief mirrors) file here
/// instead of piling up at the root. Never fails — filing must not be able to
/// kill the flow that creates the artifact; a None just means "root".
pub async fn agent_category_folder(
    pg: &PgPool,
    agent_label: &str,
    category: &str,
    created_by: &str,
) -> Option<String> {
    let root = find_or_create_folder(pg, AGENTS_ROOT, None, created_by)
        .await
        .ok()?;
    let top = find_or_create_folder(pg, agent_label, Some(&root), created_by)
        .await
        .ok()?;
    find_or_create_folder(pg, category, Some(&top), created_by)
        .await
        .ok()
}

/// attachArtifact — link an artifact to anything. `on conflict do nothing` is
/// the idempotency handle research's two-reports guard leans on: the link is
/// what makes a created artifact findable by the next entry, so it is written
/// in the same breath as the create with nothing between (see the research
/// def's `createReport`).
pub async fn attach_artifact(
    pg: &PgPool,
    artifact_id: &str,
    target_type: &str,
    target_id: &str,
    actor: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "insert into artifact_links (artifact_id, target_type, target_id, created_by) \
         values ($1::uuid, $2, $3::uuid, $4) on conflict do nothing",
    )
    .bind(artifact_id)
    .bind(target_type)
    .bind(target_id)
    .bind(actor)
    .execute(pg)
    .await?;
    Ok(())
}

/// The patch fields saveArtifact's brief-mirror call carries. The double
/// Option is TS's `!== undefined`: the outer layer says whether the field was
/// in the patch at all, the inner whether it is null.
#[derive(Default)]
pub struct SaveArtifactPatch<'a> {
    pub title: Option<Option<&'a str>>,
    pub body: Option<&'a str>,
    pub folder_id: Option<Option<&'a str>>,
    /// `'private' | 'org' | 'public'` — the research report's write step is the
    /// caller that carries it (ownership decides reach: a personal run's report
    /// stays private to its owner, an org run's publishes org-visible).
    pub visibility: Option<&'a str>,
}

/// saveArtifact, restricted to the patch legs the mirror exercises (see the
/// write-half note above): folder, title, body, then the re-read, the version
/// snapshot on a content change, and the official→KB re-mirror.
///
/// RECORDED DIVERGENCE (RUST-MIGRATION.md): the TS `remirrorIfOfficial` —
/// which pushes an official artifact's new body into its KB doc and re-routes
/// RAG — is a checked, loudly-logged NO-OP here until the retrieval plane
/// crosses in batch 5. The version snapshot IS ported, so history is whole;
/// what diverges is the KB/RAG copy of an official artifact edited through the
/// brief's mirror, which stays stale until that plane exists.
pub async fn save_artifact(
    pg: &PgPool,
    id: &str,
    patch: SaveArtifactPatch<'_>,
    actor: &str,
) -> Result<Option<Artifact>, sqlx::Error> {
    let Some(_prev) = get_artifact(pg, id).await? else {
        return Ok(None);
    };
    if let Some(folder) = patch.folder_id {
        sqlx::query(
            "update artifacts set folder_id = $2::uuid, updated_at = now() where id = $1::uuid",
        )
        .bind(id)
        .bind(folder)
        .execute(pg)
        .await?;
    }
    if let Some(title) = patch.title {
        sqlx::query("update artifacts set title = $2, updated_by = $3::text, updated_at = now() where id = $1::uuid")
            .bind(id)
            .bind(title)
            .bind(actor)
            .execute(pg)
            .await?;
    }
    if let Some(body) = patch.body {
        sqlx::query("update artifacts set body = $2, updated_by = $3::text, updated_at = now() where id = $1::uuid")
            .bind(id)
            .bind(body)
            .bind(actor)
            .execute(pg)
            .await?;
    }
    if let Some(visibility) = patch.visibility {
        sqlx::query("update artifacts set visibility = $2, updated_at = now() where id = $1::uuid")
            .bind(id)
            .bind(visibility)
            .execute(pg)
            .await?;
        // TS also mints a public_slug here when visibility becomes 'public'
        // and none exists. No Rust caller can reach that leg yet — the research
        // plane writes only 'private'/'org', and the artifacts routes (the
        // public-publishing surface) cross in batch 5, which is where the slug
        // mint lands with them.
    }
    let next = get_artifact(pg, id).await?;
    if let Some(a) = &next
        && (patch.body.is_some() || patch.title.is_some())
    {
        // Versions come free: every body change snapshots, so the
        // artifact's history is a rough record of how the day accumulated.
        let _ = crate::internal_history::snapshot(
            pg,
            "artifact",
            &a.id,
            &format!("# {}\n\n{}", a.title, a.body),
            Some(actor),
        )
        .await;
        // The recorded divergence: TS would push this body into the KB doc
        // (saveDoc → syncDocEffective → RAG re-route). That plane has not
        // crossed; the artifact and its KB mirror will diverge until it
        // does, and this line is how anyone finds out why from the logs.
        if a.official && a.kb_doc_id.is_some() {
            tracing::warn!(
                "[daily-brief] artifact {} is official with a KB doc; the KB re-mirror is not ported yet (batch 5) — the KB copy is now stale",
                a.id
            );
        }
    }
    Ok(next)
}

/// JS `String(c ?? '')` — how a sheet cell stringifies. Numbers and booleans
/// render as themselves, null as empty, an object as "[object Object]", an
/// array as its comma-joined elements (Array.prototype.toString joins).
fn js_cell_string(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Null => String::new(),
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Array(items) => items
            .iter()
            .map(js_cell_string)
            .collect::<Vec<_>>()
            .join(","),
        serde_json::Value::Object(_) => "[object Object]".into(),
    }
}

/// A sheet's JSON grid (`string[][]`, row 0 = header) → a GFM markdown table.
/// A body that does not parse (or is not an array) passes through verbatim —
/// the sheet may hold unstructured text; a grid with no rows renders empty.
fn sheet_to_markdown_table(body: &str) -> String {
    let parsed: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(_) => return body.to_string(),
    };
    let Some(raw) = parsed.as_array() else {
        return body.to_string();
    };
    let grid: Vec<Vec<String>> = raw
        .iter()
        .map(|r| {
            r.as_array()
                .map(|cells| cells.iter().map(js_cell_string).collect())
                .unwrap_or_default()
        })
        .collect();
    if grid.is_empty() {
        return String::new();
    }
    let esc = |s: &str| s.replace('|', "\\|").replace('\n', " ");
    let head = &grid[0];
    let cols = head.len();
    let line = |cells: &[String]| {
        let rendered: Vec<String> = (0..cols)
            .map(|i| esc(cells.get(i).map(String::as_str).unwrap_or("")))
            .collect();
        format!("| {} |", rendered.join(" | "))
    };
    let mut out = vec![line(head), format!("| {} |", vec!["---"; cols].join(" | "))];
    out.extend(grid.iter().skip(1).map(|row| line(row)));
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn artifact(kind: &str, body: &str) -> Artifact {
        Artifact {
            id: "a-1".into(),
            kind: kind.into(),
            title: "T".into(),
            icon: None,
            body: body.into(),
            content_type: None,
            storage_ref: None,
            visibility: "org".into(),
            edit_policy: "owner".into(),
            public_slug: None,
            official: false,
            kb_doc_id: None,
            folder_id: None,
            owner_user_id: None,
            rag_routing: "auto".into(),
            google_file_id: None,
            google_file_url: None,
            created_by: None,
            updated_by: None,
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    #[test]
    fn sheets_render_as_gfm_tables() {
        let grid = r#"[["Name","Qty"],["a | b","2"],["c\nd",null]]"#;
        assert_eq!(
            sheet_to_markdown_table(grid),
            "| Name | Qty |\n| --- | --- |\n| a \\| b | 2 |\n| c d |  |"
        );
    }

    #[test]
    fn sheet_rendering_matches_the_ts_edge_shapes() {
        // Unparseable body passes through verbatim.
        assert_eq!(sheet_to_markdown_table("not json"), "not json");
        // A JSON non-array passes through too.
        assert_eq!(sheet_to_markdown_table(r#""hello""#), r#""hello""#);
        // Empty grid → empty string.
        assert_eq!(sheet_to_markdown_table("[]"), "");
        // Ragged rows: cells beyond the header WIDTH are dropped, short rows
        // pad with '' — a cell inside the width renders even past its own
        // row's length is impossible here, but "extra" (index 1 < 2) stays.
        assert_eq!(
            sheet_to_markdown_table(r#"[["h1","h2"],["only-one","extra","dropped"]]"#),
            "| h1 | h2 |\n| --- | --- |\n| only-one | extra |"
        );
        // A non-array row is an empty row (TS maps it to []).
        assert_eq!(
            sheet_to_markdown_table(r#"[["h"],["x"],5]"#),
            "| h |\n| --- |\n| x |\n|  |"
        );
        // Cell stringification mirrors JS String(c ?? '').
        let odd = r#"[["v"],[true,1.5,{"a":1},["j","oin"]]]"#;
        // header is 1 wide → only the first cell of the odd row renders.
        assert_eq!(sheet_to_markdown_table(odd), "| v |\n| --- |\n| true |");
    }

    #[test]
    fn artifact_markdown_by_kind() {
        assert_eq!(artifact_to_markdown(&artifact("file", "bytes")), "");
        assert_eq!(artifact_to_markdown(&artifact("doc", "# hi")), "# hi");
        assert_eq!(artifact_to_markdown(&artifact("microsite", "<p>")), "<p>");
        assert_eq!(
            artifact_to_markdown(&artifact("sheet", r#"[["a"],["b"]]"#)),
            "| a |\n| --- |\n| b |"
        );
    }
}
