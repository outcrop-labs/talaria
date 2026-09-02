// Artifacts — versioned work products (doc / sheet / microsite / file) with
// their own hosting, sharing, and version history. Sharing reuses the KB
// permission model (kb_editors with item_type='artifact' + visibility / edit
// policy); versioning reuses internal_versions (kind 'artifact'). Flat, so no
// folder inheritance — an artifact carries its own audience.
//
// The whole plane: rows, folders, links,
// the official→KB mirror, public slugs, and the plan-doc activity index.

use sqlx::PgPool;

use crate::agent_auth::epoch_ms_to_iso;
use crate::kb::perms::Guarded;

/// An artifact — the full row shape.
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

/// The same columns for queries that join: the table
/// needs its alias, the row shape does not change.
const COLS_A: &str = "a.id::text, a.kind, a.title, a.icon, a.body, a.content_type, a.storage_ref, \
                      a.visibility, a.edit_policy, a.public_slug, a.official, a.kb_doc_id::text, \
                      a.folder_id::text, a.owner_user_id::text, a.rag_routing, a.google_file_id, \
                      a.google_file_url, a.created_by::text, a.updated_by::text, \
                      (trunc(extract(epoch from a.created_at) * 1000))::bigint as created_ms, \
                      (trunc(extract(epoch from a.updated_at) * 1000))::bigint as updated_ms";

/// Every artifact linked to a target — the
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
         where l.target_type = $1 and l.target_id = $2 \
         order by l.created_at desc"
    );
    let rows: Vec<ArtifactRow> = sqlx::query_as(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(target_type)
        .bind(target_id)
        .fetch_all(pg)
        .await?;
    Ok(rows.into_iter().map(Artifact::from).collect())
}

/// The plan's linked document: the first `doc`
/// among the plan's linked artifacts. The document IS that artifact — there
/// is no separate model — and the draft reads it as seed context: the
/// plan-draft transcript wants the document's current body beside the
/// conversation.
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

/// Every artifact, newest first — the raw list;
/// every caller filters to what its viewer can read.
pub async fn list_artifacts(pg: &PgPool) -> Result<Vec<Artifact>, sqlx::Error> {
    // AssertSqlSafe: the interpolation is this crate's COLS column list.
    let sql = format!("select {COLS} from artifacts order by updated_at desc");
    let rows: Vec<ArtifactRow> = sqlx::query_as(sqlx::AssertSqlSafe(sql.as_str()))
        .fetch_all(pg)
        .await?;
    Ok(rows.into_iter().map(Artifact::from).collect())
}

