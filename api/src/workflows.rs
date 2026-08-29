// Task workflows — the hook layer between "an agent got a ticket" and "the
// agent works it the right way". Port of ui/src/server/workflows.ts: the
// CRUD half, and (since the runs engine crossed) the match/ classifiers that
// decide which hook a ticket pulls in. SQL verbatim, only the uuid cast
// added for sqlx. match/skills/toolkits/env are jsonb passed through
// untouched — the DB's canonical key order is the wire order on both
// runtimes.

use sqlx::PgPool;

/// The row as LIST/CREATE serve it — workflows.ts's ROW order:
/// id, name, description, enabled, match, skills, toolkits, env, position.
#[derive(serde::Serialize)]
pub struct Workflow {
    pub id: String,
    pub name: String,
    pub description: String,
    pub enabled: bool,
    pub r#match: serde_json::Value,
    pub skills: serde_json::Value,
    pub toolkits: serde_json::Value,
    pub env: serde_json::Value,
    pub position: i32,
}

type WorkflowRow = (
    String,
    String,
    String,
    bool,
    serde_json::Value,
    serde_json::Value,
    serde_json::Value,
    serde_json::Value,
    i32,
);

impl From<WorkflowRow> for Workflow {
    fn from(r: WorkflowRow) -> Self {
        let (id, name, description, enabled, m, skills, toolkits, env, position) = r;
        Workflow {
            id,
            name,
            description,
            enabled,
            r#match: m,
            skills,
            toolkits,
            env,
            position,
        }
    }
}

const ROW: &str = "id::text, name, description, enabled, match, skills, toolkits, env, position";

pub async fn list_workflows(pg: &PgPool) -> Result<Vec<Workflow>, sqlx::Error> {
    // AssertSqlSafe: the interpolation is this crate's ROW column list.
    let sql = format!("select {ROW} from task_workflows order by position, created_at");
    let rows: Vec<WorkflowRow> = sqlx::query_as(sqlx::AssertSqlSafe(sql.as_str()))
        .fetch_all(pg)
        .await?;
    Ok(rows.into_iter().map(Workflow::from).collect())
}

/// Insert at the end (position = max+1, 0 for the first row); enabled/env
/// come from the table defaults. Returns the row as the CREATE response.
pub async fn create_workflow(
    pg: &PgPool,
    name: &str,
    description: &str,
    match_v: &serde_json::Value,
    skills: &serde_json::Value,
    toolkits: &serde_json::Value,
    created_by: &str,
) -> Result<Workflow, sqlx::Error> {
    // AssertSqlSafe: the interpolation is this crate's ROW column list.
    let sql = format!(
        "insert into task_workflows (name, description, match, skills, toolkits, created_by, position) \
         values ($1, $2, $3, $4, $5, $6, \
                 coalesce((select max(position) + 1 from task_workflows), 0)) \
         returning {ROW}"
    );
    let row: WorkflowRow = sqlx::query_as(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(name)
        .bind(description)
        .bind(match_v)
        .bind(skills)
        .bind(toolkits)
        .bind(created_by)
        .fetch_one(pg)
        .await?;
    Ok(Workflow::from(row))
}

/// The PUT patch — every field Option<"present">, absent fields untouched
/// (TS runs one update per present field, same order, same non-transaction).
pub struct WorkflowPatch {
    pub name: Option<String>,
    pub description: Option<String>,
    pub enabled: Option<bool>,
    pub match_v: Option<serde_json::Value>,
    pub skills: Option<serde_json::Value>,
    pub toolkits: Option<serde_json::Value>,
}

pub async fn update_workflow(
    pg: &PgPool,
    id: &str,
    patch: &WorkflowPatch,
) -> Result<(), sqlx::Error> {
    if let Some(v) = &patch.name {
        sqlx::query("update task_workflows set name = $1, updated_at = now() where id = $2::uuid")
            .bind(v)
            .bind(id)
            .execute(pg)
            .await?;
    }
    if let Some(v) = &patch.description {
        sqlx::query(
            "update task_workflows set description = $1, updated_at = now() where id = $2::uuid",
        )
        .bind(v)
        .bind(id)
        .execute(pg)
        .await?;
    }
    if let Some(v) = patch.enabled {
        sqlx::query(
            "update task_workflows set enabled = $1, updated_at = now() where id = $2::uuid",
        )
        .bind(v)
        .bind(id)
        .execute(pg)
        .await?;
    }
    if let Some(v) = &patch.match_v {
        sqlx::query("update task_workflows set match = $1, updated_at = now() where id = $2::uuid")
            .bind(v)
            .bind(id)
            .execute(pg)
            .await?;
    }
    if let Some(v) = &patch.skills {
        sqlx::query(
            "update task_workflows set skills = $1, updated_at = now() where id = $2::uuid",
        )
        .bind(v)
        .bind(id)
        .execute(pg)
        .await?;
    }
    if let Some(v) = &patch.toolkits {
        sqlx::query(
            "update task_workflows set toolkits = $1, updated_at = now() where id = $2::uuid",
        )
        .bind(v)
        .bind(id)
        .execute(pg)
        .await?;
    }
    Ok(())
}

/// No 404 — a missed id deletes nothing and the route still answers ok.
pub async fn delete_workflow(pg: &PgPool, id: &str) -> Result<(), sqlx::Error> {
    sqlx::query("delete from task_workflows where id = $1::uuid")
        .bind(id)
        .execute(pg)
        .await?;
    Ok(())
}

// ── The match half ───────────────────────────────────────────────────────────

/// What a match is decided against — `Pick<Task, 'title' | 'description' |
/// 'tags' | 'boardId'>`, borrowed: the heartbeat holds the tickets it just
/// fetched and should not have to clone them to classify them.
pub struct MatchTarget<'a> {
    pub title: &'a str,
    pub description: Option<&'a str>,
    pub tags: &'a [String],
    pub board_id: &'a str,
}

