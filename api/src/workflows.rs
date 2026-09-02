// Task workflows — the hook layer between "an agent got a ticket" and "the
// agent works it the right way": the CRUD half, the match classifiers that
// decide which hook a ticket pulls in, and the routing map a plan draft
// reads to route its proposals. match/skills/toolkits/env are jsonb passed
// through untouched — the DB's stored key order is the wire order.

use sqlx::PgPool;

/// The row as LIST/CREATE serve it — ROW order:
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

/// The PUT patch — every field Option<"present">, absent fields untouched:
/// one update statement per present field, in this order, no transaction.
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

/// What a match is decided against, borrowed: the heartbeat holds the
/// tickets it just fetched and should not have to clone them to classify
/// them.
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

/// Does this hook pull in on this ticket?
///
/// A hook matches when EVERY facet it declares is satisfied, and a hook that
/// declares nothing matches NOTHING (a bare `every` on zero facets would
/// match everything, and an empty-rules workflow would silently become the
/// org-wide default hook). Non-string entries in a facet simply never equal
/// a tag/board id.
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
        // spanning the seam matches. Case-folded both sides.
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

/// The match against a list the caller already holds. Batch callers (the
/// heartbeat walks every servable ticket) MUST use this with one
/// `list_workflows()` hoisted out of their loop — `workflows_for_task` per
/// ticket re-reads the whole table each time. Kept as the single expression
/// of the match so the hot path cannot drift from the one-off path.
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

// ── The routing map ──────────────────────────────────────────────────────────

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

/// The skill-name grammar — a directory under a skills root is a skill only
/// when its name matches. Same constant as
/// runs/defs/work_session.rs, kept local: a routing map and a work session
/// ask the same question of the same tree, and neither should reach into the
/// other's module for a one-line grammar.
static SKILL_NAME_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new("^[a-z0-9][a-z0-9._-]*$").unwrap());

/// One skill owner's name set, name-only: the shared root first, then every
/// enabled agent. The routing map asks only "who carries this skill", so
/// the SKILL.md summaries, the platform set, and the queued regeneration
/// pipeline are deliberately not read — a name-only dir listing answers it
/// without firing the summarizer.
async fn skill_owners(pg: &PgPool) -> Result<Vec<(String, String, HashSet<String>)>, sqlx::Error> {
    let fleet = crate::gateway::provider::fleet_dir();
    let defs: Vec<(String, String)> =
        sqlx::query_as("select slug, display_name from agent_defs where enabled order by slug")
            .fetch_all(pg)
            .await?;
    let mut out = Vec::with_capacity(defs.len() + 1);
    out.push((
        "shared".into(),
        "Shared (all agents)".into(),
        skill_names(&fleet.join("skills")).await,
    ));
    for (slug, display_name) in defs {
        out.push((
            slug.clone(),
            display_name,
            skill_names(&fleet.join("agents").join(slug).join("skills")).await,
        ));
    }
    Ok(out)
}

async fn skill_names(root: &std::path::Path) -> HashSet<String> {
    let mut names = HashSet::new();
    let Ok(mut entries) = tokio::fs::read_dir(root).await else {
        return names; // no root yet is no skills, not an error
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        if let Ok(file_type) = entry.file_type().await
            && file_type.is_dir()
        {
            let name = entry.file_name().to_string_lossy().into_owned();
            if SKILL_NAME_RE.is_match(&name) {
                names.insert(name);
            }
        }
    }
    names
}

/// A workflow's three named facets as string vectors. Non-array, absent, or
/// non-string entries yield nothing; only the three names are read, and any
/// other key in the jsonb is ignored.
fn match_facets(m: &serde_json::Value) -> Vec<(&'static str, Vec<&str>)> {
    let facet = |key: &str| -> Vec<&str> {
        m.get(key)
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
            .unwrap_or_default()
    };
    vec![
        ("boards", facet("boards")),
        ("labels", facet("labels")),
        ("keywords", facet("keywords")),
    ]
}