/// The public-artifact read behind /api/artifacts/public/{slug} — only
/// artifacts set to 'public' resolve, by slug.
pub async fn get_public_artifact(pg: &PgPool, slug: &str) -> Result<Option<Artifact>, sqlx::Error> {
    // AssertSqlSafe: the interpolation is this crate's COLS column list.
    let sql =
        format!("select {COLS} from artifacts where public_slug = $1 and visibility = 'public'");
    let row: Option<ArtifactRow> = sqlx::query_as(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(slug)
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

// ── The write half ────────────────────────────────────────────────────────────

/// Create — kind/title carry their defaults ('doc', 'Untitled').
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

// ── Folders (organize artifacts into a nestable tree) ──────────────────────

/// An artifact folder — wire order.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactFolder {
    pub id: String,
    pub name: String,
    pub icon: Option<String>,
    pub parent_id: Option<String>,
    pub visibility: String,
    pub edit_policy: String,
    pub owner_user_id: Option<String>,
    pub created_by: Option<String>,
    pub created_at: String,
}

const FOLDER_COLS: &str = "id::text, name, icon, parent_id::text, visibility, edit_policy, \
                           owner_user_id::text, created_by::text, \
                           (trunc(extract(epoch from created_at) * 1000))::bigint as created_ms";

#[derive(sqlx::FromRow)]
struct FolderRow {
    id: String,
    name: String,
    icon: Option<String>,
    parent_id: Option<String>,
    visibility: String,
    edit_policy: String,
    owner_user_id: Option<String>,
    created_by: Option<String>,
    created_ms: i64,
}

impl From<FolderRow> for ArtifactFolder {
    fn from(r: FolderRow) -> Self {
        ArtifactFolder {
            created_at: epoch_ms_to_iso(r.created_ms),
            id: r.id,
            name: r.name,
            icon: r.icon,
            parent_id: r.parent_id,
            visibility: r.visibility,
            edit_policy: r.edit_policy,
            owner_user_id: r.owner_user_id,
            created_by: r.created_by,
        }
    }
}

/// A folder in the shape the permission checks want (guardedFolder). Folders
/// carry the same three access columns as docs, spaces and artifacts, so the
/// same functions answer "can this person read it / re-share it" for all four.
pub fn guarded_folder(f: &ArtifactFolder) -> Guarded {
    Guarded {
        owner_user_id: f.owner_user_id.clone(),
        created_by: f.created_by.clone(),
        visibility: f.visibility.clone(),
        edit_policy: f.edit_policy.clone(),
    }
}

pub async fn list_folders(pg: &PgPool) -> Result<Vec<ArtifactFolder>, sqlx::Error> {
    // AssertSqlSafe: the interpolation is this crate's FOLDER_COLS column list.
    let sql = format!("select {FOLDER_COLS} from artifact_folders order by name asc");
    let rows: Vec<FolderRow> = sqlx::query_as(sqlx::AssertSqlSafe(sql.as_str()))
        .fetch_all(pg)
        .await?;
    Ok(rows.into_iter().map(ArtifactFolder::from).collect())
}

pub async fn get_folder(pg: &PgPool, id: &str) -> Result<Option<ArtifactFolder>, sqlx::Error> {
    // AssertSqlSafe: the interpolation is this crate's FOLDER_COLS column list.
    let sql = format!("select {FOLDER_COLS} from artifact_folders where id = $1::uuid");
    let row: Option<FolderRow> = sqlx::query_as(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(id)
        .fetch_optional(pg)
        .await?;
    Ok(row.map(ArtifactFolder::from))
}

/// Create. `owner_user_id` is what separates a person's folder from the
/// workspace's: a human making a folder owns it; the find-or-create path
/// agents use passes nothing, so agent cabinets stay ownerless and org-visible
/// — the workspace's, which is exactly what they are.
pub async fn create_folder(
    pg: &PgPool,
    name: &str,
    parent_id: Option<&str>,
    created_by: &str,
    owner_user_id: Option<&str>,
    visibility: Option<&str>,
) -> Result<ArtifactFolder, sqlx::Error> {
    // AssertSqlSafe: the interpolation is this crate's FOLDER_COLS column list.
    let sql = format!(
        "insert into artifact_folders (name, parent_id, created_by, owner_user_id, visibility) \
         values ($1, $2::uuid, $3, $4::uuid, $5) returning {FOLDER_COLS}"
    );
    let row: FolderRow = sqlx::query_as(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(name)
        .bind(parent_id)
        .bind(created_by)
        .bind(owner_user_id)
        .bind(visibility.unwrap_or("org"))
        .fetch_one(pg)
        .await?;
    Ok(ArtifactFolder::from(row))
}

/// Rename / set icon / reparent / re-share a folder. Rejects
/// parent cycles with None — the route answers that as `invalid` at 400.
pub async fn update_folder(
    pg: &PgPool,
    id: &str,
    name: Option<&str>,
    icon: Option<Option<&str>>,
    parent_id: Option<Option<&str>>,
    visibility: Option<&str>,
    edit_policy: Option<&str>,
) -> Result<Option<ArtifactFolder>, sqlx::Error> {
    // The cycle walk runs BEFORE any write when the patch reparents: up the
    // ancestor chain from the new parent, 100 hops max (deeper than any real
    // tree; the cap keeps a forged cycle from hanging the request).
    if let Some(Some(parent)) = parent_id {
        if parent == id {
            return Ok(None);
        }
        let mut cur = Some(parent.to_string());
        for _ in 0..100 {
            let Some(node) = cur else { break };
            if node == id {
                return Ok(None); // cycle
            }
            let row: Option<(Option<String>,)> =
                sqlx::query_as("select parent_id::text from artifact_folders where id = $1::uuid")
                    .bind(&node)
                    .fetch_optional(pg)
                    .await?;
            cur = row.and_then(|(v,)| v);
        }
    }
    if let Some(name) = name {
        sqlx::query("update artifact_folders set name = $2 where id = $1::uuid")
            .bind(id)
            .bind(name)
            .execute(pg)
            .await?;
    }
    if let Some(icon) = icon {
        sqlx::query("update artifact_folders set icon = $2 where id = $1::uuid")
            .bind(id)
            .bind(icon)
            .execute(pg)
            .await?;
    }
    if let Some(parent) = parent_id {
        sqlx::query("update artifact_folders set parent_id = $2::uuid where id = $1::uuid")
            .bind(id)
            .bind(parent)
            .execute(pg)
            .await?;
    }
    if let Some(visibility) = visibility {
        sqlx::query("update artifact_folders set visibility = $2 where id = $1::uuid")
            .bind(id)
            .bind(visibility)
            .execute(pg)
            .await?;
    }
    if let Some(edit_policy) = edit_policy {
        sqlx::query("update artifact_folders set edit_policy = $2 where id = $1::uuid")
            .bind(id)
            .bind(edit_policy)
            .execute(pg)
            .await?;
    }
    get_folder(pg, id).await
}

/// Delete a folder — its artifacts and child folders fall back to the root
/// (on delete set null), so nothing is lost.
pub async fn delete_folder(pg: &PgPool, id: &str) -> Result<(), sqlx::Error> {
    sqlx::query("delete from artifact_folders where id = $1::uuid")
        .bind(id)
        .execute(pg)
        .await?;
    Ok(())
}

/// The ownerless org-visible create shape — the one the agent cabinet path
/// uses (the workspace's).
async fn create_org_folder(
    pg: &PgPool,
    name: &str,
    parent_id: Option<&str>,
    created_by: &str,
) -> Result<String, sqlx::Error> {
    Ok(
        create_folder(pg, name, parent_id, created_by, None, Some("org"))
            .await?
            .id,
    )
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
    create_org_folder(pg, name, parent_id, created_by).await
}

/// Find-or-create a folder by NAME at the root — the "file this under X"
/// spelling agents use. Never throws (None = root).
pub async fn named_root_folder(pg: &PgPool, name: &str, created_by: &str) -> Option<String> {
    find_or_create_folder(pg, name, None, created_by).await.ok()
}

/// The single root every agent cabinet hangs under. One folder per agent at the
/// ROOT buried the user's own work under a wall of agent names the moment the
/// fleet grew; the Files browser now opens on your folders, with the whole
/// fleet's output one click away.
pub const AGENTS_ROOT: &str = "Agents";

/// The category folders agent_category_folder writes.
pub const AGENT_CATEGORIES: [&str; 6] = [
    "Documents",
    "Media",
    "Chat summaries",
    "Plans",
    "Research",
    "Briefs",
];

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

/// Link an artifact to anything. `on conflict do nothing` is
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
         values ($1::uuid, $2, $3, $4) on conflict do nothing",
    )
    .bind(artifact_id)
    .bind(target_type)
    .bind(target_id)
    .bind(actor)
    .execute(pg)
    .await?;
    Ok(())
}

/// Detach — remove one link.
pub async fn detach_artifact(
    pg: &PgPool,
    artifact_id: &str,
    target_type: &str,
    target_id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "delete from artifact_links \
         where artifact_id = $1::uuid and target_type = $2 and target_id = $3",
    )
    .bind(artifact_id)
    .bind(target_type)
    .bind(target_id)
    .execute(pg)
    .await?;
    Ok(())
}

