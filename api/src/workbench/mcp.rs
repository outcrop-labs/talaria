// The Workbench MCP — the governed surface agents drive their sandbox
// through. Registered in the MCP registry like any server (grantable per
// agent), dispatched in-process like app surfaces. Verbs are PROFILE-SCOPED:
// the dev workbench exposes the job lifecycle below; later profiles (design,
// data, content) expose their own gated verbs from the same dispatcher.
//
// The git-flow contract (why it never gets messy): Talaria cuts the branch
// (talaria/<ticket-ref>-<slug>) from default at start_job, the harness works
// ONLY inside that branch via the authenticated clone URL, finish_job opens
// the PR with the templated ticket-linked body. No raw pushes to default —
// the workbench token is the only credential in the sandbox, and every
// transition lands in task_activity ON A TICKET STILL OPEN TO AGENTS: an audit
// line is a write to the ticket like any other, so it goes through the one gate
// (`logTicket` → `authorizeTicket`) rather than around it.
//
// THIS FILE TOUCHES A TICKET IN EXACTLY THREE WAYS, and each has ONE door:
//   · the ticket an agent NAMES in its arguments  → `ticket_arg` (parse + gate,
//     one step, so `args.taskId` is read in one place in this file)
//   · an AUDIT LINE on that ticket                → `log_ticket`
//   · the plan COMMENT and the plan artifact chip → `add_comment` / `update_task`
//     from tasks.rs, as the agent
// There is no fourth way and no raw `insert into task_…` / `update tasks` here.
// That is the whole invariant: hand-written door copies drift out of sync,
// and any count of them drifts too. These are not copies, so there is
// nothing to count.
//
// The dispatcher consumes mcp_jsonrpc (the shared envelope); the verbs ride
// github.rs's REST half and the harness registry's effort chain.

use axum::http::StatusCode;
use serde_json::{Map, Value, json};
use sqlx::PgPool;

use crate::agent_auth::{AgentSubject, epoch_ms_to_iso, subject_model};
use crate::approvals::{ApprovalDeps, announce_approval};
use crate::artifacts::{SaveArtifactPatch, agent_category_folder, create_artifact, save_artifact};
use crate::boards::board_allows_agent;
use crate::body::truncate_utf16;
use crate::fleet::describe_agent;
use crate::github as gh;
use crate::github::github_status;
use crate::mcp::jsonrpc::{ListedTool, ToolOutcome, dispatch_jsonrpc};
use crate::realtime::RealtimeDeps;
use crate::runs::define::run_definition;
use crate::secretbox::SecretBox;
use crate::tasks::{
    AgentIntent, AgentWriteTarget, TaskActor, TaskDeps, TaskPatch, add_comment,
    agent_ticket_refusal, get_task, log_activity, update_task,
};
use crate::workbench::harnesses::{
    HarnessSource, effort_model, effort_models, harness_model_arg, list_harness_defs,
};
use crate::workbench::resolve_workbench;

/// Everything a verb reaches for past its own SQL: the pool, the secretbox
/// (GitHub credentials unseal through it), and the optional Redis the task
/// notification/dispatch edges degrade without — the trio `task_deps()`
/// packages for tasks.rs callers, held once so the fire-and-forget legs can
/// take a copy into their own task.
#[derive(Clone)]
pub struct WorkbenchDeps {
    pub pg: PgPool,
    pub sb: SecretBox,
    pub redis: Option<redis::aio::ConnectionManager>,
}

impl WorkbenchDeps {
    pub fn task_deps(&self) -> TaskDeps {
        TaskDeps::coexistence(self.pg.clone(), self.redis.clone())
    }
}

fn wall_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub struct WorkbenchJob {
    pub id: String,
    pub agent_id: String,
    pub agent_model: String,
    pub task_id: Option<String>,
    pub repo: String,
    pub branch: String,
    pub effort: String,
    pub plan: String,
    pub status: String,
    pub pr_url: Option<String>,
    pub summary: String,
    pub created_at: String,
    pub updated_at: String,
}

/// The agent-side job column list (the agent side carries agentId; the
/// ticket strip answers a different column set and shapes its own row in
/// the route).
const JOB_COLS: &str = "id::text, agent_id::text, agent_model, task_id::text, repo, branch, \
                        effort, plan, status, pr_url, summary, \
                        (trunc(extract(epoch from created_at) * 1000))::bigint, \
                        (trunc(extract(epoch from updated_at) * 1000))::bigint";

type JobRow = (
    String,
    String,
    String,
    Option<String>,
    String,
    String,
    String,
    String,
    String,
    Option<String>,
    String,
    i64,
    i64,
);

impl WorkbenchJob {
    fn of(r: JobRow) -> WorkbenchJob {
        WorkbenchJob {
            id: r.0,
            agent_id: r.1,
            agent_model: r.2,
            task_id: r.3,
            repo: r.4,
            branch: r.5,
            effort: r.6,
            plan: r.7,
            status: r.8,
            pr_url: r.9,
            summary: r.10,
            created_at: epoch_ms_to_iso(r.11),
            updated_at: epoch_ms_to_iso(r.12),
        }
    }

    /// The row's wire — JOB_COLS' select order minus `plan` (no caller of
    /// this wire ships the plan), timestamps as ISO instants (the
    /// Date-through-JSON.stringify rendering the wire has always carried).
    fn wire(&self) -> Value {
        let mut out = Map::new();
        out.insert("id".into(), json!(self.id));
        out.insert("agentId".into(), json!(self.agent_id));
        out.insert("agentModel".into(), json!(self.agent_model));
        out.insert(
            "taskId".into(),
            self.task_id
                .clone()
                .map(Value::String)
                .unwrap_or(Value::Null),
        );
        out.insert("repo".into(), json!(self.repo));
        out.insert("branch".into(), json!(self.branch));
        out.insert("effort".into(), json!(self.effort));
        out.insert("status".into(), json!(self.status));
        out.insert(
            "prUrl".into(),
            self.pr_url
                .clone()
                .map(Value::String)
                .unwrap_or(Value::Null),
        );
        out.insert("summary".into(), json!(self.summary));
        out.insert("createdAt".into(), json!(self.created_at));
        out.insert("updatedAt".into(), json!(self.updated_at));
        Value::Object(out)
    }
}

pub fn slugify(v: &str) -> String {
    let lower = v.trim().to_lowercase();
    let collapsed = regex::Regex::new("[^a-z0-9]+")
        .expect("slugify collapse pattern")
        .replace_all(&lower, "-");
    truncate_utf16(collapsed.trim_matches('-'), 40).to_string()
}

/// Ticket ref + title (refs are computed: board prefix + ticket_no).
async fn ticket_ref_of(pg: &PgPool, task_id: &str) -> Option<(Option<String>, String)> {
    let row: Option<(Option<String>, String)> = sqlx::query_as(
        "select case when t.ticket_no is not null then coalesce(b.ticket_prefix, 'TASK') || '-' || t.ticket_no end, \
                t.title \
         from tasks t join boards b on b.id = t.board_id where t.id = $1::uuid",
    )
    .bind(task_id)
    .fetch_optional(pg)
    .await
    .ok()
    .flatten();
    row
}

