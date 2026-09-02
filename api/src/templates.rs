// Templates — the org-wide library of markdown skeletons plus the bindings
// that pick one. The resolution chain (the order everywhere a template
// applies: explicit pick → agent binding → board default → none) serves
// every consumer — create_task seeds an empty description through it.

use sqlx::PgPool;

use crate::agent_auth::epoch_ms_to_iso;

/// A board's template binding: which ticket template the board uses, and
/// whether it is the default.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BoardTemplateBinding {
    pub template_id: String,
    pub is_default: bool,
}

/// The board's bindings — no ORDER BY: the client treats the set as a set.
pub async fn board_templates(
    pg: &PgPool,
    board_id: &str,
) -> Result<Vec<BoardTemplateBinding>, sqlx::Error> {
    let rows = sqlx::query_as::<_, (String, bool)>(
        "select template_id::text, is_default from board_templates where board_id = $1::uuid",
    )
    .bind(board_id)
    .fetch_all(pg)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(template_id, is_default)| BoardTemplateBinding {
            template_id,
            is_default,
        })
        .collect())
}

/// Replace a board's template set. One transaction: delete, then re-insert
/// each id with is_default set by identity with `default_id`. Duplicates in
/// the list are swallowed by the conflict clause.
pub async fn set_board_templates(
    pg: &PgPool,
    board_id: &str,
    template_ids: &[String],
    default_id: Option<&str>,
) -> Result<(), sqlx::Error> {
    let mut tx = pg.begin().await?;
    sqlx::query("delete from board_templates where board_id = $1::uuid")
        .bind(board_id)
        .execute(&mut *tx)
        .await?;
    for id in template_ids {
        sqlx::query(
            "insert into board_templates (board_id, template_id, is_default) \
             values ($1::uuid, $2::uuid, $3) on conflict do nothing",
        )
        .bind(board_id)
        .bind(id)
        .bind(default_id == Some(id.as_str()))
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await
}

// ── The library row + resolution chain ───────────────────────────────────────

/// A template row — the org-wide library of markdown skeletons + prompt
/// guidance. `body` is the skeleton the filled document
/// must keep; `guidance` is for the agent filling it, never shown on the
/// ticket.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Template {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub body: String,
    pub guidance: String,
    pub created_by: Option<String>,
    pub updated_at: String,
}

const COLS: &str = "id::text, name, kind, body, guidance, created_by::text, \
                    (trunc(extract(epoch from updated_at) * 1000))::bigint as updated_ms";

type TemplateRow = (String, String, String, String, String, Option<String>, i64);

impl From<TemplateRow> for Template {
    fn from(r: TemplateRow) -> Self {
        let (id, name, kind, body, guidance, created_by, updated_ms) = r;
        Template {
            id,
            name,
            kind,
            body,
            guidance,
            created_by,
            updated_at: epoch_ms_to_iso(updated_ms),
        }
    }
}