/// Set an artifact's RAG routing ('auto' | 'none' | a custom brain id).
/// The caller applies the re-placement
/// (retrieval::artifact_routing) — split to keep this module free of
/// retrieval imports. A garbage brain id fails the uuid cast and answers with
/// the PG sentence; the route maps it to 400.
pub async fn set_artifact_routing(
    pg: &PgPool,
    id: &str,
    routing: &str,
    actor: &str,
) -> Result<Option<Artifact>, String> {
    if routing != "auto" && routing != "none" {
        let ok: Option<(i32,)> =
            sqlx::query_as("select 1 from rag_collections where id = $1::uuid and kind = 'custom'")
                .bind(routing)
                .fetch_optional(pg)
                .await
                .map_err(|e| crate::error::pg_message(&e))?;
        if ok.is_none() {
            return Err("unknown brain".into());
        }
    }
    sqlx::query(
        "update artifacts set rag_routing = $2, updated_by = $3::text, updated_at = now() \
         where id = $1::uuid",
    )
    .bind(id)
    .bind(routing)
    .bind(actor)
    .execute(pg)
    .await
    .map_err(|e| crate::error::pg_message(&e))?;
    get_artifact(pg, id)
        .await
        .map_err(|e| crate::error::pg_message(&e))
}

/// The things an artifact is attached to (its "Attached to" list) —
/// (targetType, targetId) pairs in link order.
pub async fn targets_for_artifact(
    pg: &PgPool,
    artifact_id: &str,
) -> Result<Vec<(String, String)>, sqlx::Error> {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "select target_type, target_id::text from artifact_links where artifact_id = $1::uuid",
    )
    .bind(artifact_id)
    .fetch_all(pg)
    .await?;
    Ok(rows)
}