/// The seven tools — verbatim inputSchemas, their own key order (name,
/// description, inputSchema; properties in declaration order).
pub fn workbench_tools() -> Vec<Value> {
    vec![
        json!({
            "name": "doctor",
            "description": "Diagnose YOUR workbench end to end: profile, chosen harness (with a probe command to run in your shell), auth, GitHub connection, repo grants, effort→model map, and pass-through config locations. Run this first when anything about your workbench misbehaves — or before your first job.",
            "inputSchema": { "type": "object", "properties": {} },
        }),
        json!({
            "name": "list_repos",
            "description": "The repositories YOUR workbench is granted. Work only these — anything else is out of bounds.",
            "inputSchema": { "type": "object", "properties": {} },
        }),
        json!({
            "name": "start_job",
            "description": "Start a workbench job for a ticket. Talaria cuts the working branch from the default branch and returns an authenticated clone URL. Work ONLY on that branch; commit and push to it as you go. For feature-scale work, write your plan first (it is recorded and rides into the PR). One job per ticket at a time.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "taskId": { "type": "string", "description": "The ticket this job implements — ALWAYS pass it when the work came from a ticket; it links the branch, audit trail, plan gate, and PR to the ticket. Refused if that ticket is one you may not work: a board you are not allowed on, a closed ticket (done / failed / cancelled), an archived ticket, or a ticket on an archived board. Ask for it to be reopened, or work the follow-up ticket." },
                    "repo": { "type": "string", "description": "owner/name — must be one of your granted repos" },
                    "effort": { "type": "string", "enum": ["light", "standard", "heavy"], "description": "How hard this work is — routes tooling and review weight" },
                    "plan": { "type": "string", "description": "Your implementation plan: approach, files touched, test strategy. Required for standard/heavy." },
                },
                "required": ["repo"],
            },
        }),
        json!({
            "name": "job_status",
            "description": "Your workbench jobs (optionally one by id): branch, status, PR link.",
            "inputSchema": { "type": "object", "properties": { "jobId": { "type": "string" } } },
        }),
        json!({
            "name": "merge_to_testing",
            "description": "Merge a job's branch into the repo's TESTING branch for integration testing (only when the repo has one configured). The PR to the base branch stays open and unmerged — testing is a sideline, never the way work ships.",
            "inputSchema": { "type": "object", "properties": { "jobId": { "type": "string" } }, "required": ["jobId"] },
        }),
        json!({
            "name": "request_repo",
            "description": "Request a NEW repository in an approved org — a human approves before anything is created (you will see it in list_repos once granted). Use only when the work genuinely needs a fresh repo; explain why.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "org": { "type": "string", "description": "The GitHub org — must be on the approved list (doctor shows it)" },
                    "name": { "type": "string", "description": "Repo name (lowercase, dashes)" },
                    "description": { "type": "string", "description": "One-line repo description" },
                    "why": { "type": "string", "description": "Why this work needs a new repo" },
                    "taskId": { "type": "string", "description": "The ticket that motivated it — same rule as start_job: a ticket you may not work (board not yours, closed, archived, archived board) is refused, so omit it rather than guessing." },
                },
                "required": ["org", "name", "why"],
            },
        }),
        json!({
            "name": "finish_job",
            "description": "Finish a job: Talaria verifies the branch has commits and opens the pull request with the ticket-linked body. Returns the PR URL — put it in your outcome report. Use abandon:true to close out a job that produced nothing. Either way the job closes out; the ticket only gets an audit line if it is still open to you (a ticket closed or archived while you worked takes no further agent writes), so report the PR URL yourself.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "jobId": { "type": "string" },
                    "summary": { "type": "string", "description": "What the change does — becomes the PR body core" },
                    "abandon": { "type": "boolean" },
                },
                "required": ["jobId"],
            },
        }),
    ]
}

/// The agent columns the verbs read — the row `agent_by_model` fetches.
#[derive(Debug, Clone)]
pub struct AgentCtx {
    pub id: String,
    pub model: String,
    pub display_name: String,
    pub department: String,
    pub role: Option<String>,
    /// 'off' | 'auto' | 'on' (a bare string: the column's domain).
    pub workbench: String,
    pub workbench_profile: Option<String>,
    pub workbench_harness: Option<String>,
    pub workbench_models: Option<Map<String, Value>>,
}

/// agent_defs' workbench column set — nine wide, too wide to spell inline.
type AgentModelRow = (
    String,
    String,
    String,
    String,
    Option<String>,
    String,
    Option<String>,
    Option<String>,
    Option<Value>,
);

async fn agent_by_model(pg: &PgPool, model: &str) -> Result<Option<AgentCtx>, sqlx::Error> {
    let row: Option<AgentModelRow> = sqlx::query_as(
        "select id::text, model, display_name, department, role, workbench, \
                workbench_profile, workbench_harness, workbench_models \
         from agent_defs where model = $1 and enabled",
    )
    .bind(model)
    .fetch_optional(pg)
    .await?;
    Ok(row.map(|r| AgentCtx {
        id: r.0,
        model: r.1,
        display_name: r.2,
        department: r.3,
        role: r.4,
        workbench: r.5,
        workbench_profile: r.6,
        workbench_harness: r.7,
        workbench_models: r.8.and_then(|v| v.as_object().cloned()),
    }))
}

/// Who a workbench audit line is written BY — and the reason `log_ticket`
/// takes this instead of a bare actor string.
///
/// A `task_activity` row on a ticket IS a write to that ticket. An AGENT's
/// line therefore has to pass the same gate every other agent write passes; a
/// HUMAN's (the ticket-strip approve / reject / merge-to-testing actions) does
/// not, because the route already checked board edit rights.
///
/// `Agent` is the ONLY way to write an agent line, and it carries the subject
/// the gate needs — there is no shape that says "an agent wrote this" and
/// skips the check. The bare-string form is the human one, and it is VERIFIED
/// rather than believed (see `agent_behind`): a string that names a fleet
/// agent is treated as an agent write, so the human door cannot be used to
/// launder an agent one back in.
#[derive(Debug, Clone)]
pub enum WorkbenchActor {
    Human(String),
    Agent(AgentSubject),
}

/// The agent behind a WorkbenchActor, or None for a genuine human.
/// Deliberately not `agent_by_model` — that filters on `enabled`, and a
/// disabled agent's writes must not fall through the gate by being read as a
/// person's.
async fn agent_behind(pg: &PgPool, by: &WorkbenchActor) -> Option<AgentSubject> {
    match by {
        WorkbenchActor::Agent(subject) => Some(subject.clone()),
        WorkbenchActor::Human(name) => {
            let hit =
                sqlx::query_scalar::<_, i32>("select 1 from agent_defs where model = $1 limit 1")
                    .bind(name)
                    .fetch_optional(pg)
                    .await
                    .ok()
                    .flatten();
            hit.map(|_| AgentSubject::Model(name.clone()))
        }
    }
}

