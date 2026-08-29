// Task workflows — the hook layer between "an agent got a ticket" and "the
// agent works it the right way". Port of ui/src/server/workflows.ts's CRUD
// half (the match/ classifiers stay TS-side until the runs engine crosses);
// SQL verbatim, only the uuid cast added for sqlx. match/skills/toolkits/env
// are jsonb passed through untouched — the DB's canonical key order is the
// wire order on both runtimes.

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