// ── Promotion to the knowledgebase ──────────────────────────────────────────
// "Making an artifact official" mirrors it into the KB (as an official doc in
// a system "Artifacts" space) so it grounds the org brain. The link is kept
// in artifacts.kb_doc_id; un-officializing removes the mirror.

const ARTIFACTS_SPACE: &str = "Artifacts";

async fn ensure_artifacts_space(pg: &PgPool, actor: &str) -> Result<String, sqlx::Error> {
    if let Some(existing) = crate::kb::list_spaces(pg)
        .await?
        .into_iter()
        .find(|s| s.name == ARTIFACTS_SPACE)
    {
        return Ok(existing.id);
    }
    let space = crate::kb::create_space(
        pg,
        &crate::kb::NewSpace {
            name: ARTIFACTS_SPACE.into(),
            icon: Some("◆".into()),
            description: Some("Official artifacts, mirrored into the knowledgebase.".into()),
            created_by: actor.to_string(),
            owner_user_id: None,
        },
    )
    .await?;
    Ok(space.id)
}

/// official=true mirrors (or re-mirrors) the artifact
/// into the Artifacts space as an official KB doc; false removes the mirror.
pub async fn set_artifact_official(
    pg: &PgPool,
    qd: &crate::retrieval::qdrant::QdrantDeps,
    ed: &crate::retrieval::embed::EmbedDeps,
    id: &str,
    official: bool,
    actor: &str,
) -> Result<Option<Artifact>, sqlx::Error> {
    let Some(a) = get_artifact(pg, id).await? else {
        return Ok(None);
    };
    if official {
        let space_id = ensure_artifacts_space(pg, actor).await?;
        let md = artifact_to_markdown(&a);
        // Reuse the recorded mirror while it still exists.
        let kb_doc_id = match &a.kb_doc_id {
            Some(kb) if matches!(crate::kb::get_doc(pg, kb).await, Ok(Some(_))) => kb.clone(),
            _ => {
                crate::kb::create_doc(
                    pg,
                    &crate::kb::NewDoc {
                        space_id,
                        parent_id: None,
                        title: Some(a.title.clone()),
                        kind: Some("human".into()),
                        created_by: actor.to_string(),
                        owner_user_id: a.owner_user_id.clone(),
                    },
                )
                .await?
                .id
            }
        };
        crate::kb::save_doc(
            pg,
            qd,
            ed,
            &kb_doc_id,
            &crate::kb::DocPatch {
                title: Some(a.title.clone()),
                body: Some(md),
                ..Default::default()
            },
            actor,
        )
        .await?;
        crate::kb::set_official(pg, qd, ed, &kb_doc_id, true, actor).await?; // → org brain
        sqlx::query(
            "update artifacts set official = true, kb_doc_id = $2::uuid, updated_at = now() \
             where id = $1::uuid",
        )
        .bind(id)
        .bind(&kb_doc_id)
        .execute(pg)
        .await?;
    } else {
        if let Some(kb) = &a.kb_doc_id {
            let _ = crate::kb::delete_doc(pg, qd, ed, kb).await;
        }
        sqlx::query(
            "update artifacts set official = false, kb_doc_id = null, updated_at = now() \
             where id = $1::uuid",
        )
        .bind(id)
        .execute(pg)
        .await?;
    }
    get_artifact(pg, id).await
}

