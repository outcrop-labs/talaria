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
