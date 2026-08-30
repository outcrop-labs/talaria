// WHAT AN AGENT IS TOLD IT HAS — names and labels, never values
// (workspace-secrets.ts grantedHandlesFor). The handle mechanism itself
// substitutes at the MCP gateway; this is the string a rendered soul can
// carry: it tells a model which handles exist for it, so it can use one
// deliberately rather than invent a name. A model that has been granted
// nothing is told nothing, which is also correct — a list of names it cannot
// use is a map of the workspace's credentials.

use sqlx::PgPool;

/// The handle token a prompt carries: «secret:doc» or «secret:doc.entry».
pub fn handle_for(doc: &str, entry: Option<&str>) -> String {
    match entry {
        Some(e) => format!("«secret:{doc}.{e}»"),
        None => format!("«secret:{doc}»"),
    }
}

pub struct HandleRow {
    pub name: String,
    pub key: String,
    pub label: String,
}

/// The briefing text for a set of granted handles (workspace-secrets.ts
/// handleBriefing). Pure — the render calls it through granted_handles_for.
pub fn handle_briefing(rows: &[HandleRow]) -> String {
    if rows.is_empty() {
        return String::new();
    }
    // Rows arrive (name, key)-ordered; group by document preserving that.
    let mut docs: Vec<(String, Vec<(String, String)>)> = Vec::new();
    for r in rows {
        match docs.iter_mut().find(|(name, _)| *name == r.name) {
            Some((_, entries)) => entries.push((r.key.clone(), r.label.clone())),
            None => docs.push((r.name.clone(), vec![(r.key.clone(), r.label.clone())])),
        }
    }
    let lines: Vec<String> = docs
        .iter()
        .map(|(name, es)| {
            if es.len() == 1 {
                let (.., label) = &es[0];
                format!("{} ({label})", handle_for(name, None))
            } else {
                es.iter()
                    .map(|(key, label)| format!("{} ({label})", handle_for(name, Some(key))))
                    .collect::<Vec<_>>()
                    .join(", ")
            }
        })
        .collect();
    format!(
        "Credentials you may USE without seeing: {}. Pass the handle exactly as written wherever the value would go — Talaria substitutes it at the boundary. You will never be shown the value, and a handle you invent resolves to nothing.",
        lines.join("; ")
    )
}

/// The granted handles for one agent model, as the briefing text ('' when it
/// holds nothing). Direct grants and folder grants both qualify; expired or
/// exhausted secrets do not. A failure is '' — the render treats the agent as
/// holding nothing rather than failing (TS's `.catch(() => '')` at the call).
pub async fn granted_handles_for(pg: &PgPool, caller: &str) -> Result<String, sqlx::Error> {
    let rows: Vec<(String, String, String)> = sqlx::query_as(
        "select distinct s.name, e.key, e.label \
         from workspace_secrets s \
         join workspace_secret_entries e on e.secret_id = s.id \
         left join workspace_secret_grants g on g.secret_id = s.id and g.agent_model = $1 \
         left join secret_folder_grants fg on fg.folder_id = s.secret_folder_id and fg.agent_model = $1 \
         where (g.agent_model is not null or fg.agent_model is not null) \
           and (s.expires_at is null or s.expires_at > now()) \
           and (s.uses_remaining is null or s.uses_remaining > 0) \
         order by s.name, e.key",
    )
    .bind(caller)
    .fetch_all(pg)
    .await?;
    Ok(handle_briefing(
        &rows
            .into_iter()
            .map(|(name, key, label)| HandleRow { name, key, label })
            .collect::<Vec<_>>(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handles_spell_the_doc_and_entry_forms() {
        assert_eq!(handle_for("github", None), "«secret:github»");
        assert_eq!(handle_for("github", Some("prod")), "«secret:github.prod»");
    }

    #[test]
    fn the_briefing_groups_by_document_and_names_each_entry() {
        let one = vec![HandleRow {
            name: "github".into(),
            key: "token".into(),
            label: "org token".into(),
        }];
        assert_eq!(
            handle_briefing(&one),
            "Credentials you may USE without seeing: «secret:github» (org token). Pass the handle exactly as written wherever the value would go — Talaria substitutes it at the boundary. You will never be shown the value, and a handle you invent resolves to nothing."
        );
        // Two entries on one doc → per-entry handles, comma-joined.
        let two = vec![
            HandleRow {
                name: "aws".into(),
                key: "prod".into(),
                label: "prod".into(),
            },
            HandleRow {
                name: "aws".into(),
                key: "staging".into(),
                label: "staging".into(),
            },
            HandleRow {
                name: "figma".into(),
                key: "token".into(),
                label: "team".into(),
            },
        ];
        let b = handle_briefing(&two);
        assert!(b.contains(
            "«secret:aws.prod» (prod), «secret:aws.staging» (staging); «secret:figma» (team)"
        ));
        assert_eq!(handle_briefing(&[]), "");
    }
}