/// Record where an artifact was mirrored into Google Drive (last export wins).
pub async fn record_google_export(
    pg: &PgPool,
    id: &str,
    file_id: &str,
    url: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "update artifacts set google_file_id = $2, google_file_url = $3, updated_at = now() \
         where id = $1::uuid",
    )
    .bind(id)
    .bind(file_id)
    .bind(url)
    .execute(pg)
    .await?;
    Ok(())
}

/// Keep the KB mirror fresh when an already-official artifact's content
/// changes (remirrorIfOfficial). Errors are the caller's .catch — swallowed.
async fn remirror_if_official(pg: &PgPool, a: &Artifact, actor: &str) -> Result<(), sqlx::Error> {
    if a.official
        && let Some(kb_doc_id) = &a.kb_doc_id
    {
        let _ = crate::kb::save_doc(
            pg,
            &crate::retrieval::qdrant::real_deps(),
            &crate::retrieval::embed::real_deps(),
            kb_doc_id,
            &crate::kb::DocPatch {
                title: Some(a.title.clone()),
                body: Some(artifact_to_markdown(a)),
                ..Default::default()
            },
            actor,
        )
        .await;
    }
    Ok(())
}

/// The artifact's retrievable copy, as the
/// plan-doc activity flow: auto-routing only, payload keyed to the plan so
/// the activity index's ACL can resolve reach.
pub async fn index_plan_doc(
    pg: &PgPool,
    qd: &crate::retrieval::qdrant::QdrantDeps,
    ed: &crate::retrieval::embed::EmbedDeps,
    doc: &Artifact,
    conversation_id: &str,
) -> Result<(), String> {
    if !doc.rag_routing.is_empty() && doc.rag_routing != "auto" {
        return Ok(());
    }
    let mut payload = serde_json::Map::new();
    payload.insert(
        "planId".into(),
        serde_json::Value::String(conversation_id.to_string()),
    );
    payload.insert(
        "planOwnerId".into(),
        doc.owner_user_id
            .clone()
            .map(serde_json::Value::String)
            .unwrap_or(serde_json::Value::Null),
    );
    let _ = crate::retrieval::sources::index_activity(
        pg,
        qd,
        ed,
        &crate::retrieval::index::IndexDoc {
            source_type: "plan-doc".into(),
            source_id: doc.id.clone(),
            title: Some(doc.title.clone()),
            text: format!("{}\n\n{}", doc.title, doc.body),
            payload: Some(payload),
            // Deep-linked: a retrieval hit is a pointer to ONE document, and
            // the id is right here.
            href: Some(format!("/artifacts?a={}", doc.id)),
        },
    )
    .await;
    Ok(())
}