/// The workflow payload delivered WITH the work (dispatch + heartbeat +
/// get_ticket). `skills`/`toolkits` ride as the row's jsonb, passthrough —
/// same reason as the CRUD half.
#[derive(serde::Serialize)]
pub struct WorkflowDelivery {
    pub name: String,
    pub skills: serde_json::Value,
    pub toolkits: serde_json::Value,
}

/// workflows.ts matchWorkflow — does this hook pull in on this ticket?
///
/// A hook matches when EVERY facet it declares is satisfied, and a hook that
/// declares nothing matches NOTHING (a bare `every` on zero facets would
/// match everything, and an empty-rules workflow would silently become the
/// org-wide default hook). Non-string entries in a facet simply never equal
/// a tag/board id — TS's `Array.includes` behaves the same way.
pub fn match_workflow(h: &Workflow, t: &MatchTarget<'_>) -> bool {
    if !h.enabled {
        return false;
    }
    let m = &h.r#match;
    let mut facets: Vec<bool> = Vec::new();
    let labels = m.get("labels").and_then(|v| v.as_array());
    if let Some(labels) = labels.filter(|l| !l.is_empty()) {
        facets.push(
            labels
                .iter()
                .filter_map(|l| l.as_str())
                .any(|l| t.tags.iter().any(|tag| tag == l)),
        );
    }
    let boards = m.get("boards").and_then(|v| v.as_array());
    if let Some(boards) = boards.filter(|b| !b.is_empty()) {
        facets.push(
            boards
                .iter()
                .filter_map(|b| b.as_str())
                .any(|b| b == t.board_id),
        );
    }
    let keywords = m.get("keywords").and_then(|v| v.as_array());
    if let Some(keywords) = keywords.filter(|k| !k.is_empty()) {
        // Title and description are one haystack, newline-joined — a keyword
        // spanning the seam matches, as in TS. Case-folded both sides.
        let hay = format!("{}\n{}", t.title, t.description.unwrap_or("")).to_lowercase();
        facets.push(
            keywords
                .iter()
                .filter_map(|k| k.as_str())
                .any(|k| hay.contains(&k.to_lowercase())),
        );
    }
    !facets.is_empty() && facets.iter().all(|f| *f)
}

/// workflows.ts workflowsFrom — the match against a list the caller already
/// holds. Batch callers (the heartbeat walks every servable ticket) MUST use
/// this with one `list_workflows()` hoisted out of their loop — calling
/// `workflows_for_task` per ticket re-read the whole table each time. Kept
/// as the single expression of the match so the hot path cannot drift from
/// the one-off path.
pub fn workflows_from(all: &[Workflow], t: &MatchTarget<'_>) -> Vec<WorkflowDelivery> {
    all.iter()
        .filter(|h| match_workflow(h, t))
        .map(|h| WorkflowDelivery {
            name: h.name.clone(),
            skills: h.skills.clone(),
            toolkits: h.toolkits.clone(),
        })
        .collect()
}