/// THE ONLY door onto a ticket's workbench audit trail, and it asks the gate
/// itself. Silence on refusal is deliberate: these lines are audit trail,
/// never the operation. A refused line is skipped — the merge still merged,
/// the PR is still open, the job is still abandoned, and the agent has all of
/// that in its tool result. Returns whether the line landed, for callers that
/// want to say so.
///
/// EXPORTED on purpose: it is the workbench's only ticket-audit door, and a
/// new workbench surface reaching for `log_activity` directly is the next
/// copy of the bug this shape exists to end.
pub async fn log_ticket(
    pg: &PgPool,
    task_id: Option<&str>,
    by: &WorkbenchActor,
    description: &str,
) -> bool {
    let Some(task_id) = task_id else { return false };
    let subject = agent_behind(pg, by).await;
    if let Some(subject) = &subject
        && authorize_ticket(pg, task_id, subject).await.is_err()
    {
        return false;
    }
    let actor = match by {
        WorkbenchActor::Human(name) => name.clone(),
        WorkbenchActor::Agent(subject) => subject_model(subject).to_string(),
    };
    log_activity(pg, task_id, &actor, "workbench", description)
        .await
        .is_ok()
}

/// THE GATE — for a caller-supplied taskId AND for every workbench audit line
/// (`log_ticket` calls this; nothing else may). Verbs here take the ticket
/// from the agent, and everything downstream either DISCLOSES it (the ticket
/// ref and title ride into the branch name and, at finish_job, into a public
/// PR title and body) or WRITES to it (a plan comment authored as the agent,
/// the plan artifact chip on the ticket, workbench audit lines).
///
/// Both rules are `agent_ticket_refusal`, IMPORTED — ONE predicate answers
/// policy AND ticket state, so this door cannot ask half the question.
/// Unknown and not-allowed refuse with the SAME message: a distinct "no such
/// ticket" would turn this verb into a ticket enumeration oracle.
async fn authorize_ticket(
    pg: &PgPool,
    task_id: &str,
    subject: &AgentSubject,
) -> Result<(), String> {
    let deny = || {
        format!(
            "taskId \"{task_id}\" is not a ticket you may work — it does not exist, or its board \
             does not allow you. Omit taskId, or ask an admin for access to that board."
        )
    };
    let Ok(Some(task)) = get_task(pg, task_id).await else {
        return Err(deny());
    };
    if !board_allows_agent(pg, &task.board_id, subject)
        .await
        .unwrap_or(false)
    {
        return Err(deny());
    }
    let target = AgentWriteTarget::from(&task);
    if let Ok(Some(shut)) = agent_ticket_refusal(pg, &target, subject, AgentIntent::Write).await {
        return Err(format!(
            "{shut}. Ticket {} is \"{}\". Ask for it to be reopened, or work the follow-up ticket.",
            task.ticket_ref.as_deref().unwrap_or(task_id),
            task.status
        ));
    }
    Ok(())
}

/// THE ONLY way a verb gets a ticket out of its OWN arguments — parsing and
/// the gate are one step, and `args.taskId` is read here and nowhere else in
/// this file. A verb cannot hold an agent-supplied taskId it has not gated:
/// the ungated read is not a thing you can write, rather than a thing you
/// must remember not to write.
///
/// Absent/blank taskId is `Ok(None)`: omitting the ticket is legal on both
/// verbs that take one, and only a NAMED ticket is a claim to be checked.
async fn ticket_arg(
    pg: &PgPool,
    subject: &AgentSubject,
    args: &Map<String, Value>,
) -> Result<Option<String>, String> {
    let task_id = args
        .get("taskId")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty());
    let Some(task_id) = task_id else {
        return Ok(None);
    };
    let task_id = task_id.to_string();
    authorize_ticket(pg, &task_id, subject).await?;
    Ok(Some(task_id))
}

/// Board policy alone, for the DISCLOSURE point (finish_job): may this agent
/// still see this ticket? Deliberately NOT the closed check — a person
/// closing the ticket while the job ran should not cost the PR its ticket
/// link.
async fn ticket_still_ours(pg: &PgPool, subject: &AgentSubject, task_id: &str) -> bool {
    let Ok(Some(task)) = get_task(pg, task_id).await else {
        return false;
    };
    board_allows_agent(pg, &task.board_id, subject)
        .await
        .unwrap_or(false)
}

/// THE ONLY way this file tells anybody an approval was raised. Not one of
/// the three ticket doors at the top of the file — a door onto somebody's
/// inbox — but it is one for exactly the same reason: two verbs here park
/// work on a human (`workbench_plan`, `repo_request`), both declared in
/// approvals.rs with the AUTHORITY their decision route enforces, and that
/// declaration is what decides who may be told and how much of it.
///
/// Fire-and-forget (the caller is servicing an agent's tool call),
/// idempotent against the approval sweep (both mark the same key, merged in
/// the database), and it never decides an audience — a third gated verb gets
/// the disclosure rules right by calling this and cannot get them wrong by
/// not calling it.
fn announce_raised(deps: &WorkbenchDeps, key: String) {
    let pg = deps.pg.clone();
    let realtime = RealtimeDeps::publish_only(deps.redis.clone());
    tokio::spawn(async move {
        let deps = ApprovalDeps::new(
            pg,
            realtime,
            std::sync::Arc::new(run_definition),
            std::sync::Arc::new(wall_ms),
        );
        announce_approval(&deps, &key).await;
    });
}

/// One tool call: `Ok(value)` is the JSON the model reads, `Err(kind)` is
/// the two failure flavors (a tool that ANSWERED with a failure vs a call
/// that threw).
pub enum CallOutcome {
    Ok(Value),
    Fail(String),
    Throw(String),
}

/// The non-'unknown agent' errors every verb shares, pre-shaped.
fn thrown(message: String) -> CallOutcome {
    CallOutcome::Throw(message)
}