/// A compact, model-readable map of the org's routing: enabled workflows
/// with match rules, bound skills, and which agents carry those skills —
/// the context a plan draft (and the plan surface's aside) reads to route
/// ticket work. Empty string when there is nothing to route by; callers
/// omit the section.
///
/// The first TWENTY workflows render — the map is context, not an index. A
/// skill the shared root carries says "any agent" and stops the walk there
/// (shared is first in the owner list), because then every agent has it;
/// anything else accumulates every carrying agent's label.
pub async fn routing_context(pg: &PgPool) -> Result<String, sqlx::Error> {
    let all: Vec<Workflow> = list_workflows(pg)
        .await?
        .into_iter()
        .filter(|w| {
            w.enabled
                && (w.skills.as_array().is_some_and(|s| !s.is_empty())
                    || match_facets(&w.r#match).iter().any(|(_, v)| !v.is_empty()))
        })
        .collect();
    if all.is_empty() {
        return Ok(String::new());
    }
    let boards: Vec<(String, String)> =
        sqlx::query_as("select id::text, name from boards where archived_at is null")
            .fetch_all(pg)
            .await?;
    let board_name: HashMap<String, String> = boards.into_iter().collect();
    let owners = skill_owners(pg).await?;
    let carriers = |skill: &str| -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for (owner, label, names) in &owners {
            if !names.contains(skill) {
                continue;
            }
            if owner == "shared" {
                return vec!["any agent".into()];
            }
            out.push(label.clone());
        }
        out
    };
    let lines: Vec<String> = all
        .iter()
        .take(20)
        .map(|w| {
            let facets = match_facets(&w.r#match);
            let named = |key: &str| {
                facets
                    .iter()
                    .find(|(k, _)| *k == key)
                    .map(|(_, v)| v.clone())
            };
            let rules = [
                named("boards").filter(|b| !b.is_empty()).map(|b| {
                    format!(
                        "boards: {}",
                        b.iter()
                            .map(|id| board_name
                                .get(*id)
                                .cloned()
                                .unwrap_or_else(|| id.to_string()))
                            .collect::<Vec<_>>()
                            .join("/")
                    )
                }),
                named("labels")
                    .filter(|l| !l.is_empty())
                    .map(|l| format!("labels: {}", l.join(", "))),
                named("keywords")
                    .filter(|k| !k.is_empty())
                    .map(|k| format!("keywords: {}", k.join(", "))),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join("; ");
            let skills = w
                .skills
                .as_array()
                .filter(|s| !s.is_empty())
                .map(|entries| {
                    entries
                        .iter()
                        .filter_map(|s| s.as_str())
                        .map(|sk| {
                            let c = carriers(sk);
                            if c.is_empty() {
                                format!("{sk} (no agent carries this yet)")
                            } else {
                                format!("{sk} ({})", c.join(", "))
                            }
                        })
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_else(|| "(no skills bound)".into());
            format!(
                "- {} — matches [{}] → skills: {}",
                w.name,
                if rules.is_empty() { "no rules" } else { &rules },
                skills
            )
        })
        .collect();
    Ok(lines.join("\n"))
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
        // A non-string entry never equals a string tag.
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

    #[test]
    fn match_facets_read_the_three_named_arrays_and_skip_everything_else() {
        // Non-array / absent / non-string facets are empty.
        let rules = json!({
            "boards": ["b-1", "b-2"],
            "labels": [],
            "keywords": "not an array",
            "other": ["unknown facet"]
        });
        let f = match_facets(&rules);
        assert_eq!(f[0], ("boards", vec!["b-1", "b-2"]));
        assert!(f[1].1.is_empty());
        assert!(f[2].1.is_empty());
        // null/absent match → all three empty.
        for m in [json!(null), json!({})] {
            assert!(match_facets(&m).iter().all(|(_, v)| v.is_empty()));
        }
        // A facet holding non-strings keeps only the strings — the join in
        // the rendered line never sees a Value.
        let mixed = json!({"labels": [1, "support", true]});
        let f = match_facets(&mixed);
        assert_eq!(f[1].1, vec!["support"]);
    }
}