/// The workflow payload for one ticket — the one-off path (dispatch,
/// get_ticket). Reads the table itself; batch callers use `workflows_from`.
pub async fn workflows_for_task(
    pg: &PgPool,
    t: &MatchTarget<'_>,
) -> Result<Vec<WorkflowDelivery>, sqlx::Error> {
    let all = list_workflows(pg).await?;
    Ok(workflows_from(&all, t))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn hook(enabled: bool, m: serde_json::Value) -> Workflow {
        Workflow {
            id: "w-1".into(),
            name: "Support triage".into(),
            description: String::new(),
            enabled,
            r#match: m,
            skills: json!(["talaria-support"]),
            toolkits: json!([{ "server": "github", "tools": ["issues"] }]),
            env: json!({}),
            position: 0,
        }
    }

    fn target<'a>(
        title: &'a str,
        description: Option<&'a str>,
        tags: &'a [String],
        board_id: &'a str,
    ) -> MatchTarget<'a> {
        MatchTarget {
            title,
            description,
            tags,
            board_id,
        }
    }

    fn strs(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn disabled_hooks_and_ruleless_hooks_match_nothing() {
        let tags = strs(&["support"]);
        let t = target("Crash on export", None, &tags, "b-1");
        // Disabled, with rules that would match: still nothing.
        assert!(!match_workflow(
            &hook(false, json!({"labels": ["support"]})),
            &t
        ));
        // Enabled but no rules: nothing — a ruleless hook must not become
        // the org-wide default via a vacuous `every`.
        assert!(!match_workflow(&hook(true, json!({})), &t));
        assert!(!match_workflow(&hook(true, json!(null)), &t));
        // Empty facet arrays are the same as absent ones.
        assert!(!match_workflow(&hook(true, json!({"labels": []})), &t));
    }

    #[test]
    fn every_declared_facet_must_hold() {
        let tags = strs(&["support"]);
        let t = target("Crash on export", None, &tags, "b-1");
        // Labels AND boards both hold.
        assert!(match_workflow(
            &hook(
                true,
                json!({"labels": ["billing", "support"], "boards": ["b-1"]})
            ),
            &t
        ));
        // Labels hold, boards don't → no match. This is the AND, not the OR.
        assert!(!match_workflow(
            &hook(true, json!({"labels": ["support"], "boards": ["b-2"]})),
            &t
        ));
    }

    #[test]
    fn keywords_span_the_title_description_seam_case_folded() {
        let tags: Vec<String> = Vec::new();
        let t = target("Export", Some("crashes on LARGE inputs"), &tags, "b-1");
        // Substring, case-folded on both sides, across the newline join.
        assert!(match_workflow(
            &hook(true, json!({"keywords": ["export\nCrashes"]})),
            &t
        ));
        assert!(match_workflow(
            &hook(true, json!({"keywords": ["large"]})),
            &t
        ));
        // Any one keyword matching is enough.
        assert!(match_workflow(
            &hook(true, json!({"keywords": ["nothing", "inputs"]})),
            &t
        ));
        // But it is still a facet: keyword holds while a declared label
        // facet fails → no match.
        assert!(!match_workflow(
            &hook(true, json!({"keywords": ["export"], "labels": ["support"]})),
            &t
        ));
        // A missing description is the empty string in the haystack.
        let t2 = target("Just a title", None, &tags, "b-1");
        assert!(match_workflow(
            &hook(true, json!({"keywords": ["\n"]})),
            &t2
        ));
    }

    #[test]
    fn non_string_facet_entries_never_equal_anything() {
        let tags = strs(&["support"]);
        let t = target("Crash", None, &tags, "b-1");
        // TS's Array.includes on a string array with a non-string needle is
        // false; so is this.
        assert!(!match_workflow(
            &hook(true, json!({"labels": [42, true]})),
            &t
        ));
        assert!(!match_workflow(
            &hook(true, json!({"boards": [{"id": "b-1"}]})),
            &t
        ));
    }

    #[test]
    fn workflows_from_filters_and_delivers_name_skills_toolkits() {
        let tags = strs(&["support"]);
        let t = target("Crash on export", None, &tags, "b-1");
        let all = vec![
            hook(true, json!({"labels": ["support"]})),
            hook(true, json!({"labels": ["billing"]})),
            hook(false, json!({"labels": ["support"]})),
        ];
        let delivered = workflows_from(&all, &t);
        assert_eq!(delivered.len(), 1);
        assert_eq!(delivered[0].name, "Support triage");
        assert_eq!(delivered[0].skills, json!(["talaria-support"]));
        assert_eq!(
            delivered[0].toolkits,
            json!([{ "server": "github", "tools": ["issues"] }])
        );
    }
}