async fn call_tool(
    deps: &WorkbenchDeps,
    subject: &AgentSubject,
    name: &str,
    args: &Map<String, Value>,
) -> CallOutcome {
    let pg = &deps.pg;
    let agent_model = subject_model(subject).to_string();
    let agent = match agent_by_model(pg, &agent_model).await {
        Ok(Some(agent)) => agent,
        Ok(None) => return CallOutcome::Fail("unknown agent".into()),
        Err(e) => return thrown(format!("agent read: {e}")),
    };
    let profile = match resolve_workbench(
        pg,
        &crate::workbench::WorkbenchAgent {
            department: &agent.department,
            role: agent.role.as_deref(),
            workbench: &agent.workbench,
            workbench_profile: agent.workbench_profile.as_deref(),
        },
    )
    .await
    {
        Ok(Some(profile)) => profile,
        Ok(None) => {
            return CallOutcome::Fail(
                "no workbench attached — an admin can enable one on your agent settings".into(),
            );
        }
        Err(e) => return thrown(e),
    };

    match name {
        "doctor" => {
            let registry = match list_harness_defs(pg).await {
                Ok(r) => r,
                Err(e) => return thrown(format!("harness registry read: {e}")),
            };
            let chosen_slug: Option<String> = match &agent.workbench_harness {
                Some(h) if profile.harnesses.iter().any(|s| s == h) => Some(h.clone()),
                _ => profile.harnesses.first().cloned(),
            };
            let h = registry
                .iter()
                .find(|x| Some(x.def.slug.as_str()) == chosen_slug.as_deref());
            let gh_status = github_status(pg, &deps.sb).await;
            let repos = gh::granted_repos(pg, &agent.id).await;
            let mut checks: Vec<String> = Vec::new();
            checks.push(format!(
                "profile: {} ({}) — attached",
                profile.name, profile.slug
            ));
            checks.push(match h {
                Some(h) => format!(
                    "harness: {} ({}, {}) — chosen",
                    h.def.label,
                    h.def.slug,
                    match h.source {
                        HarnessSource::Builtin => "builtin",
                        HarnessSource::Custom => "custom",
                    }
                ),
                None => format!(
                    "harness: \"{}\" NOT in the registry — pick another or ask an admin",
                    chosen_slug
                        .as_deref()
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| "undefined".into())
                ),
            });
            if let Some(h) = h {
                checks.push(match &h.def.auth {
                    crate::workbench::harnesses::HarnessAuth::Gateway => {
                        "auth: Talaria gateway (no key needed on your side)".into()
                    }
                    crate::workbench::harnesses::HarnessAuth::Provider { provider, env_var } => {
                        format!("auth: native {provider} key expected in {env_var}")
                    }
                });
            }
            checks.push(match gh_status.configured {
                true => format!(
                    "github: connected{}",
                    gh_status
                        .account
                        .as_deref()
                        .map(|a| format!(" as {a}"))
                        .unwrap_or_default()
                ),
                false => "github: NOT connected — jobs cannot start (an admin connects it in Admin → Org)"
                    .into(),
            });
            checks.push(if repos.is_empty() {
                "repos: none granted — ask an admin to grant repos on your agent settings".into()
            } else {
                format!("repos: {}", repos.join(", "))
            });
            let efforts = match effort_models(pg, agent.workbench_models.as_ref()).await {
                Ok(e) => Value::Object(e),
                Err(e) => return thrown(e),
            };
            CallOutcome::Ok(json!({
                "checks": checks,
                "harness": h.map(|h| json!({
                    "slug": h.def.slug,
                    "guide": h.def.guide,
                    "probe": h.def.probe,
                    "mcpTools": if h.def.mcp_serve.is_some() { "registered on your config when this harness is chosen" } else { "none — drive it via jsonRun" },
                    "passthroughConfig": h.def.mcp_config.as_ref().map(|m| format!("/opt/workbench-config/{}", m.filename)),
                })),
                "efforts": efforts,
                "workspaceRoot": "/opt/data/workbench/jobs/<jobId>",
                "sessionHistory": "/opt/data/workbench/harness (persistent, shared with your department)",
                "next": match h.and_then(|h| h.def.probe.clone()) {
                    Some(probe) => format!("Run the probe in your shell to verify the harness binary: {probe}"),
                    None => "No probe declared — try the harness directly on your first job.".into(),
                },
            }))
        }

        "list_repos" => {
            let repos = gh::granted_repos(pg, &agent.id).await;
            let efforts = match effort_models(pg, agent.workbench_models.as_ref()).await {
                Ok(e) => Value::Object(e),
                Err(e) => return thrown(e),
            };
            CallOutcome::Ok(json!({
                "repos": repos,
                "efforts": efforts,
                "note": "Pick effort by the work, not the model: light = quick fixes, standard = regular features (plan required), heavy = hard cross-cutting work (plan required, used sparingly).",
            }))
        }

        "start_job" => {
            let repo = arg_str(args, "repo");
            let effort = match args.get("effort").and_then(Value::as_str) {
                Some(e) if e == "light" || e == "heavy" => e.to_string(),
                _ => "standard".to_string(),
            };
            let plan = truncate_utf16(&arg_str(args, "plan"), 20_000).to_string();
            if !gh::granted_repos(pg, &agent.id)
                .await
                .iter()
                .any(|r| r == &repo)
            {
                return CallOutcome::Fail(format!(
                    "repo \"{repo}\" is not granted to you — list_repos shows yours"
                ));
            }
            if effort != "light" && plan.trim().is_empty() {
                return CallOutcome::Fail(
                    "standard/heavy work requires a plan first — describe approach, files, and test strategy in `plan`"
                        .into(),
                );
            }
            // BEFORE anything reads the ticket: the caller supplied this id,
            // so board policy and the closed-ticket rule decide whether it
            // may be touched at all. Everything below (ref + title in the
            // branch name, the plan comment and artifact chip on the ticket,
            // the audit line, and finish_job's public PR title) depends on
            // this having passed — which is why the id arrives ALREADY gated
            // rather than gated on the next line.
            let task_id = match ticket_arg(pg, subject, args).await {
                Ok(id) => id,
                Err(error) => return CallOutcome::Fail(error),
            };
            // One live job per ticket keeps branches 1:1 with work.
            if let Some(task_id) = &task_id {
                let dup = sqlx::query_scalar::<_, i32>(
                    "select 1 from workbench_jobs where task_id = $1::uuid and status = 'started' limit 1",
                )
                .bind(task_id)
                .fetch_optional(pg)
                .await;
                if matches!(dup, Ok(Some(_))) {
                    return CallOutcome::Fail(
                        "a job is already running for this ticket — job_status shows it; finish or abandon it first"
                            .into(),
                    );
                }
            }
            let (ticket_ref, title) = match &task_id {
                Some(task_id) => match ticket_ref_of(pg, task_id).await {
                    Some((r, t)) => (r.unwrap_or_default(), t),
                    None => (String::new(), String::new()),
                },
                None => (String::new(), String::new()),
            };
            let branch = if !ticket_ref.is_empty() {
                truncate_utf16(
                    &format!(
                        "talaria/{}-{}",
                        ticket_ref.to_lowercase(),
                        if slugify(&title).is_empty() {
                            "work".into()
                        } else {
                            slugify(&title)
                        }
                    ),
                    80,
                )
                .to_string()
            } else {
                truncate_utf16(
                    &format!("talaria/job-{}-{}", slugify(&repo), random_base36(6)),
                    80,
                )
                .to_string()
            };
            let created_branch = match gh::create_branch(pg, &deps.sb, &repo, &branch, None).await {
                Ok(cb) => cb,
                Err(e) => return thrown(e),
            };
            // Plan-first, gated by effort: light auto-proceeds; standard
            // proceeds with the plan posted to the ticket (audit trail);
            // heavy WAITS for a human to approve the plan from the ticket
            // before any clone URL exists.
            let gated = effort == "heavy" && task_id.is_some();
            let status = if gated {
                "awaiting_approval"
            } else {
                "started"
            };
            let job_row: Result<JobRow, sqlx::Error> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "insert into workbench_jobs (agent_id, agent_model, task_id, repo, branch, effort, plan, status) \
                 values ($1::uuid, $2, $3::uuid, $4, $5, $6, $7, $8) returning {JOB_COLS}"
            )))
            .bind(&agent.id)
            .bind(&agent.model)
            .bind(&task_id)
            .bind(&repo)
            .bind(&branch)
            .bind(&effort)
            .bind(&plan)
            .bind(status)
            .fetch_one(pg)
            .await;
            let job = match job_row {
                Ok(row) => WorkbenchJob::of(row),
                Err(e) => return thrown(format!("job write: {e}")),
            };
            // `gated` means this agent has just stopped and will not move
            // again until a person approves the plan from the ticket. That is
            // an approval the instant the row lands, so it is announced now
            // rather than found by the approval sweep up to five minutes
            // later.
            if gated {
                announce_raised(deps, format!("workbench_plan:{}", job.id));
            }
            if let Some(task_id) = &task_id
                && !plan.trim().is_empty()
            {
                // Through `add_comment`, not `insert into task_comments`: it
                // also lands in the activity feed and pushes the board, which
                // a raw insert silently did not.
                let _ = add_comment(
                    &deps.task_deps(),
                    task_id,
                    &agent.model,
                    &format!("**Workbench plan** ({effort} effort · {repo}):\n\n{plan}"),
                    None,
                )
                .await;
                // The plan also becomes a markdown ARTIFACT attached to the
                // ticket — durable and versioned, not just scrollback. Filed
                // under the agent's Plans cabinet; org-visible like the
                // ticket it belongs to.
                let spawn_deps = deps.clone();
                let spawn_agent = agent.clone();
                let spawn_task_id = task_id.clone();
                let spawn_ref = ticket_ref.clone();
                let spawn_title = title.clone();
                let spawn_repo = repo.clone();
                let spawn_effort = effort.clone();
                let spawn_plan = plan.clone();
                tokio::spawn(async move {
                    let label = if !spawn_agent.display_name.is_empty() {
                        spawn_agent.display_name.clone()
                    } else {
                        describe_agent(&spawn_agent.model).label
                    };
                    let folder =
                        agent_category_folder(&spawn_deps.pg, &label, "Plans", &spawn_agent.model)
                            .await;
                    let artifact_title = truncate_utf16(
                        &format!(
                            "Plan — {}",
                            if !spawn_ref.is_empty() {
                                spawn_ref.clone()
                            } else if !spawn_title.is_empty() {
                                spawn_title.clone()
                            } else {
                                spawn_repo.clone()
                            }
                        ),
                        120,
                    )
                    .to_string();
                    let Ok(artifact) = create_artifact(
                        &spawn_deps.pg,
                        Some("doc"),
                        Some(&artifact_title),
                        &spawn_agent.model,
                        None,
                        folder.as_deref(),
                    )
                    .await
                    else {
                        return;
                    };
                    let body = format!(
                        "# Workbench plan — {}{}{}\n\n_{} effort · {} · by {}_\n\n{}",
                        if !spawn_ref.is_empty() {
                            &spawn_ref
                        } else {
                            ""
                        },
                        if !spawn_ref.is_empty() { " · " } else { "" },
                        if !spawn_title.is_empty() {
                            &spawn_title
                        } else {
                            &spawn_repo
                        },
                        spawn_effort,
                        spawn_repo,
                        label,
                        spawn_plan
                    );
                    let _ = save_artifact(
                        &spawn_deps.pg,
                        &artifact.id,
                        SaveArtifactPatch {
                            body: Some(&body),
                            ..Default::default()
                        },
                        &spawn_agent.model,
                    )
                    .await;
                    let chip = json!({
                        "id": artifact.id,
                        "filename": if artifact.title.is_empty() { "Plan".to_string() } else { artifact.title.clone() },
                        "mime": "ref/artifact",
                        "size": 0,
                        "refType": "artifact",
                    });
                    // Through `update_task` as the AGENT, never `update
                    // tasks set …`: raw SQL here would be an agent-reachable
                    // write to `tasks` that skips the human-in-the-loop
                    // invariant entirely. The agent actor also gets the
                    // attachment activity line and the board push for free.
                    if let Ok(Some(cur)) = get_task(&spawn_deps.pg, &spawn_task_id).await {
                        let have = cur.attachments.as_array().cloned().unwrap_or_default();
                        if !have.iter().any(|a| {
                            a.get("id").and_then(Value::as_str) == Some(artifact.id.as_str())
                        }) {
                            let mut next = have.clone();
                            next.push(chip);
                            let _ = update_task(
                                &spawn_deps.task_deps(),
                                &spawn_task_id,
                                TaskPatch {
                                    attachments: Some(Value::Array(next)),
                                    ..Default::default()
                                },
                                &TaskActor::agent(spawn_agent.model.clone()),
                            )
                            .await;
                        }
                    }
                });
            }
            log_ticket(
                pg,
                task_id.as_deref(),
                &WorkbenchActor::Agent(subject.clone()),
                &if gated {
                    format!("workbench job awaiting plan approval: {repo} @ {branch} (heavy)")
                } else {
                    format!(
                        "workbench job started: {repo} @ {branch} ({effort}){}",
                        if plan.is_empty() {
                            ""
                        } else {
                            " — plan recorded"
                        }
                    )
                },
            )
            .await;
            // Effort → model is Talaria's call: the agent picked the effort,
            // the platform resolves which model that means today. Invocation
            // hints come from the profile's harness adapters with the model
            // slotted in.
            let model = match effort_model(pg, &effort, agent.workbench_models.as_ref()).await {
                Ok(m) => m,
                Err(e) => return thrown(e),
            };
            // The agent's chosen harness leads (falling back to the profile's
            // first); its invocation line carries the model in the harness's
            // own syntax, so it's directly runnable.
            let chosen_slug: Option<String> = match &agent.workbench_harness {
                Some(h) if profile.harnesses.iter().any(|s| s == h) => Some(h.clone()),
                _ => profile.harnesses.first().cloned(),
            };
            let registry = match list_harness_defs(pg).await {
                Ok(r) => r,
                Err(e) => return thrown(format!("harness registry read: {e}")),
            };
            let mut found: Vec<&crate::workbench::harnesses::ResolvedHarness> = profile
                .harnesses
                .iter()
                .filter_map(|slug| registry.iter().find(|h| h.def.slug == *slug))
                .collect();
            found.sort_by(|a, b| {
                let chosen = chosen_slug.as_deref();
                if Some(a.def.slug.as_str()) == chosen {
                    std::cmp::Ordering::Less
                } else if Some(b.def.slug.as_str()) == chosen {
                    std::cmp::Ordering::Greater
                } else {
                    std::cmp::Ordering::Equal
                }
            });
            let harnesses: Vec<Value> = found
                .iter()
                .map(|h| {
                    let mut entry = Map::new();
                    entry.insert("harness".into(), json!(h.def.slug));
                    entry.insert(
                        "chosen".into(),
                        json!(Some(h.def.slug.as_str()) == chosen_slug.as_deref()),
                    );
                    entry.insert(
                        "run".into(),
                        json!(match &model {
                            Some(m) => h
                                .def
                                .invoke
                                .replace("<model>", &harness_model_arg(&h.def, m)),
                            None => h.def.invoke.clone(),
                        }),
                    );
                    if let Some(json_invoke) = &h.def.json_invoke {
                        entry.insert(
                            "jsonRun".into(),
                            json!(match &model {
                                Some(m) =>
                                    json_invoke.replace("<model>", &harness_model_arg(&h.def, m)),
                                None => json_invoke.clone(),
                            }),
                        );
                    }
                    if Some(h.def.slug.as_str()) == chosen_slug.as_deref() {
                        entry.insert("guide".into(), json!(h.def.guide));
                    }
                    Value::Object(entry)
                })
                .collect();
            // One WORKSPACE per job — concurrent jobs never collide — under
            // the persistent volume, so clones and harness sessions survive
            // restarts.
            let workdir = format!("/opt/data/workbench/jobs/{}", job.id);
            let clone = if gated {
                Value::Null
            } else {
                match gh::clone_url(pg, &deps.sb, &repo).await {
                    Ok(url) => url.map(Value::String).unwrap_or(Value::Null),
                    Err(e) => return thrown(e),
                }
            };
            let rules = if gated {
                "Heavy work waits for a human: your plan is on the ticket for approval. Poll job_status — once approved it returns the clone URL. Do NOT begin building."
                    .to_string()
            } else {
                format!(
                    "Clone with the URL above INTO your workdir (mkdir -p {workdir} first). It carries no credential and needs none — your sandbox's git asks Talaria for one when it pushes, so never add a token to a remote URL. One workspace per job: never work outside it, so concurrent jobs stay isolated. Your harness's session history persists under /opt/data/workbench/harness and is shared with your department — resume prior sessions or pick up a teammate's hand-off from there. Work ONLY on {branch}; commit and push to it as you go — your sandbox is preconfigured so commits are authored as YOU (do not override git identity). Never touch {base} directly. Use your CHOSEN harness (first in the list, marked chosen) with the {effort}-effort model shown — via its MCP tools if registered on your config, else its jsonRun form; read structured results, never scrape raw logs. Escalate effort only when the work truly needs it. When done, call finish_job — Talaria opens the PR.",
                    base = created_branch.base,
                )
            };
            let mut value = Map::new();
            value.insert("jobId".into(), json!(job.id));
            value.insert("repo".into(), json!(repo));
            value.insert("branch".into(), json!(branch));
            value.insert("base".into(), json!(created_branch.base));
            value.insert("resumed".into(), json!(!created_branch.created));
            value.insert("status".into(), json!(job.status));
            value.insert("workdir".into(), json!(workdir));
            if !gated {
                value.insert("cloneUrl".into(), clone);
            }
            value.insert("effort".into(), json!(effort));
            value.insert(
                "model".into(),
                model.map(Value::String).unwrap_or(Value::Null),
            );
            value.insert("harnesses".into(), Value::Array(harnesses));
            value.insert("rules".into(), json!(rules));
            CallOutcome::Ok(Value::Object(value))
        }

        "job_status" => {
            let job_id = args
                .get("jobId")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty());
            let rows: Result<Vec<JobRow>, sqlx::Error> = match job_id {
                Some(job_id) => sqlx::query_as(sqlx::AssertSqlSafe(format!(
                    "select {JOB_COLS} from workbench_jobs where id = $1::uuid and agent_id = $2::uuid"
                )))
                .bind(job_id)
                .bind(&agent.id)
                .fetch_all(pg)
                .await,
                None => sqlx::query_as(sqlx::AssertSqlSafe(format!(
                    "select {JOB_COLS} from workbench_jobs where agent_id = $1::uuid \
                     order by created_at desc limit 10"
                )))
                .bind(&agent.id)
                .fetch_all(pg)
                .await,
            };
            let rows = match rows {
                Ok(rows) => rows,
                Err(e) => return thrown(format!("job read: {e}")),
            };
            // Running jobs get a FRESH clone URL each poll (app tokens expire
            // ~1h); gated jobs stay locked until a human approves from the
            // ticket.
            let mut jobs: Vec<Value> = Vec::with_capacity(rows.len());
            for row in rows {
                let job = WorkbenchJob::of(row);
                let mut wire = job.wire();
                if job.status == "started" {
                    let clone = match gh::clone_url(pg, &deps.sb, &job.repo).await {
                        Ok(url) => url.map(Value::String).unwrap_or(Value::Null),
                        Err(e) => return thrown(e),
                    };
                    let obj = wire.as_object_mut().expect("wire is an object");
                    obj.insert("cloneUrl".into(), clone);
                    obj.insert(
                        "workdir".into(),
                        json!(format!("/opt/data/workbench/jobs/{}", job.id)),
                    );
                }
                jobs.push(wire);
            }
            CallOutcome::Ok(json!({ "jobs": jobs }))
        }

        "merge_to_testing" => {
            let job_id = arg_str(args, "jobId");
            let rows: Vec<JobRow> = match sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "select {JOB_COLS} from workbench_jobs where id = $1::uuid and agent_id = $2::uuid"
            )))
            .bind(&job_id)
            .bind(&agent.id)
            .fetch_all(pg)
            .await
            {
                Ok(rows) => rows,
                Err(e) => return thrown(format!("job read: {e}")),
            };
            let Some(row) = rows.into_iter().next() else {
                return CallOutcome::Fail("unknown job".into());
            };
            let job = WorkbenchJob::of(row);
            let r = merge_job_to_testing(
                deps,
                &MergeJob {
                    id: job.id.clone(),
                    repo: job.repo.clone(),
                    branch: job.branch.clone(),
                    status: job.status.clone(),
                    task_id: job.task_id.clone(),
                },
                &WorkbenchActor::Agent(subject.clone()),
            )
            .await;
            match r {
                Ok(testing_branch) => CallOutcome::Ok(json!({
                    "merged": true,
                    "testingBranch": testing_branch,
                    "note": "Testing merge only — the PR still ships through review.",
                })),
                Err(MergeJobError::Fail(error)) => CallOutcome::Fail(error),
                Err(MergeJobError::Throw(message)) => CallOutcome::Throw(message),
            }
        }

        "request_repo" => {
            let cfg = gh::get_github_config(pg).await;
            let org = arg_str(args, "org").trim().to_string();
            let repo_name = sanitize_repo_name(&arg_str(args, "name"));
            if cfg.repo_creation_orgs.is_empty() {
                return CallOutcome::Fail(
                    "repo creation is not enabled — an admin can approve orgs on the GitHub panel"
                        .into(),
                );
            }
            if !cfg.repo_creation_orgs.iter().any(|o| o == &org) {
                return CallOutcome::Fail(format!(
                    "org \"{org}\" is not approved for repo creation (approved: {})",
                    cfg.repo_creation_orgs.join(", ")
                ));
            }
            if repo_name.is_empty() {
                return CallOutcome::Fail("name required".into());
            }
            // Same door as start_job — literally the same function, not the
            // same three lines again: this taskId is stored on the request
            // row and gets a workbench audit line, so it is authorised rather
            // than believed.
            let task_id = match ticket_arg(pg, subject, args).await {
                Ok(id) => id,
                Err(error) => return CallOutcome::Fail(error),
            };
            let dup = sqlx::query_scalar::<_, i32>(
                "select 1 from workbench_repo_requests \
                 where agent_id = $1::uuid and org = $2 and name = $3 and status = 'pending' limit 1",
            )
            .bind(&agent.id)
            .bind(&org)
            .bind(&repo_name)
            .fetch_optional(pg)
            .await;
            if matches!(dup, Ok(Some(_))) {
                return CallOutcome::Fail(
                    "you already have a pending request for this repo — a human will decide".into(),
                );
            }
            let filed: Result<String, sqlx::Error> = sqlx::query_scalar(
                "insert into workbench_repo_requests \
                   (agent_id, agent_model, org, name, description, why, task_id) \
                 values ($1::uuid, $2, $3, $4, $5, $6, $7::uuid) returning id::text",
            )
            .bind(&agent.id)
            .bind(&agent.model)
            .bind(&org)
            .bind(&repo_name)
            .bind(truncate_utf16(&arg_str(args, "description"), 300))
            .bind(truncate_utf16(&arg_str(args, "why"), 1000))
            .bind(&task_id)
            .fetch_one(pg)
            .await;
            let filed = match filed {
                Ok(id) => id,
                Err(e) => return thrown(format!("repo request write: {e}")),
            };
            log_ticket(
                pg,
                task_id.as_deref(),
                &WorkbenchActor::Agent(subject.clone()),
                &format!("requested a new repo: {org}/{repo_name} — awaiting human approval"),
            )
            .await;
            // Humans hear about it once, through the one announcer — which
            // asks the resolver who may read the agent's `why` and who may
            // only be told that a request is waiting. See `announce_raised`.
            announce_raised(deps, format!("repo_request:{filed}"));
            CallOutcome::Ok(json!({
                "status": "pending",
                "note": "Request filed — a human decides. Continue other work; the repo appears in list_repos if approved.",
            }))
        }

        "finish_job" => {
            let job_id = arg_str(args, "jobId");
            let rows: Vec<JobRow> = match sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "select {JOB_COLS} from workbench_jobs where id = $1::uuid and agent_id = $2::uuid"
            )))
            .bind(&job_id)
            .bind(&agent.id)
            .fetch_all(pg)
            .await
            {
                Ok(rows) => rows,
                Err(e) => return thrown(format!("job read: {e}")),
            };
            let Some(row) = rows.into_iter().next() else {
                return CallOutcome::Fail("unknown job".into());
            };
            let job = WorkbenchJob::of(row);
            // Abandon works from ANY live state — including a still-gated
            // plan. The job row is this agent's own (the select filters on
            // agent_id), so the state change needs no ticket permission; the
            // AUDIT LINE does, and log_ticket asks for it. Abandoning is the
            // one branch gated by nothing at all — the audit line through
            // `log_ticket` is the check it cannot skip.
            if args.get("abandon") == Some(&Value::Bool(true))
                && (job.status == "started" || job.status == "awaiting_approval")
            {
                if let Err(e) = sqlx::query(
                    "update workbench_jobs set status = 'abandoned', updated_at = now() where id = $1::uuid",
                )
                .bind(&job.id)
                .execute(pg)
                .await
                {
                    return thrown(format!("job update: {e}"));
                }
                log_ticket(
                    pg,
                    job.task_id.as_deref(),
                    &WorkbenchActor::Agent(subject.clone()),
                    &format!("workbench job abandoned: {} @ {}", job.repo, job.branch),
                )
                .await;
                return CallOutcome::Ok(json!({ "status": "abandoned" }));
            }
            if job.status == "awaiting_approval" {
                return CallOutcome::Fail(
                    "the plan has not been approved yet — poll job_status".into(),
                );
            }
            if job.status != "started" {
                return CallOutcome::Fail(format!("job is already {}", job.status));
            }
            let summary = truncate_utf16(&arg_str(args, "summary"), 20_000).to_string();
            let base = match gh::effective_base(pg, &deps.sb, &job.repo).await {
                Ok(base) => base,
                Err(e) => return thrown(e),
            };
            let ahead = match gh::branch_ahead(pg, &deps.sb, &job.repo, &base, &job.branch).await {
                Ok(ahead) => ahead,
                Err(e) => return thrown(e),
            };
            if ahead == 0 {
                return CallOutcome::Fail(
                    "the branch has no commits yet — push your work first (or finish with abandon:true)"
                        .into(),
                );
            }
            // The ticket ref and TITLE go into a PUBLIC PR title and body, so
            // the board check is re-run at the disclosure point rather than
            // trusted from start_job: job rows written before that gate
            // existed (or whose board grant was revoked since) must not
            // publish a ticket the agent may no longer see. Losing the link
            // degrades the PR; it never blocks the work.
            let ticket_ok = match &job.task_id {
                Some(task_id) => ticket_still_ours(pg, subject, task_id).await,
                None => false,
            };
            let mut ticket_line = String::new();
            let mut ticket: Option<(Option<String>, String)> = None;
            if let Some(task_id) = &job.task_id
                && ticket_ok
                && let Some(found) = ticket_ref_of(pg, task_id).await
            {
                ticket = Some(found.clone());
                ticket_line = format!(
                    "Ticket: {} — {}\n\n",
                    found.0.as_deref().unwrap_or(task_id),
                    found.1
                );
            }
            let body = format!(
                "{ticket_line}{}{}🔧 Opened by **{}** (`{}`) via the Talaria workbench ({} effort). Commits on this branch are authored by the agent.",
                if summary.is_empty() {
                    "(no summary provided)".to_string()
                } else {
                    summary.clone()
                },
                if job.plan.is_empty() {
                    String::new()
                } else {
                    format!("\n\n## Plan\n\n{}", job.plan)
                },
                agent.display_name,
                agent.model,
                job.effort,
            );
            let title = match &ticket {
                Some((ticket_ref, title)) => truncate_utf16(
                    &format!(
                        "{}{}",
                        ticket_ref
                            .as_deref()
                            .map(|r| format!("[{r}] "))
                            .unwrap_or_default(),
                        title
                    ),
                    100,
                )
                .to_string(),
                None => format!("Workbench: {}", job.branch),
            };
            let pr = match gh::create_pull_request(
                pg,
                &deps.sb,
                &job.repo,
                &job.branch,
                &base,
                &title,
                &body,
                false,
            )
            .await
            {
                Ok(pr) => pr,
                Err(e) => return thrown(e),
            };
            if let Err(e) = sqlx::query(
                "update workbench_jobs set status = 'pr_open', pr_url = $2, summary = $3, \
                 updated_at = now() where id = $1::uuid",
            )
            .bind(&job.id)
            .bind(&pr.url)
            .bind(&summary)
            .execute(pg)
            .await
            {
                return thrown(format!("job update: {e}"));
            }
            // `ticket_ok` above answered the DISCLOSURE question (may this
            // ref and title go into a public PR?). The audit line asks a
            // different one — may this agent still WRITE to the ticket? — so
            // it is not re-derived here: log_ticket asks the gate. A ticket
            // closed while the job ran therefore gets no line, which is the
            // invariant working: the PR still opened and its URL is in the
            // result the agent gets back.
            log_ticket(
                pg,
                job.task_id.as_deref(),
                &WorkbenchActor::Agent(subject.clone()),
                &format!("workbench PR opened: {}", pr.url),
            )
            .await;
            CallOutcome::Ok(json!({
                "prUrl": pr.url,
                "prNumber": pr.number,
                "note": "Include this PR link in your outcome report.",
            }))
        }

        _ => CallOutcome::Fail(format!("unknown tool \"{name}\"")),
    }
}