/// One row by id, any kind — callers that care check `kind` themselves,
/// which is exactly how the resolution chain treats a dead or wrong-kind
/// reference: fall through to the next link.
pub async fn get_template(pg: &PgPool, id: &str) -> Result<Option<Template>, sqlx::Error> {
    // AssertSqlSafe: the interpolation is this crate's COLS column list.
    let sql = format!("select {COLS} from templates where id = $1::uuid");
    let row: Option<TemplateRow> = sqlx::query_as(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(id)
        .fetch_optional(pg)
        .await?;
    Ok(row.map(Template::from))
}

/// The whole library, kind-grouped (the library page's tab order), or one
/// kind's list.
pub async fn list_templates(pg: &PgPool, kind: Option<&str>) -> Result<Vec<Template>, sqlx::Error> {
    let sql = match kind {
        Some(_) => format!("select {COLS} from templates where kind = $1 order by name asc"),
        None => format!("select {COLS} from templates order by kind asc, name asc"),
    };
    let mut q = sqlx::query_as::<_, TemplateRow>(sqlx::AssertSqlSafe(sql.as_str()));
    if let Some(k) = kind {
        q = q.bind(k);
    }
    let rows = q.fetch_all(pg).await?;
    Ok(rows.into_iter().map(Template::from).collect())
}

/// Body/guidance default to empty — a skeleton can start as a name and get
/// its body in the editor.
pub async fn create_template(pg: &PgPool, t: NewTemplate<'_>) -> Result<Template, sqlx::Error> {
    let sql = format!(
        "insert into templates (name, kind, body, guidance, created_by) \
         values ($1, $2, $3, $4, $5) returning {COLS}"
    );
    let row: TemplateRow = sqlx::query_as(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(t.name)
        .bind(t.kind)
        .bind(t.body.unwrap_or(""))
        .bind(t.guidance.unwrap_or(""))
        .bind(t.created_by)
        .fetch_one(pg)
        .await?;
    Ok(Template::from(row))
}

pub struct NewTemplate<'a> {
    pub name: &'a str,
    pub kind: &'a str,
    pub body: Option<&'a str>,
    pub guidance: Option<&'a str>,
    pub created_by: &'a str,
}

/// The each-if-defined patch: only the fields the request carried move, each
/// restamps `updated_at`, and a BODY edit is versioned (the template editor
/// shows the history). Snapshot failures are swallowed: the edit stands, the
/// history row is the nice-to-have.
pub async fn update_template(
    pg: &PgPool,
    id: &str,
    patch: TemplatePatch<'_>,
) -> Result<Option<Template>, sqlx::Error> {
    if let Some(name) = patch.name {
        sqlx::query("update templates set name = $1, updated_at = now() where id = $2::uuid")
            .bind(name)
            .bind(id)
            .execute(pg)
            .await?;
    }
    if let Some(body) = patch.body {
        sqlx::query("update templates set body = $1, updated_at = now() where id = $2::uuid")
            .bind(body)
            .bind(id)
            .execute(pg)
            .await?;
        let _ = crate::internal_history::snapshot(pg, "template", id, body, patch.author).await;
    }
    if let Some(guidance) = patch.guidance {
        sqlx::query("update templates set guidance = $1, updated_at = now() where id = $2::uuid")
            .bind(guidance)
            .bind(id)
            .execute(pg)
            .await?;
    }
    get_template(pg, id).await
}

pub struct TemplatePatch<'a> {
    pub name: Option<&'a str>,
    pub body: Option<&'a str>,
    pub guidance: Option<&'a str>,
    pub author: Option<&'a str>,
}

pub async fn delete_template(pg: &PgPool, id: &str) -> Result<(), sqlx::Error> {
    sqlx::query("delete from templates where id = $1::uuid")
        .bind(id)
        .execute(pg)
        .await?;
    Ok(())
}

/// Bind/unbind an agent's default templates (null clears; the patch carries
/// only the fields the request defined).
pub async fn set_agent_templates(
    pg: &PgPool,
    agent_model: &str,
    ticket_template_id: Option<Option<&str>>,
    plan_template_id: Option<Option<&str>>,
) -> Result<(), sqlx::Error> {
    if let Some(t) = ticket_template_id {
        sqlx::query("update agent_defs set ticket_template_id = $1::uuid where model = $2")
            .bind(t)
            .bind(agent_model)
            .execute(pg)
            .await?;
    }
    if let Some(t) = plan_template_id {
        sqlx::query("update agent_defs set plan_template_id = $1::uuid where model = $2")
            .bind(t)
            .bind(agent_model)
            .execute(pg)
            .await?;
    }
    Ok(())
}

/// An agent's template overrides — an engineering agent always writing eng
/// tickets, regardless of board. No row → no bindings, never an error.
pub struct AgentTemplateBindings {
    pub ticket_template_id: Option<String>,
    pub plan_template_id: Option<String>,
}