/// The full save patch. The double Option is the patch's tri-state:
/// the outer layer says whether the field was in the patch at all, the inner
/// whether it is null.
#[derive(Default)]
pub struct SaveArtifactPatch<'a> {
    pub title: Option<Option<&'a str>>,
    pub body: Option<&'a str>,
    pub icon: Option<Option<&'a str>>,
    pub storage_ref: Option<Option<&'a str>>,
    pub content_type: Option<Option<&'a str>>,
    pub folder_id: Option<Option<&'a str>>,
    /// `'private' | 'org' | 'public'` — publishing mints the public slug.
    pub visibility: Option<&'a str>,
    /// `'owner' | 'org' | 'restricted'`.
    pub edit_policy: Option<&'a str>,
}

/// Save — every patch leg in a fixed write order (folder, title,
/// body, icon, storage, contentType, editPolicy, visibility), then the re-read,
/// the version snapshot on a content change, and the official→KB re-mirror.
pub async fn save_artifact(
    pg: &PgPool,
    id: &str,
    patch: SaveArtifactPatch<'_>,
    actor: &str,
) -> Result<Option<Artifact>, sqlx::Error> {
    let Some(prev) = get_artifact(pg, id).await? else {
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
    if let Some(icon) = patch.icon {
        sqlx::query("update artifacts set icon = $2, updated_at = now() where id = $1::uuid")
            .bind(id)
            .bind(icon)
            .execute(pg)
            .await?;
    }
    if let Some(storage_ref) = patch.storage_ref {
        sqlx::query("update artifacts set storage_ref = $2, updated_by = $3::text, updated_at = now() where id = $1::uuid")
            .bind(id)
            .bind(storage_ref)
            .bind(actor)
            .execute(pg)
            .await?;
    }
    if let Some(content_type) = patch.content_type {
        sqlx::query(
            "update artifacts set content_type = $2, updated_at = now() where id = $1::uuid",
        )
        .bind(id)
        .bind(content_type)
        .execute(pg)
        .await?;
    }
    if let Some(edit_policy) = patch.edit_policy {
        sqlx::query(
            "update artifacts set edit_policy = $2, updated_at = now() where id = $1::uuid",
        )
        .bind(id)
        .bind(edit_policy)
        .execute(pg)
        .await?;
    }
    if let Some(visibility) = patch.visibility {
        sqlx::query("update artifacts set visibility = $2, updated_at = now() where id = $1::uuid")
            .bind(id)
            .bind(visibility)
            .execute(pg)
            .await?;
        // First publish mints the share slug; the `is null` guard makes a
        // concurrent publish keep one slug, not flip it mid-share.
        if visibility == "public" && prev.public_slug.is_none() {
            sqlx::query(
                "update artifacts set public_slug = $2 \
                 where id = $1::uuid and public_slug is null",
            )
            .bind(id)
            .bind(crate::kb::random_slug())
            .execute(pg)
            .await?;
        }
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
        // Keep the KB mirror fresh — best-effort.
        let _ = remirror_if_official(pg, a, actor).await;
    }
    Ok(next)
}

/// Delete — the KB mirror dies with the artifact (its RAG copy goes
/// via delete_doc's unindex), then the row.
pub async fn delete_artifact(
    pg: &PgPool,
    qd: &crate::retrieval::qdrant::QdrantDeps,
    ed: &crate::retrieval::embed::EmbedDeps,
    id: &str,
) -> Result<(), sqlx::Error> {
    if let Ok(Some(a)) = get_artifact(pg, id).await
        && let Some(kb_doc_id) = &a.kb_doc_id
    {
        let _ = crate::kb::delete_doc(pg, qd, ed, kb_doc_id).await;
    }
    sqlx::query("delete from artifacts where id = $1::uuid")
        .bind(id)
        .execute(pg)
        .await?;
    Ok(())
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
        // A non-array row is an empty row.
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
