// "Request a change": a person asks for something in a plugin, from its card
// in Manage → Apps, and it becomes a ticket for the plugin's maintainer on
// the plugin's board — the board of the ticket that started it. The person
// asking assigns it, which is what a person does on any board; the agent's
// session starts from the ticket like any other assignment.

use serde::Serialize;
use sqlx::PgPool;

use talaria_tasks::{NewTask, TaskDeps, create_task};

use crate::Plugin;

/// The longest request a ticket carries.
pub const REQUEST_MAX: usize = 4000;

/// The ticket's title: the request's first line, kept short.
pub fn change_title(slug: &str, request: &str) -> String {
    let first = request
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim();
    let mut title: String = first.chars().take(80).collect();
    if first.chars().count() > 80 {
        title.push('…');
    }
    format!("[plugin {slug}] {title}")
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangeFiled {
    pub task_id: String,
    pub ticket_ref: Option<String>,
    pub board_id: String,
}

/// File the request. Err is the sentence the person reads. `requester` is the
/// user id (the ticket's creator); `requester_name` is how the ticket says
/// who asked.
pub async fn request_change(
    deps: &TaskDeps,
    p: &Plugin,
    request: &str,
    requester: &str,
    requester_name: &str,
) -> Result<ChangeFiled, String> {
    let pg: &PgPool = &deps.pg;
    let request = request.trim();
    if request.is_empty() {
        return Err("say what should change".into());
    }
    let Some(board_id) = p.board_id.as_deref() else {
        return Err(format!(
            "plugin \"{}\" has no board for its tickets — the ticket that started it is gone",
            p.app_slug
        ));
    };
    let Some(agent) = p.maintainer_model.as_deref() else {
        return Err(format!(
            "plugin \"{}\" has no maintaining agent any more",
            p.app_slug
        ));
    };
    let description = format!(
        "{request}\n\n— Requested by {requester_name} from Manage → Apps, for the **{}** plugin ({}). Its maintainer is {agent}: read the plugin with `plugin_status`, build the change on its repo, and open a PR — a person merges it, and an admin approves the update.",
        p.app_slug, p.repo
    );
    let task = create_task(
        deps,
        &NewTask {
            board_id,
            title: &change_title(&p.app_slug, request),
            description: Some(&description),
            priority: None,
            effort: None,
            assignees: &[agent.to_string()],
            due_date: None,
            start_date: None,
            color: None,
            estimated_hours: None,
            parent_id: None,
            tags: &[],
            created_by: requester,
        },
    )
    .await
    .map_err(|e| e.message())?;
    if let Err(e) = sqlx::query(
        "insert into plugin_notices (app_slug, kind, task_id, detail, created_by) \
         values ($1, 'change_request', $2::uuid, $3, $4)",
    )
    .bind(&p.app_slug)
    .bind(&task.id)
    .bind(request)
    .bind(requester)
    .execute(pg)
    .await
    {
        tracing::warn!(
            "[plugins] {}: change request record failed: {e}",
            p.app_slug
        );
    }
    Ok(ChangeFiled {
        task_id: task.id,
        ticket_ref: task.ticket_ref,
        board_id: board_id.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_title_is_the_first_line_kept_short() {
        assert_eq!(
            change_title("notes", "\n  Add tags to notes\nand a filter"),
            "[plugin notes] Add tags to notes"
        );
        let long = "x".repeat(100);
        let t = change_title("notes", &long);
        assert!(t.ends_with('…') && t.chars().count() == "[plugin notes] ".len() + 81);
    }
}