pub async fn agent_template_bindings(
    pg: &PgPool,
    agent_model: &str,
) -> Result<AgentTemplateBindings, sqlx::Error> {
    let row: Option<(Option<String>, Option<String>)> = sqlx::query_as(
        "select ticket_template_id::text, plan_template_id::text \
         from agent_defs where model = $1",
    )
    .bind(agent_model)
    .fetch_optional(pg)
    .await?;
    Ok(match row {
        Some((ticket_template_id, plan_template_id)) => AgentTemplateBindings {
            ticket_template_id,
            plan_template_id,
        },
        None => AgentTemplateBindings {
            ticket_template_id: None,
            plan_template_id: None,
        },
    })
}

/// The context resolution runs against — every link optional, absent links
/// skipped.
pub struct ResolveContext<'a> {
    pub explicit_id: Option<&'a str>,
    pub agent_model: Option<&'a str>,
    pub board_id: Option<&'a str>,
}

/// One row narrowed to the requested kind — the chain's shared
/// "dead or wrong-kind reference falls through" step.
async fn template_of_kind(
    pg: &PgPool,
    id: &str,
    kind: &str,
) -> Result<Option<Template>, sqlx::Error> {
    Ok(get_template(pg, id).await?.filter(|t| t.kind == kind))
}

/// The template that applies in a context: explicit pick → the agent's own
/// binding → the board's default (tickets only) → none (freeform). Dead
/// references — a deleted template, or one of the wrong kind — fall through
/// to the next link rather than erroring; a ticket must never fail to seed
/// because its board pointed at a ghost.
pub async fn resolve_template(
    pg: &PgPool,
    kind: &str,
    ctx: &ResolveContext<'_>,
) -> Result<Option<Template>, sqlx::Error> {
    if let Some(explicit) = ctx.explicit_id.filter(|id| !id.is_empty())
        && let Some(t) = template_of_kind(pg, explicit, kind).await?
    {
        return Ok(Some(t));
    }
    if let Some(agent_model) = ctx.agent_model.filter(|m| !m.is_empty()) {
        let b = agent_template_bindings(pg, agent_model).await?;
        let id = if kind == "ticket" {
            b.ticket_template_id
        } else {
            b.plan_template_id
        };
        if let Some(id) = id.filter(|id| !id.is_empty())
            && let Some(t) = template_of_kind(pg, &id, kind).await?
        {
            return Ok(Some(t));
        }
    }
    if kind == "ticket"
        && let Some(board_id) = ctx.board_id.filter(|id| !id.is_empty())
    {
        let default = board_templates(pg, board_id)
            .await?
            .into_iter()
            .find(|b| b.is_default);
        if let Some(def) = default
            && let Some(t) = template_of_kind(pg, &def.template_id, kind).await?
        {
            return Ok(Some(t));
        }
    }
    Ok(None)
}

/// The prompt block a template contributes to an agent's instructions. `what`
/// names the surface — ticket descriptions or the plan document.
pub fn template_prompt(t: &Template, what: &str) -> String {
    let mut parts = vec![format!(
        "Format {what} on this template — keep its headings/section structure, \
         fill every section (use \"n/a\" only when truly inapplicable):\n<<<\n{}\n>>>",
        t.body
    )];
    let guidance = t.guidance.trim();
    if !guidance.is_empty() {
        parts.push(format!("Template guidance: {guidance}"));
    }
    parts.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_prompt_carries_body_and_trimmed_guidance() {
        let t = Template {
            id: "t-1".into(),
            name: "Eng ticket".into(),
            kind: "ticket".into(),
            body: "## Context\n…".into(),
            guidance: "  be terse  ".into(),
            created_by: None,
            updated_at: "2026-01-01T00:00:00.000Z".into(),
        };
        let p = template_prompt(&t, "ticket descriptions");
        assert!(p.starts_with(
            "Format ticket descriptions on this template — keep its headings/section structure, \
             fill every section (use \"n/a\" only when truly inapplicable):\n<<<\n## Context\n…\n>>>"
        ));
        assert!(p.ends_with("Template guidance: be terse"));
        // Blank guidance contributes nothing — no dangling header.
        let mut bare = t.clone();
        bare.guidance = "   ".into();
        assert!(!template_prompt(&bare, "the plan document").contains("Template guidance"));
    }
}