/// `String(args.x ?? '')` — anything non-string reads as the empty string.
fn arg_str(args: &Map<String, Value>, key: &str) -> String {
    args.get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

/// `String(args.name ?? '').trim().toLowerCase().replace(/[^a-z0-9._-]+/g,
/// '-').slice(0, 100)` — the repo-name normalizer.
fn sanitize_repo_name(v: &str) -> String {
    let lower = v.trim().to_lowercase();
    let collapsed = regex::Regex::new("[^a-z0-9._-]+")
        .expect("repo name collapse pattern")
        .replace_all(&lower, "-");
    truncate_utf16(&collapsed, 100).to_string()
}

/// `Math.random().toString(36).slice(2, 8)` — six base36 chars for a
/// ticketless branch mint. Shape-matched, not distribution-matched; the
/// branch name carries no authority.
fn random_base36(len: usize) -> String {
    const ALPHABET: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut bytes = vec![0u8; len];
    getrandom::fill(&mut bytes).expect("system rng");
    bytes
        .iter()
        .map(|b| ALPHABET[(*b as usize) % ALPHABET.len()] as char)
        .collect()
}

/// The job slice `merge_job_to_testing` reads — the agent verb passes a fresh
/// row; the ticket strip passes its own select's row.
pub struct MergeJob {
    pub id: String,
    pub repo: String,
    pub branch: String,
    pub status: String,
    pub task_id: Option<String>,
}

/// Why a testing merge did not happen. The two failure flavors, kept apart
/// because the two CALLERS answer them differently: `Fail` is a tool failure —
/// a sentence for the operator/agent (`{ ok: false, error }` through the
/// route, `Error: …` through the verb); `Throw` is the infra/REST failure,
/// which propagates rather than answers (rpc `error:` for the verb, a 500
/// for the route).
pub enum MergeJobError {
    Fail(String),
    Throw(String),
}

/// Merge a job's branch into the repo's testing branch — ONE implementation
/// for both the agent verb and the human ticket-strip action, which is
/// exactly why `by` is a WorkbenchActor and not an actor string: the two
/// callers are not the same kind of writer, and the audit line it emits has
/// to know. The human route passes the user's email and is unaffected; the
/// agent verb passes the subject and is gated. The merge itself is a GitHub
/// operation and proceeds either way.
pub async fn merge_job_to_testing(
    deps: &WorkbenchDeps,
    job: &MergeJob,
    by: &WorkbenchActor,
) -> Result<String, MergeJobError> {
    use MergeJobError::*;
    if job.status != "started" && job.status != "pr_open" {
        return Err(Fail(format!("job is {}", job.status)));
    }
    let flow = gh::repo_flow(&deps.pg, &job.repo).await.map_err(Throw)?;
    let Some(testing_branch) = flow.testing_branch else {
        return Err(Fail(
            "this repo has no testing branch configured — an admin can set one on the GitHub panel"
                .into(),
        ));
    };
    let r = gh::merge_into(&deps.pg, &deps.sb, &job.repo, &testing_branch, &job.branch)
        .await
        .map_err(Throw)?;
    if !r.merged {
        return Err(Fail(
            r.reason.clone().unwrap_or_else(|| "merge failed".into()),
        ));
    }
    sqlx::query(
        "update workbench_jobs set merged_testing_at = now(), updated_at = now() where id = $1::uuid",
    )
    .bind(&job.id)
    .execute(&deps.pg)
    .await
    .map_err(|e| Throw(format!("job update: {e}")))?;
    log_ticket(
        &deps.pg,
        job.task_id.as_deref(),
        by,
        &format!(
            "workbench: merged {} into {} for testing{}",
            job.branch,
            testing_branch,
            r.reason
                .as_deref()
                .map(|reason| format!(" ({reason})"))
                .unwrap_or_default()
        ),
    )
    .await;
    Ok(testing_branch)
}

// ── JSON-RPC surface (the shared dispatcher; this surface's tools + call) ────

/// The in-process dispatcher the MCP gateway hands `talaria-workbench://`
/// requests to. `allowed` arrives ALREADY filtered to what the caller may
/// use (the gateway's resolution, enforced again here); the tools answer
/// tools/list as-is (a tool is its own listed entry).
pub async fn dispatch_workbench_mcp(
    deps: &WorkbenchDeps,
    rpc: &Value,
    agent: &AgentSubject,
    allowed: Option<&[String]>,
) -> (StatusCode, Option<Value>) {
    let tools: Vec<ListedTool> = workbench_tools()
        .into_iter()
        .filter(|t| {
            allowed
                .map(|a| {
                    a.iter()
                        .any(|name| Some(name.as_str()) == t["name"].as_str())
                })
                .unwrap_or(true)
        })
        .map(|t| ListedTool {
            name: t["name"].as_str().unwrap_or_default().to_string(),
            entry: t,
        })
        .collect();
    let deps = deps.clone();
    let agent = agent.clone();
    let call = Box::new(move |name: &str, args: Map<String, Value>| {
        let deps = deps.clone();
        let agent = agent.clone();
        let name = name.to_string();
        Box::pin(async move {
            match call_tool(&deps, &agent, &name, &args).await {
                CallOutcome::Ok(value) => ToolOutcome::Ok(value),
                CallOutcome::Fail(error) => ToolOutcome::Fail(error),
                CallOutcome::Throw(message) => ToolOutcome::Throw(message),
            }
        }) as std::pin::Pin<Box<dyn std::future::Future<Output = ToolOutcome> + Send>>
    });
    dispatch_jsonrpc(rpc, tools, "talaria-workbench", call).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_matches_the_ts_normalizer() {
        assert_eq!(slugify("  Fix the Login Bug!! "), "fix-the-login-bug");
        assert_eq!(slugify("a---b"), "a-b");
        assert_eq!(slugify("--lead--"), "lead");
        assert_eq!(slugify(""), "");
        let long = "x".repeat(120);
        assert_eq!(slugify(&long).len(), 40);
    }

    #[test]
    fn repo_names_collapse_dots_and_dashes_kept() {
        // No dash-trim: an edge run of junk leaves an edge dash
        // ("my repo!" → "my-repo-").
        assert_eq!(sanitize_repo_name("  My Repo! "), "my-repo-");
        // The class keeps dots, underscores, and dashes as-is.
        assert_eq!(sanitize_repo_name("a.b_c-d"), "a.b_c-d");
        assert_eq!(sanitize_repo_name(""), "");
    }

    #[test]
    fn the_tools_spell_the_ts_key_order() {
        let tools = workbench_tools();
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(
            names,
            vec![
                "doctor",
                "list_repos",
                "start_job",
                "job_status",
                "merge_to_testing",
                "request_repo",
                "finish_job"
            ]
        );
        // start_job's properties ride in declaration order, required last.
        let props: Vec<&str> = tools[2]["inputSchema"]["properties"]
            .as_object()
            .unwrap()
            .keys()
            .map(|s| s.as_str())
            .collect();
        assert_eq!(props, vec!["taskId", "repo", "effort", "plan"]);
        assert_eq!(tools[2]["inputSchema"]["required"], json!(["repo"]));
        // request_repo's required set, in declaration order.
        assert_eq!(
            tools[5]["inputSchema"]["required"],
            json!(["org", "name", "why"])
        );
    }

    #[test]
    fn the_job_wire_spells_the_row_order_and_drops_no_nulls() {
        let job = WorkbenchJob {
            id: "j".into(),
            agent_id: "a".into(),
            agent_model: "m".into(),
            task_id: None,
            repo: "o/r".into(),
            branch: "b".into(),
            effort: "standard".into(),
            plan: "p".into(),
            status: "started".into(),
            pr_url: None,
            summary: String::new(),
            created_at: "2026-01-01T00:00:00.000Z".into(),
            updated_at: "2026-01-01T00:00:00.000Z".into(),
        };
        let wire = job.wire();
        let keys: Vec<&str> = wire
            .as_object()
            .unwrap()
            .keys()
            .map(|s| s.as_str())
            .collect();
        // JOB_COLS' order minus `plan` — job_status never ships the plan,
        // on any status.
        assert_eq!(
            keys,
            vec![
                "id",
                "agentId",
                "agentModel",
                "taskId",
                "repo",
                "branch",
                "effort",
                "status",
                "prUrl",
                "summary",
                "createdAt",
                "updatedAt"
            ]
        );
        assert_eq!(wire["taskId"], Value::Null);
        assert!(wire.as_object().unwrap().get("plan").is_none());
    }
}
