// RESEARCH, AS A DURABLE RUN — a pipeline that must not die with its process.
//
// WHY A RUN AT ALL. An in-process pipeline answers "the app restarted while
// I was researching" with `set status = 'error', error = 'run went stale'`
// forty-five minutes after the fact, to a person who had asked a question and
// waited. Nothing was lost when that process died — every search that had come
// back was in memory and would have been in the report — but nothing had ever
// been WRITTEN DOWN in a form another process could pick up.
//
// This file is that cursor. The run is a sequence of small steps over a
// checkpoint that carries the resume position AND the work so far — the round,
// how many of the round's queries have come back, every finding, and the whole
// citation registry — so a driver on any instance can re-enter the loop at the
// exact query that had not run yet. `runs/reclaim.rs` is what re-enters it.
//
// ── THE STEPS, and why they are this small ──────────────────────────────────
//
//   begin       write the domain row if it is missing, resolve the search model
//               and the depth budget. NO billed call.
//   plan        ONE persona call: this round's queries (recon skips it — its
//               single query is the question).
//   search      ONE sonar call: one query, its findings, its sources.
//   synthesize  ONE persona call: the report.
//   artifact    create the report artifact and LINK it to the run, so a
//               re-entry can find the one it already made.
//   save        write the body, the members' editor grants, the citation rows
//               and the domain record's terminal fields. Every one idempotent.
//   publish     index the report, notify the owner, done.
//
// ONE BILLED CALL PER STEP, CHECKPOINTED IMMEDIATELY AFTER, is the whole reason
// the steps are cut here and not somewhere more convenient. The runtime is
// AT-LEAST-ONCE (runs/define.rs states the seven ways), and research is the
// place where a lost checkpoint is expensive rather than theoretical: a search
// stage that ran and did not checkpoint is PAID FOR A SECOND TIME on resume.
//
// WHAT A RE-ENTRY REPEATS, IN THE WORST CASE, per step: begin nothing (`on
// conflict do nothing`); plan one planner call, re-planned rather than
// duplicated; search ONE sonar call, re-billed, for the one query in flight;
// synthesize one synthesis call, re-billed, nothing downstream yet; artifact a
// SECOND artifact only if the process dies between the create and the link,
// which is one statement apart (the loser is an empty untitled doc); save
// nothing that is not keyed (saveArtifact overwrites the same id, citation rows
// are `on conflict do nothing`); publish the index is content-hashed so a
// repeat is a no-op — the NOTIFICATION is the one repeat this file cannot close
// from here, which is why `publish` is the last step and does nothing after it.
//
// THE OTHER HAZARDS, answered: abandoned-but-running — every step calls
// `stop_if_abandoned` before each outward call (risk 2); the run never messages
// anybody to ask anything — the no-sources park was removed 2026-08-28 (ticket
// #5: two runs sat 'awaiting' for hours reading as working ones), it retries by
// itself and then fails with a sentence in `error` (risk 3, and 4/5 are moot
// with nobody asked); `start_research` passes the research record's own uuid as
// the run id, so one research row can only ever have one run (risk 6); the tail
// is publish's (risk 7).
//
// TESTABILITY IS A DESIGN CONSTRAINT: every edge — the models, the artifacts,
// the domain rows, the index, the bell — is a field on `ResearchRunDeps`, so
// the tests below drive whole runs with no database, no Redis and no gateway.
//
// CANCELLATION IS STEP-BOUNDARY ONLY. `run_harness` takes no abort signal, so
// cancellation here is `stop_if_abandoned` before each outward call, plus the
// driver's abandon-by-rejection. An in-flight call is bounded by the gateway's
// own ten-minute timeout instead. The abandoned copy still stops SPENDING at
// the next boundary; what it cannot do is tear down a request already on the
// wire.

use std::collections::HashSet;
use std::sync::{Arc, LazyLock, OnceLock};

use futures_util::future::BoxFuture;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgPool;

use crate::artifacts::{
    SaveArtifactPatch, agent_category_folder, attach_artifact, create_artifact, save_artifact,
};
use crate::capability_reach::{ReachVia, Supplier, reach_for_keys};
use crate::fleet::describe_agent;
use crate::harness::defs::research::{
    ResearchDepth, SearchSink, SynthSource, SynthesisInput, ToolSearchDeps, clamp_queries,
    queries_from_lines, queries_harness, search_harness, search_transport, synthesis_harness,
    tool_search_transport,
};
use crate::harness::run::{
    RunContext, RunLedger, capability_keys_for, real_deps as harness_real_deps, run_harness,
};
use crate::harness::transport::LedgerSource;
use crate::kb::perms::{EditorGrant, set_editors};
use crate::model::access::gateway_models;
use crate::model::roles::resolve_role_model;
use crate::notify::{NotificationInput, NotifyDeps, add_notification};
use crate::retrieval::index::IndexDoc;
use crate::retrieval::sources::{index_activity, index_personal};
use crate::retrieval::{embed, qdrant};
use crate::runs::define::{
    Authority, DEFAULT_MAX_ATTEMPTS, RunDefinition, RunRow, RunStepContext, StepResult,
    register_run,
};
use crate::source_registry::{ResearchSource, SourceRegistry, SourceSeed};
use crate::state::AppState;

/// The registry key and the `kind` column. Written into every row this
/// definition has ever produced, so it never changes.
pub const RESEARCH_RUN_KIND: &str = "research";

// ── The depth budgets ────────────────────────────────────────────────────────
//
// Declared here rather than in the domain module: this module is loaded so the
// reclaim sweep can drive a research run it finds, and an import back into the
// domain module would be a cycle. The domain module re-exports the public names
// (`research_modes`, `plan_search`, …) so no caller moves.

/// The mode's spelling as the harness inputs and the role suffix carry it.
pub fn depth_str(mode: ResearchDepth) -> &'static str {
    match mode {
        ResearchDepth::Recon => "recon",
        ResearchDepth::Brief => "brief",
        ResearchDepth::Expedition => "expedition",
    }
}

/// Depth budget + search model preference per mode. Query counts bound each
/// round; rounds bound the expedition loop. Overridable for tests via env.
#[derive(Debug, Clone)]
pub struct ModeBudget {
    pub rounds: u64,
    pub queries: u64,
    pub search: Vec<&'static str>,
    pub blurb: &'static str,
}

fn base_budget(mode: ResearchDepth) -> ModeBudget {
    match mode {
        ResearchDepth::Recon => ModeBudget {
            rounds: 1,
            queries: 1,
            search: vec!["sonar", "sonar-pro"],
            blurb: "one fast pass: a cited answer in minutes",
        },
        ResearchDepth::Brief => ModeBudget {
            rounds: 1,
            queries: 3,
            search: vec!["sonar-pro", "sonar"],
            blurb: "planned angles, one synthesis: a briefing",
        },
        ResearchDepth::Expedition => ModeBudget {
            rounds: 3,
            queries: 4,
            search: vec!["sonar-pro", "sonar-reasoning-pro", "sonar"],
            blurb: "iterative deep dive: a full report",
        },
    }
}

/// Unset, unparsable or zero `TALARIA_RESEARCH_MAX_ROUNDS` all fall back to
/// the mode's own bound; a positive integer overrides it.
pub fn budget_for(mode: ResearchDepth) -> ModeBudget {
    let mut b = base_budget(mode);
    if let Ok(v) = std::env::var("TALARIA_RESEARCH_MAX_ROUNDS")
        && let Ok(n) = v.parse::<u64>()
        && n > 0
    {
        b.rounds = n;
    }
    b
}

/// The picker's mode list: what the UI shows next to each button.
pub fn research_modes() -> Vec<(&'static str, &'static str)> {
    [
        ResearchDepth::Recon,
        ResearchDepth::Brief,
        ResearchDepth::Expedition,
    ]
    .into_iter()
    .map(|m| (depth_str(m), base_budget(m).blurb))
    .collect()
}

// ── Search planning ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchVia {
    /// The model searches as part of answering.
    Native,
    /// Our harness drives a search tool and hands it the results.
    Tool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchPlan {
    pub model: String,
    pub via: SearchVia,
    pub supplier: Option<Supplier>,
}

/// THE REASON A RUN CANNOT START, phrased for the person who can fix it.
/// Exported so the route and the MCP tool report the same sentence.
pub const NO_SEARCH_REASON: &str = "this workspace cannot search yet. Either connect a search backend (Settings \u{2192} Search, e.g. a self-hosted SearXNG) so any model can research through it, or register a model with native web search and assign it to the research role";

pub type ModelsFn = Arc<dyn Fn() -> BoxFuture<'static, Vec<String>> + Send + Sync>;
pub type RoleModelFn = Arc<dyn Fn(String) -> BoxFuture<'static, Option<String>> + Send + Sync>;
pub type PathOfFn = Arc<dyn Fn(String) -> BoxFuture<'static, Option<SearchPlan>> + Send + Sync>;

/// The planner's three reads, injectable so the CHOICE RULE is tested without
/// a database.
pub struct PlanSearchDeps {
    pub models: ModelsFn,
    pub role_model: RoleModelFn,
    pub reach: PathOfFn,
}

/// HOW THE RUN'S SEARCH MODEL IS CHOSEN — proof required, in this order:
///
/// 1. The admin said which one (`research-<mode>` role), but only when the
///    capability is PROVEN, never presumed — the founder's rule, after a run
///    asked a plain chat model to search the live web and got a fluent brief
///    from training data. Silence is not a capability HERE, where the model is
///    chosen: `reach_for_keys` says `native` only when a probe or catalog
///    MEASURED this model browsing, and says `tool` when a search backend is
///    really there on this install.
/// 2. A model that searches on its own, if the org has one — asked of its
///    CAPABILITY FACTS, so a new sonar spelling is picked up the day it is
///    registered.
/// 3. Anything routable, through our own search (the model only has to be able
///    to call a tool, which is most of them).
pub async fn plan_search_with(mode: ResearchDepth, deps: &PlanSearchDeps) -> Option<SearchPlan> {
    let assigned = (deps.role_model)(format!("research-{}", depth_str(mode))).await;
    if let Some(assigned) = assigned
        && let Some(plan) = (deps.reach)(assigned).await
    {
        return Some(plan);
    }

    let models = (deps.models)().await;
    if models.is_empty() {
        return None;
    }
    let plans: Vec<Option<SearchPlan>> =
        futures_util::future::join_all(models.into_iter().map(|m| (deps.reach)(m))).await;
    let flat: Vec<SearchPlan> = plans.into_iter().flatten().collect();
    flat.iter()
        .find(|p| p.via == SearchVia::Native)
        .cloned()
        .or_else(|| flat.first().cloned())
}

/// The real planner over the pool.
pub async fn plan_search(pg: &PgPool, mode: ResearchDepth) -> Option<SearchPlan> {
    plan_search_with(mode, &real_plan_search_deps(pg)).await
}

fn real_plan_search_deps(pg: &PgPool) -> PlanSearchDeps {
    let pg_models = pg.clone();
    let pg_role = pg.clone();
    let pg_reach = pg.clone();
    PlanSearchDeps {
        models: Arc::new(move || {
            let pg = pg_models.clone();
            Box::pin(async move {
                // A read failure means "no models",
                // and the planner returns None rather than starting a blind run.
                gateway_models(&pg)
                    .await
                    .unwrap_or_default()
                    .into_iter()
                    .map(|m| m.id)
                    .collect()
            })
        }),
        role_model: Arc::new(move |role: String| {
            let pg = pg_role.clone();
            Box::pin(async move { resolve_role_model(&pg, &role).await.ok().flatten() })
        }),
        reach: Arc::new(move |model: String| {
            let pg = pg_reach.clone();
            Box::pin(async move {
                let keys = capability_keys_for(&pg, &model).await;
                let reach = reach_for_keys(&pg, &keys, &["search"]).await;
                match reach.get("search").and_then(|r| r.via) {
                    Some(ReachVia::Tool) => Some(SearchPlan {
                        model,
                        via: SearchVia::Tool,
                        supplier: reach.get("search").and_then(|r| r.supplier.clone()),
                    }),
                    Some(ReachVia::Native) => Some(SearchPlan {
                        model,
                        via: SearchVia::Native,
                        supplier: None,
                    }),
                    None => None,
                }
            })
        }),
    }
}

/// A deep-research-class search model is an agentic researcher in itself (each
/// call runs its own multi-search sweep) — shrink OUR loop so effort doesn't
/// multiply: fewer, bigger stages instead of many small ones.
///
/// AUDIT NOTE, left as-is deliberately: this is capability reasoning done by
/// REGEX ON A MODEL NAME (`/deep-research/i`) rather than a declared
/// capability. Not converted because "runs its
/// own multi-search sweep" is a genuinely new capability; the right end state
/// is a `deep-research` capability a probe or catalog declares. Until then the
/// substring match is honest about being a heuristic.
fn is_deep_research_model(model: &str) -> bool {
    model.to_lowercase().contains("deep-research")
}

pub fn adapt_budget(budget: ModeBudget, search_model: &str) -> ModeBudget {
    if !is_deep_research_model(search_model) {
        return budget;
    }
    ModeBudget {
        rounds: budget.rounds.min(2),
        queries: 1,
        ..budget
    }
}

// ── Input and checkpoint ─────────────────────────────────────────────────────

/// Everything the pipeline needs that does not change while it runs. On the
/// RUN's input rather than read back from `research_runs` every step because
/// the run row is written first: a process that dies between the two inserts
/// leaves a run that can still rebuild the domain record from what it was
/// started with. camelCase serde — the keys were camelCase from the first row
/// written, and an input a previous deploy wrote must re-enter here.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResearchInput {
    pub question: String,
    pub mode: ResearchDepth,
    pub agent_model: String,
    pub owner_user_id: Option<String>,
    pub requested_by: String,
    /// THE RUN THIS ONE EXTENDS, for a follow-up asked from a report's own
    /// conversation. On the INPUT because a resumed run must seed the same
    /// registry the first attempt did — the parent's sources go into the
    /// checkpoint at `begin`, so [1]..[n] keep meaning what the
    /// already-written prose says they mean however many times the driver
    /// dies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_run_id: Option<String>,
}

/// What the NEXT step does — the one field the driver's re-entry branches on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ResearchStage {
    Plan,
    Search,
    Synthesize,
    Artifact,
    Save,
    Publish,
}

/// THE RESUME CURSOR, and the work so far. It carries findings and sources
/// rather than only positions because the alternative is re-running completed
/// searches to rebuild them — every one of those is a paid model call.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResearchCheckpoint {
    pub stage: ResearchStage,
    /// Resolved once, in `begin`, and then fixed for the life of the run: an
    /// admin re-pointing the `research-*` role mid-expedition must not switch
    /// models between one round and the next.
    pub search_model: String,
    /// HOW that model reaches the web, resolved in the same breath: the plan's
    /// supplier when the model is blind and a tool does the searching, null
    /// when it searches as part of answering.
    pub search_supplier: Option<Supplier>,
    /// The adapted depth budget, likewise resolved once.
    pub rounds: u64,
    pub per_round: u64,
    /// 1-based. `round > rounds` means the loop is done.
    pub round: u64,
    /// This round's planned queries, in order.
    pub plan: Vec<String>,
    /// How many of `plan` have COME BACK and are already in `findings`. The
    /// single number that makes "resume mid-round" mean anything.
    pub done: u64,
    /// Across all rounds — the `queries` stat, and the number in the phase
    /// line.
    pub queries_run: u64,
    /// One markdown note per query that has run, in order.
    pub findings: Vec<String>,
    pub sources: Vec<ResearchSource>,
    /// Any query threw. Goes to the synthesis harness, which grounds against
    /// it.
    pub search_failed: bool,
    /// How many times the run has re-searched on its own after a round that
    /// found nothing citable — nobody is asked any more; this bounds the run's
    /// OWN retries.
    #[serde(default)]
    pub retries: u64,
    /// The written report, once the persona has produced it.
    pub report: Option<Report>,
    /// THE IDEMPOTENCY HANDLE for the one effect that cannot be undone.
    /// Written before the body is, so a re-entry writes into the artifact it
    /// already made rather than creating a second report.
    pub artifact_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub title: String,
    pub body: String,
    pub cited: u64,
    pub dropped: u64,
    pub ungrounded: u64,
}

/// One search query's outcome: the model's cited answer and its source list.
#[derive(Debug, Clone)]
pub struct SearchHit {
    pub content: String,
    pub sources: Vec<SourceSeed>,
}

// ── Deps ─────────────────────────────────────────────────────────────────────

pub struct PlanQueriesArgs {
    pub run_id: String,
    pub agent_model: String,
    pub question: String,
    pub findings_so_far: Vec<String>,
    pub max: u64,
}

pub struct SearchArgs {
    pub run_id: String,
    pub model: String,
    /// The plan's supplier: set means the model is blind and the tool does the
    /// searching.
    pub supplier: Option<Supplier>,
    pub query: String,
}

pub struct SynthesizeArgs {
    pub run_id: String,
    pub agent_model: String,
    pub question: String,
    pub mode: ResearchDepth,
    pub sources: Vec<SynthSource>,
    pub findings: Vec<String>,
    pub search_failed: bool,
}

pub struct SynthOutcome {
    pub doc: String,
    pub ungrounded: u64,
}

pub struct CreateReportArgs {
    pub run_id: String,
    pub title: String,
    pub owner_user_id: Option<String>,
    pub requested_by: String,
    pub agent_label: String,
}

pub struct WriteReportArgs {
    pub artifact_id: String,
    pub body: String,
    pub owner_user_id: Option<String>,
    /// Did an ORG agent start this run (the row-derived ORG_AGENT_RUN test)?
    /// Decides REACH, where the owner decides governance: an org-agent run
    /// publishes org-visible however owned, a personal one goes private.
    pub org_run: bool,
    pub agent_label: String,
    /// The run's agent (fleet model id) — granted viewer on a personal
    /// report so the agent that wrote it can read it back.
    pub agent_model: String,
    pub member_ids: Vec<String>,
}

/// The editor grants a personal report lands with: everyone shared on the
/// run as editors, and the agent that ran it as a viewer. The toolkit's own
/// instructions tell that agent to read the report back with get_document,
/// and the brain already scopes a personal report to "them + their
/// assistant" — these grants are what make the artifacts plane agree with
/// both. (The pre-port TS behaved the same way and had the same gap.)
fn personal_report_grants(member_ids: &[String], agent_model: &str) -> Vec<EditorGrant> {
    let mut grants: Vec<EditorGrant> = member_ids
        .iter()
        .map(|id| EditorGrant {
            principal_type: "user".into(),
            principal_id: id.clone(),
            role: "editor".into(),
        })
        .collect();
    grants.push(EditorGrant {
        principal_type: "agent".into(),
        principal_id: agent_model.to_string(),
        role: "viewer".into(),
    });
    grants
}

pub struct IndexArgs {
    pub run_id: String,
    pub artifact_id: String,
    pub title: String,
    pub body: String,
    pub question: String,
    pub mode: ResearchDepth,
    pub owner_user_id: Option<String>,
    /// Same row-derived org-ness as WriteReportArgs carries: an org-agent
    /// run indexes ambient (orgWide) however owned; a personal one goes to
    /// the owner's private brain.
    pub org_run: bool,
}

pub struct NotifyArgs {
    pub owner_user_id: String,
    pub run_id: String,
    pub title: String,
    pub agent_label: String,
    pub mode: ResearchDepth,
    pub sources: u64,
}

pub struct FinishRowArgs {
    pub run_id: String,
    pub artifact_id: String,
    pub stats: Value,
}

pub type SearchPlanForFn =
    Arc<dyn Fn(ResearchDepth) -> BoxFuture<'static, Option<SearchPlan>> + Send + Sync>;
pub type PlanQueriesFn =
    Arc<dyn Fn(PlanQueriesArgs) -> BoxFuture<'static, Vec<String>> + Send + Sync>;
pub type SearchFn =
    Arc<dyn Fn(SearchArgs) -> BoxFuture<'static, Result<SearchHit, String>> + Send + Sync>;
pub type SynthesizeFn =
    Arc<dyn Fn(SynthesizeArgs) -> BoxFuture<'static, Result<SynthOutcome, String>> + Send + Sync>;
pub type AgentLabelFn = Arc<dyn Fn(&str) -> String + Send + Sync>;
pub type EnsureRowFn =
    Arc<dyn Fn(&str, ResearchInput) -> BoxFuture<'static, Result<(), String>> + Send + Sync>;
pub type SourcesOfFn =
    Arc<dyn Fn(String) -> BoxFuture<'static, Result<Vec<ResearchSource>, String>> + Send + Sync>;
pub type RowExistsFn =
    Arc<dyn Fn(String) -> BoxFuture<'static, Result<bool, String>> + Send + Sync>;
pub type MemberIdsFn =
    Arc<dyn Fn(String) -> BoxFuture<'static, Result<Vec<String>, String>> + Send + Sync>;
/// Did an ORG agent start this run? The row answers (research.rs's
/// ORG_AGENT_RUN test) — derived at write time, never serialized on the
/// input, so a run in flight across a deploy resolves as correctly as one
/// started after it.
pub type OrgRunFn = Arc<dyn Fn(String) -> BoxFuture<'static, Result<bool, String>> + Send + Sync>;
pub type SaveSourcesFn = Arc<
    dyn Fn(String, Vec<ResearchSource>) -> BoxFuture<'static, Result<(), String>> + Send + Sync,
>;
pub type FinishRowFn =
    Arc<dyn Fn(FinishRowArgs) -> BoxFuture<'static, Result<(), String>> + Send + Sync>;
pub type FailRowFn =
    Arc<dyn Fn(String, String) -> BoxFuture<'static, Result<(), String>> + Send + Sync>;
pub type LinkedArtifactFn =
    Arc<dyn Fn(String) -> BoxFuture<'static, Result<Option<String>, String>> + Send + Sync>;
pub type CreateReportFn =
    Arc<dyn Fn(CreateReportArgs) -> BoxFuture<'static, Result<String, String>> + Send + Sync>;
pub type WriteReportFn =
    Arc<dyn Fn(WriteReportArgs) -> BoxFuture<'static, Result<(), String>> + Send + Sync>;
pub type IndexFn = Arc<dyn Fn(IndexArgs) -> BoxFuture<'static, Result<(), String>> + Send + Sync>;
pub type NotifyFn = Arc<dyn Fn(NotifyArgs) -> BoxFuture<'static, Result<(), String>> + Send + Sync>;

/// Every edge the run touches, each overridable, so tests drive whole runs —
/// including a reclaim — with no database, no Redis and no model.
#[derive(Clone)]
pub struct ResearchRunDeps {
    /// The whole plan, not just the model: it says which PATH the run's
    /// searches take. None = this install cannot prove search for anything,
    /// and the run refuses to start rather than pay a blind model to answer
    /// from memory.
    pub search_plan_for: SearchPlanForFn,
    /// The persona plans a round. Never fails — a failed plan is one lost
    /// round, and an empty list is the persona saying the question is
    /// saturated.
    pub plan_queries: PlanQueriesFn,
    /// One query against the search model. ERRS on a dead query, which costs
    /// one angle rather than the run.
    pub search: SearchFn,
    /// The persona writes the document. Errs on an unusable reply.
    pub synthesize: SynthesizeFn,
    pub agent_label: AgentLabelFn,
    /// Write the domain record if it is not there. Idempotent on the id.
    pub ensure_row: EnsureRowFn,
    /// The parent report's numbered sources, for a follow-up.
    pub sources_of: SourcesOfFn,
    /// Is the domain record still there? DELETE /api/research/{id} removes it,
    /// and that is the product's "stop this" — a run that kept spending after
    /// it would be billing a person for a thing they threw away.
    pub row_exists: RowExistsFn,
    pub member_ids: MemberIdsFn,
    /// Org-ness of the run, from the row (see OrgRunFn). The Save and Publish
    /// steps both ask — a resume between them must re-derive, not trust a
    /// checkpoint, because the answer rides no serialized state.
    pub org_run: OrgRunFn,
    pub save_sources: SaveSourcesFn,
    pub finish_row: FinishRowFn,
    /// Mirror a failure onto the domain record. See `mirror_failure`.
    pub fail_row: FailRowFn,
    /// The artifact already linked to this run, if a previous entry made one.
    pub linked_artifact: LinkedArtifactFn,
    /// Create the report artifact AND link it to the run, in that order and
    /// with nothing between them: the link is what makes the id findable by
    /// the next entry, so the window in which a crash costs a second artifact
    /// is one statement wide.
    pub create_report: CreateReportFn,
    pub write_report: WriteReportFn,
    pub index: IndexFn,
    pub notify: NotifyFn,
}

// ── The real model edges ─────────────────────────────────────────────────────

/// One search query → the search model's cited answer + its source list.
///
/// THE TRANSPORT IS THE PLAN'S, and this is the half of the capability rule the
/// run used to break. A supplier set means `tool_search_transport` — the model
/// drives our checked search tool and the sources come off the tool's results —
/// and no supplier means the sonar-native transport that reads the provider's
/// own `search_results`/`citations` annotations. The capability floor is the
/// harness's (`requires: ['search']`, refuse below), so a model KNOWN not to
/// search fails loudly here instead of answering fluently from training data.
async fn search_stage(state: &AppState, args: SearchArgs) -> Result<SearchHit, String> {
    let sink: SearchSink = Arc::new(std::sync::Mutex::new(Vec::new()));
    let transport = match &args.supplier {
        Some(supplier) => tool_search_transport(
            state.clone(),
            args.run_id.clone(),
            sink.clone(),
            supplier.clone(),
            ToolSearchDeps {
                call_tool: None,
                base: None,
            },
        ),
        None => search_transport(state.clone(), args.run_id.clone(), sink.clone()),
    };
    let mut deps = harness_real_deps(state);
    deps.transport = transport;
    let run = run_harness(
        state,
        &search_harness(),
        &json!({ "query": args.query }),
        RunContext {
            caller: format!("research:{}", args.run_id),
            model: Some(args.model.clone()),
            deps: Some(Arc::new(deps)),
            ..Default::default()
        },
    )
    .await
    .map_err(|e| e.0)?;
    // `search_harness` declares `OnFailure::Throw` and the runner honors it on
    // every path that fails to produce a value — the transport and the pre-call
    // ones included — so this is a TYPE narrowing rather than a second copy of
    // the policy.
    let Some(Value::String(content)) = run.value else {
        return Err(run
            .error
            .unwrap_or_else(|| format!("search stage produced nothing on \"{}\"", args.model)));
    };
    let sources = sink
        .lock()
        .expect("the search sink is never held across an await")
        .iter()
        .cloned()
        .map(|s| SourceSeed {
            url: s.url,
            title: s.title,
            snippet: s.snippet,
        })
        .collect();
    Ok(SearchHit { content, sources })
}

/// This round's search queries, or an empty list when the persona says the
/// question is saturated. Never fails.
///
/// THE LINE-BASED SALVAGE, which used to live inside the extractor. On a small
/// model a numbered list is likelier than a JSON array. The tolerance is kept:
/// the harness records the contract failure on its `harness_runs` row, and the
/// salvage runs afterwards, here, where it is visible. It runs ONLY on a reply
/// the guard found nothing in — flagged content never re-enters a model's
/// context, and a salvage path must not quietly route around that rule.
async fn plan_stage(state: &AppState, args: PlanQueriesArgs) -> Vec<String> {
    let input = json!({
        "question": args.question,
        "max": args.max,
        "findingsSoFar": args.findings_so_far,
    });
    let run = run_harness(
        state,
        &queries_harness(),
        &input,
        RunContext {
            // Both persona stages of a run carry the same attribution — the
            // search stages meter themselves through the gateway. An expedition
            // plans up to three times, so leaving this off would make the
            // planning half of a run's cost the half nobody could find.
            caller: format!("research:{}", args.run_id),
            model: Some(args.agent_model),
            ledger: Some(RunLedger {
                source: Some(LedgerSource::Research),
                ref_id: Some(args.run_id),
                task_id: None,
            }),
            ..Default::default()
        },
    )
    .await;
    let Ok(run) = run else {
        // A null-policy harness never takes the thrown half; the map is the
        // type's demand, and one lost round is the contract's answer.
        return Vec::new();
    };
    if let Some(Value::Array(items)) = &run.value {
        let queries: Vec<String> = items
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect();
        return clamp_queries(&queries, args.max as usize);
    }
    if let Some(raw) = &run.raw
        && run.findings.is_empty()
    {
        return clamp_queries(&queries_from_lines(raw), args.max as usize);
    }
    Vec::new()
}

/// The persona writes the document against the global registry.
///
/// THE GROUNDING PASS IS INSIDE THIS CALL: `search_failed` and the findings
/// are on the INPUT because `synthesis_harness` declares `ground` — the search
/// hits are this turn's tool results, so `ungrounded_ref` — the definitive
/// research failure mode — runs on the report, from the runner, with one
/// findings row per fabricated link.
async fn synthesis_stage(state: &AppState, args: SynthesizeArgs) -> Result<SynthOutcome, String> {
    let input = serde_json::to_value(SynthesisInput {
        question: args.question,
        mode: args.mode,
        sources: args.sources,
        findings: args.findings,
        search_failed: args.search_failed,
    })
    .map_err(|e| e.to_string())?;
    let run = run_harness(
        state,
        &synthesis_harness(),
        &input,
        RunContext {
            // `source: research` + the run id is what makes a run's COST
            // answerable: the persona stages meter as research and not as an
            // ownerless chat turn.
            caller: format!("research:{}", args.run_id),
            model: Some(args.agent_model),
            ledger: Some(RunLedger {
                source: Some(LedgerSource::Research),
                ref_id: Some(args.run_id),
                task_id: None,
            }),
            ..Default::default()
        },
    )
    .await
    .map_err(|e| e.0)?;
    // A TYPE NARROWING, not a policy — `synthesis_harness` declares
    // `OnFailure::Throw`. It used to be a policy restated by hand, and the
    // `?? ''` it defended with saved an artifact containing only the Sources
    // list, marked the run done, indexed the empty report and notified the
    // requester.
    let Some(Value::String(doc)) = run.value else {
        return Err(run
            .error
            .unwrap_or_else(|| "the report came back empty".into()));
    };
    // Filtered to `ungrounded_ref` because `findings` also carries the
    // `secret_leak`/`pii_leak` hits, and this number has one meaning on the
    // research surface: how many links this model asserted that no search
    // result backs.
    let ungrounded = run
        .findings
        .iter()
        .filter(|f| f.check == "ungrounded_ref")
        .count() as u64;
    Ok(SynthOutcome { doc, ungrounded })
}

// ── The real domain edges ────────────────────────────────────────────────────

fn truncate_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// The real edges. `state` carries the pool the domain rows write through and
/// the harness assembly the three model stages run over.
pub fn real_research_deps(state: AppState) -> ResearchRunDeps {
    let st_plan = state.clone();
    let st_plan_q = state.clone();
    let st_search = state.clone();
    let st_synth = state.clone();
    let st_ensure = state.clone();
    let st_sources = state.clone();
    let st_exists = state.clone();
    let st_members = state.clone();
    let st_save = state.clone();
    let st_finish = state.clone();
    let st_fail = state.clone();
    let st_linked = state.clone();
    let st_create = state.clone();
    let st_write = state.clone();
    let st_index = state.clone();
    let st_org = state.clone();
    let st_notify = state;

    ResearchRunDeps {
        search_plan_for: Arc::new(move |mode| {
            let pg = st_plan.pg.clone();
            Box::pin(async move { plan_search(&pg, mode).await })
        }),
        plan_queries: Arc::new(move |args| {
            let state = st_plan_q.clone();
            Box::pin(async move { plan_stage(&state, args).await })
        }),
        search: Arc::new(move |args| {
            let state = st_search.clone();
            Box::pin(async move { search_stage(&state, args).await })
        }),
        synthesize: Arc::new(move |args| {
            let state = st_synth.clone();
            Box::pin(async move { synthesis_stage(&state, args).await })
        }),
        agent_label: Arc::new(|model: &str| describe_agent(model).label),

        ensure_row: Arc::new(move |run_id: &str, input: ResearchInput| {
            let pg = st_ensure.pg.clone();
            let run_id = run_id.to_string();
            Box::pin(async move {
                // The run row is written first, so this is the recovery path
                // for a process that died between the two inserts. `on
                // conflict do nothing` because the ordinary path is
                // `start_research` having already written it — and because a
                // DELETED run must not be resurrected by a later step, which is
                // why this only ever runs at the very first step.
                sqlx::query(
                    "insert into research_runs \
                     (id, owner_user_id, requested_by, agent_model, mode, question, parent_run_id) \
                     values ($1::uuid, $2::uuid, $3, $4, $5, $6, $7::uuid) \
                     on conflict (id) do nothing",
                )
                .bind(&run_id)
                .bind(&input.owner_user_id)
                .bind(&input.requested_by)
                .bind(&input.agent_model)
                .bind(depth_str(input.mode))
                .bind(&input.question)
                .bind(&input.parent_run_id)
                .execute(&pg)
                .await
                .map(|_| ())
                .map_err(|e| e.to_string())
            })
        }),

        sources_of: Arc::new(move |run_id: String| {
            let pg = st_sources.pg.clone();
            Box::pin(async move {
                let rows: Vec<(i32, String, Option<String>, Option<String>)> = sqlx::query_as(
                    "select idx, url, title, snippet from research_sources \
                     where run_id = $1::uuid order by idx asc",
                )
                .bind(&run_id)
                .fetch_all(&pg)
                .await
                .map_err(|e| e.to_string())?;
                Ok(rows
                    .into_iter()
                    .map(|(idx, url, title, snippet)| ResearchSource {
                        idx: idx.max(0) as u64,
                        url,
                        title,
                        snippet,
                    })
                    .collect())
            })
        }),

        row_exists: Arc::new(move |run_id: String| {
            let pg = st_exists.pg.clone();
            Box::pin(async move {
                let row: Option<(i32,)> =
                    sqlx::query_as("select 1 from research_runs where id = $1::uuid")
                        .bind(&run_id)
                        .fetch_optional(&pg)
                        .await
                        .map_err(|e| e.to_string())?;
                Ok(row.is_some())
            })
        }),

        member_ids: Arc::new(move |run_id: String| {
            let pg = st_members.pg.clone();
            Box::pin(async move {
                let rows: Vec<(String,)> = sqlx::query_as(
                    "select user_id::text from research_members where run_id = $1::uuid",
                )
                .bind(&run_id)
                .fetch_all(&pg)
                .await
                .map_err(|e| e.to_string())?;
                Ok(rows.into_iter().map(|(id,)| id).collect())
            })
        }),

        org_run: Arc::new(move |run_id: String| {
            let pg = st_org.pg.clone();
            Box::pin(async move {
                crate::research::is_org_agent_run(&pg, &run_id)
                    .await
                    .map_err(|e| e.to_string())
            })
        }),

        save_sources: Arc::new(move |run_id: String, sources: Vec<ResearchSource>| {
            let pg = st_save.pg.clone();
            Box::pin(async move {
                for s in &sources {
                    sqlx::query(
                        "insert into research_sources (run_id, idx, url, title, snippet) \
                         values ($1::uuid, $2, $3, $4, $5) on conflict do nothing",
                    )
                    .bind(&run_id)
                    .bind(s.idx as i64)
                    .bind(&s.url)
                    .bind(&s.title)
                    .bind(&s.snippet)
                    .execute(&pg)
                    .await
                    .map_err(|e| e.to_string())?;
                }
                Ok(())
            })
        }),

        finish_row: Arc::new(move |args: FinishRowArgs| {
            let pg = st_finish.pg.clone();
            Box::pin(async move {
                // `status` is written here and in `fail_row` and NOWHERE else.
                // It is the TERMINAL OUTCOME, kept as a column because
                // /api/research's duplicate-question check reads it in raw SQL.
                // `error = null` because a run that finishes after a failed
                // attempt must not keep the old attempt's sentence.
                sqlx::query(
                    "update research_runs set status = 'done', phase = null, error = null, \
                     artifact_id = $2::uuid, stats = $3::jsonb, updated_at = now(), \
                     completed_at = now() where id = $1::uuid",
                )
                .bind(&args.run_id)
                .bind(&args.artifact_id)
                .bind(&args.stats)
                .execute(&pg)
                .await
                .map(|_| ())
                .map_err(|e| e.to_string())
            })
        }),

        fail_row: Arc::new(move |run_id: String, error: String| {
            let pg = st_fail.pg.clone();
            Box::pin(async move {
                let bounded = truncate_chars(&error, 2000);
                sqlx::query(
                    "update research_runs set status = 'error', phase = null, error = $2, \
                     updated_at = now() where id = $1::uuid and status <> 'done'",
                )
                .bind(&run_id)
                .bind(&bounded)
                .execute(&pg)
                .await
                .map(|_| ())
                .map_err(|e| e.to_string())
            })
        }),

        linked_artifact: Arc::new(move |run_id: String| {
            let pg = st_linked.pg.clone();
            Box::pin(async move {
                let row: Option<(String,)> = sqlx::query_as(
                    "select artifact_id::text from artifact_links \
                     where target_type = 'research' and target_id = $1 limit 1",
                )
                .bind(&run_id)
                .fetch_optional(&pg)
                .await
                .map_err(|e| e.to_string())?;
                Ok(row.map(|(id,)| id))
            })
        }),

        create_report: Arc::new(move |args: CreateReportArgs| {
            let pg = st_create.pg.clone();
            Box::pin(async move {
                let folder =
                    agent_category_folder(&pg, &args.agent_label, "Research", &args.requested_by)
                        .await;
                let artifact = create_artifact(
                    &pg,
                    Some("doc"),
                    Some(&args.title),
                    &args.requested_by,
                    args.owner_user_id.as_deref(),
                    folder.as_deref(),
                )
                .await
                .map_err(|e| e.to_string())?;
                // IMMEDIATELY, and with nothing between: the link is the handle
                // a re-entry finds the artifact by, so every statement between
                // the create and it is another statement wide the "two reports"
                // window gets.
                attach_artifact(
                    &pg,
                    &artifact.id,
                    "research",
                    &args.run_id,
                    &args.agent_label,
                )
                .await
                .map_err(|e| e.to_string())?;
                Ok(artifact.id)
            })
        }),

        write_report: Arc::new(move |args: WriteReportArgs| {
            let pg = st_write.pg.clone();
            Box::pin(async move {
                // TWO facts, deliberately two different questions. The OWNER
                // decides governance — whose row the report is, who shares or
                // deletes it. ORG-NESS decides reach: an org-agent run
                // publishes org-visible however owned (the ladder's human
                // governs it instead of an allow-list), while a personal run —
                // a person's own ask, or their assistant's — stays PRIVATE to
                // them, with the run's members granted editor on the doc:
                // sharing the run is the only way anyone else sees it.
                let org_reach = args.org_run || args.owner_user_id.is_none();
                save_artifact(
                    &pg,
                    &args.artifact_id,
                    SaveArtifactPatch {
                        body: Some(&args.body),
                        visibility: Some(if org_reach { "org" } else { "private" }),
                        ..Default::default()
                    },
                    &args.agent_label,
                )
                .await
                .map_err(|e| e.to_string())?;
                if !org_reach {
                    // A report that is saved must not fail its run because a
                    // share could not land.
                    let _ = set_editors(
                        &pg,
                        "artifact",
                        &args.artifact_id,
                        &personal_report_grants(&args.member_ids, &args.agent_model),
                    )
                    .await;
                }
                Ok(())
            })
        }),

        index: Arc::new(move |args: IndexArgs| {
            let pg = st_index.pg.clone();
            Box::pin(async move {
                // Placement follows org-ness, not ownership: an org-agent
                // run's report goes to the ambient index (marked orgWide so
                // scopes actually match it) however owned; a personal run's
                // goes to the owner's private brain (them + their assistant).
                let org_reach = args.org_run || args.owner_user_id.is_none();
                let mut payload = serde_json::Map::new();
                payload.insert("runId".into(), json!(args.run_id));
                payload.insert("question".into(), json!(args.question));
                payload.insert("mode".into(), json!(depth_str(args.mode)));
                if org_reach {
                    payload.insert("orgWide".into(), json!(true));
                }
                let doc = IndexDoc {
                    source_type: "research".into(),
                    source_id: args.artifact_id.clone(),
                    title: Some(args.title.clone()),
                    text: format!("{}\n\n{}", args.title, args.body),
                    payload: Some(payload),
                    // The run's own path, never a query param — Research
                    // selects by path segment, and an href here is what a
                    // retrieval hit hands the clicker.
                    href: Some(format!("/research/{}", args.run_id)),
                };
                let qd = qdrant::real_deps();
                let ed = embed::real_deps();
                match &args.owner_user_id {
                    Some(owner) if !org_reach => {
                        // index_personal swallows its own failures whole-body —
                        // a report that exists but is unfindable is not a
                        // failed report.
                        index_personal(&pg, &qd, &ed, owner, &doc).await;
                        Ok(())
                    }
                    _ => index_activity(&pg, &qd, &ed, &doc).await,
                }
            })
        }),

        notify: Arc::new(move |args: NotifyArgs| {
            let st = st_notify.clone();
            Box::pin(async move {
                let deps = NotifyDeps::publishing(st.pg.clone(), st.redis().await.ok());
                add_notification(
                    &deps,
                    &args.owner_user_id,
                    &NotificationInput {
                        kind: "research",
                        title: &format!("Research ready: {}", truncate_chars(&args.title, 120)),
                        body: Some(&format!(
                            "{} finished {}: {} sources",
                            args.agent_label,
                            match args.mode {
                                ResearchDepth::Recon => "a recon",
                                ResearchDepth::Brief => "a brief",
                                ResearchDepth::Expedition => "an expedition",
                            },
                            args.sources
                        )),
                        // The run's own path: /research/<id>. This href is
                        // what the inbox card and the notification email link
                        // at, and Research opens a selection from the path
                        // only — the `?r=` form landed people on the bare
                        // view with nothing selected. Rows written before the
                        // fix are repaired by migration; emails already sent
                        // are caught by AppLayout's legacy redirect.
                        href: Some(&format!("/research/{}", args.run_id)),
                    },
                )
                .await
                .map_err(|e| e.to_string())
            })
        }),
    }
}

// ── The step ─────────────────────────────────────────────────────────────────

/// The sentence a run with nothing citable ends on — the same words the
/// pre-parking version threw, so the error a user sees never got softer.
const NO_SOURCES: &str = "no sources found. Search returned nothing citable.";

/// How many times the run re-searches on its own before ending. Two retries:
/// the transient outage, and the one after it.
const MAX_NO_SOURCE_RETRIES: u64 = 2;

/// Risk 2, stated as a function. `max_step_ms` and a lost lease abort by
/// REJECTING a race — nothing stops a future that ignores its signal — so a
/// step can be re-entered on another instance while this one is still in
/// flight. Called before every outward call so the abandoned copy stops
/// spending rather than racing its successor to the same checkpoint.
fn stop_if_abandoned(ctx: &RunStepContext) -> Result<(), String> {
    if ctx.signal.is_aborted() {
        Err(
            "the driver abandoned this step; another instance resumes from the last checkpoint"
                .into(),
        )
    } else {
        Ok(())
    }
}

/// How round one starts, in one place because it happens twice: at `begin`,
/// and again when the run re-searches after a round that found nothing.
/// Recon asks the question itself — one pass, no planner call — and every
/// other mode plans its angles first. Two spellings of that would mean a
/// retried recon quietly acquiring a planning stage the mode does not have.
fn first_round(input: &ResearchInput) -> (ResearchStage, Vec<String>) {
    if input.mode == ResearchDepth::Recon {
        (ResearchStage::Search, vec![input.question.clone()])
    } else {
        (ResearchStage::Plan, Vec::new())
    }
}

/// The report-side marker grammar. TWO digits — deliberately narrower than
/// the REGISTRY's marker grammar (`source_registry::MARKER_RE`), which is
/// three because an expedition's global numbering can pass [99], while this
/// file's own three regexes (cleanup, dropped, cited) were never widened with
/// it. Kept at {1,2} on purpose: widening here would change report bytes (a
/// [100] marker that today survives the strip would start being counted and
/// stripped), and stable report bytes outrank the latent inconsistency. The
/// registry test file documents the same gap.
static REPORT_MARKER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[(\d{1,2})\]").expect("the marker grammar compiles"));

/// Strip a wrapping code fence — `^```[a-z]*\n?` at the very start or `\n?```$`
/// at the very end, the only two places a fence can be an artifact rather than
/// content.
static FENCE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^```[a-z]*\n?|\n?```$").expect("the fence grammar compiles"));

static H1_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^# (.+)$").expect("the title grammar compiles"));

/// Keep only markers that resolve; drop everything else.
fn cleanup_markers(doc: &str, known: &HashSet<u64>) -> String {
    let unfenced = FENCE_RE.replace_all(doc.trim(), "");
    REPORT_MARKER_RE
        .replace_all(&unfenced, |caps: &regex::Captures| {
            let n: u64 = caps[1].parse().unwrap_or(0);
            if known.contains(&n) {
                caps[0].to_string()
            } else {
                String::new()
            }
        })
        .into_owned()
}

fn marker_nums(doc: &str) -> Vec<u64> {
    REPORT_MARKER_RE
        .captures_iter(doc)
        .map(|c| c[1].parse().unwrap_or(0))
        .collect()
}

fn title_of(cleaned: &str, question: &str) -> String {
    H1_RE
        .captures(cleaned)
        .map(|c| c[1].trim().to_string())
        .unwrap_or_else(|| format!("Research: {}", truncate_chars(question, 80)))
}

fn stage_str(stage: ResearchStage) -> &'static str {
    match stage {
        ResearchStage::Plan => "plan",
        ResearchStage::Search => "search",
        ResearchStage::Synthesize => "synthesize",
        ResearchStage::Artifact => "artifact",
        ResearchStage::Save => "save",
        ResearchStage::Publish => "publish",
    }
}

/// Serialize the next checkpoint for a `Next` — clone-and-mutate at each
/// branch, which is why every branch builds its `next` from `cp.clone()` and
/// nothing here needs a Default.
fn step_next(cp: &ResearchCheckpoint, phase: &str) -> Result<StepResult, String> {
    Ok(StepResult::Next {
        checkpoint: serde_json::to_value(cp).map_err(|e| e.to_string())?,
        phase: Some(phase.to_string()),
    })
}

async fn advance(
    ctx: &RunStepContext,
    deps: &ResearchRunDeps,
    input: &ResearchInput,
    cp: &ResearchCheckpoint,
) -> Result<StepResult, String> {
    let run_id = ctx.run.id.clone();

    // DELETE on /api/research/{id} removes the domain record and is the only
    // stop button research has. Checked once per step rather than per call: it
    // costs one indexed read and it is the difference between "I deleted that"
    // and a run that goes on billing searches for a report nobody will ever
    // open.
    if !(deps.row_exists)(run_id.clone()).await? {
        tracing::info!(
            "[runs/research] {run_id}: the research record is gone (deleted) — stopping at stage \"{}\"",
            stage_str(cp.stage)
        );
        return Ok(StepResult::Done {
            result: json!({ "deleted": true }),
        });
    }

    match cp.stage {
        // ── plan: ONE persona call ─────────────────────────────────────────
        ResearchStage::Plan => {
            if cp.round > cp.rounds {
                let mut next = cp.clone();
                next.stage = ResearchStage::Synthesize;
                return step_next(&next, "writing the report");
            }
            (ctx.log)(if cp.round == 1 {
                "planning search angles".to_string()
            } else {
                format!("round {}: chasing gaps", cp.round)
            });
            stop_if_abandoned(ctx)?;
            let plan = (deps.plan_queries)(PlanQueriesArgs {
                run_id: run_id.clone(),
                agent_model: input.agent_model.clone(),
                question: input.question.clone(),
                // Round 1 plans from the question; later rounds plan against
                // what is already known, which is what makes an expedition
                // iterative.
                findings_so_far: if cp.round == 1 {
                    Vec::new()
                } else {
                    cp.findings.clone()
                },
                max: cp.per_round,
            })
            .await;
            // An empty plan is the persona saying the question is saturated —
            // a reason to synthesize, never a failure. `round = rounds + 1`
            // so a re-entry into plan falls straight through to synthesize
            // instead of planning a round the persona just declined.
            if plan.is_empty() {
                let mut next = cp.clone();
                next.stage = ResearchStage::Synthesize;
                next.round = cp.rounds + 1;
                return step_next(&next, "writing the report");
            }
            let n = plan.len();
            let mut next = cp.clone();
            next.stage = ResearchStage::Search;
            next.plan = plan;
            next.done = 0;
            step_next(
                &next,
                &format!("round {}: {} angle(s) planned", cp.round, n),
            )
        }

        // ── search: ONE sonar call, and the checkpoint straight after it ───
        ResearchStage::Search => {
            let Some(query) = cp.plan.get(cp.done as usize).cloned() else {
                // The round is finished. Defensive rather than reachable — the
                // branch below advances the round as it consumes the last
                // query — but a checkpoint written by an older deploy could
                // land here.
                let mut next = cp.clone();
                next.stage = ResearchStage::Plan;
                next.round = cp.round + 1;
                next.plan = Vec::new();
                next.done = 0;
                return step_next(&next, &format!("round {} complete", cp.round));
            };
            let n = cp.queries_run + 1;
            (ctx.log)(format!("searching ({n}): {}", truncate_chars(&query, 80)));
            let mut registry = SourceRegistry::from(&cp.sources);
            let note;
            let mut failed = cp.search_failed;
            let hit = async {
                stop_if_abandoned(ctx)?;
                (deps.search)(SearchArgs {
                    run_id: run_id.clone(),
                    model: cp.search_model.clone(),
                    supplier: cp.search_supplier.clone(),
                    query: query.clone(),
                })
                .await
            }
            .await;
            match hit {
                Ok(hit) => {
                    note = format!(
                        "### Query: {}\n{}",
                        query,
                        registry.renumber(&hit.content, &hit.sources)
                    );
                }
                Err(e) => {
                    // ONE DEAD QUERY COSTS ONE ANGLE, unchanged — except that
                    // an abandoned step must not be recorded as a failed
                    // search: it is this driver being taken off the run, and
                    // the query has not been asked yet.
                    if ctx.signal.is_aborted() {
                        return Err(e);
                    }
                    failed = true;
                    note = format!("### Query: {}\n(search failed: {e})", query);
                }
            }
            let done = cp.done + 1;
            let round_over = done >= cp.plan.len() as u64;
            let mut next = cp.clone();
            // The round advances IN THE SAME WRITE that records its last
            // query, so there is no checkpoint in which a completed round is
            // still pointing at a query that has already run.
            if round_over {
                next.stage = ResearchStage::Plan;
                next.round = cp.round + 1;
                next.plan = Vec::new();
                next.done = 0;
            } else {
                next.done = done;
            }
            next.queries_run = n;
            next.findings.push(note);
            next.sources = registry.list();
            next.search_failed = failed;
            step_next(&next, &format!("searched {n}"))
        }

        // ── synthesize: ONE persona call ───────────────────────────────────
        ResearchStage::Synthesize => {
            if cp.sources.is_empty() {
                // NOTHING CITABLE, AND NOBODY IS ASKED. This used to park on a
                // human decision — "search again?" — on the theory that only
                // the person who asked can tell a transient search outage from
                // a question the web cannot answer. Two runs parked on
                // 2026-08-28 and sat for hours looking exactly like working
                // ones (ticket #5): research is an autonomous harness. The
                // transient case answers ITSELF on the retry below, and the
                // unanswerable case was never going to be rescued by a human
                // clicking "search again" either — it ends the run, loudly,
                // with a sentence in `error`.
                if cp.retries >= MAX_NO_SOURCE_RETRIES {
                    return Err(NO_SOURCES.into());
                }
                let (stage, plan) = first_round(input);
                let mut next = cp.clone();
                next.stage = stage;
                next.plan = plan;
                // The findings are dropped with the round: with no sources in
                // the registry every one of them is a failure note or uncited
                // prose, and carrying them into the retry would put them in
                // the report.
                next.round = 1;
                next.done = 0;
                next.findings = Vec::new();
                next.search_failed = false;
                next.retries = cp.retries + 1;
                return step_next(&next, "searching again");
            }

            (ctx.log)("writing the report".to_string());
            stop_if_abandoned(ctx)?;
            let registry = SourceRegistry::from(&cp.sources);
            let synth_sources: Vec<SynthSource> = registry
                .list()
                .iter()
                .map(|s| SynthSource {
                    idx: s.idx,
                    url: s.url.clone(),
                    title: s.title.clone(),
                })
                .collect();
            let SynthOutcome { doc, ungrounded } = (deps.synthesize)(SynthesizeArgs {
                run_id: run_id.clone(),
                agent_model: input.agent_model.clone(),
                question: input.question.clone(),
                mode: input.mode,
                sources: synth_sources,
                findings: cp.findings.clone(),
                search_failed: cp.search_failed,
            })
            .await?;

            // Keep only markers that resolve; append the mechanical Sources
            // section.
            //
            // `dropped` is counted rather than only stripped. Deleting an
            // invented citation is the right thing to save, but it also made a
            // model that fabricates half its markers look identical to one
            // that cites perfectly — the exact model-fitness signal this run
            // is in the best position to report, thrown away by the line that
            // fixed the symptom.
            let known: HashSet<u64> = registry.list().iter().map(|s| s.idx).collect();
            let dropped = marker_nums(&doc)
                .into_iter()
                .filter(|n| !known.contains(n))
                .count() as u64;
            let cleaned = cleanup_markers(&doc, &known);
            let cited: HashSet<u64> = marker_nums(&cleaned).into_iter().collect();
            let sources_md = registry
                .list()
                .iter()
                .map(|s| {
                    format!(
                        "{}. [{}]({}){}",
                        s.idx,
                        s.title.clone().unwrap_or_else(|| s.url.clone()),
                        s.url,
                        if cited.contains(&s.idx) {
                            ""
                        } else {
                            " *(consulted)*"
                        }
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            let mut next = cp.clone();
            next.stage = ResearchStage::Artifact;
            next.report = Some(Report {
                title: title_of(&cleaned, &input.question),
                body: format!("{cleaned}\n\n## Sources\n\n{sources_md}\n"),
                cited: cited.len() as u64,
                dropped,
                ungrounded,
            });
            step_next(&next, "filing the report")
        }

        // ── artifact: get an addressable id BEFORE anything writes a body ──
        ResearchStage::Artifact => {
            let Some(report) = cp.report.clone() else {
                let mut next = cp.clone();
                next.stage = ResearchStage::Synthesize;
                return step_next(&next, "writing the report");
            };
            // THE GUARD AGAINST TWO REPORTS. The checkpoint first (the
            // ordinary resume), then the LINK — which is written in the same
            // breath as the create, so an entry whose predecessor died after
            // creating but before checkpointing still finds the artifact it
            // made instead of making another one.
            let existing = match cp.artifact_id.clone() {
                Some(id) => Some(id),
                None => (deps.linked_artifact)(run_id.clone()).await?,
            };
            if let Some(existing) = existing {
                let mut next = cp.clone();
                next.artifact_id = Some(existing);
                next.stage = ResearchStage::Save;
                return step_next(&next, "saving the report");
            }
            stop_if_abandoned(ctx)?;
            let artifact_id = (deps.create_report)(CreateReportArgs {
                run_id: run_id.clone(),
                title: report.title,
                owner_user_id: input.owner_user_id.clone(),
                requested_by: input.requested_by.clone(),
                agent_label: (deps.agent_label)(&input.agent_model),
            })
            .await?;
            let mut next = cp.clone();
            next.artifact_id = Some(artifact_id);
            next.stage = ResearchStage::Save;
            step_next(&next, "saving the report")
        }

        // ── save: four writes, every one of them keyed ─────────────────────
        ResearchStage::Save => {
            let (Some(report), Some(artifact_id)) = (cp.report.clone(), cp.artifact_id.clone())
            else {
                let mut next = cp.clone();
                next.stage = if cp.report.is_some() {
                    ResearchStage::Artifact
                } else {
                    ResearchStage::Synthesize
                };
                return step_next(&next, "filing the report");
            };
            stop_if_abandoned(ctx)?;
            // Org-ness from the row, not from the owner: an org-agent run
            // publishes org-wide even though the ladder stamps the human it
            // answers as the run's owner.
            let org_run = (deps.org_run)(run_id.clone()).await?;
            let member_ids = if input.owner_user_id.is_some() && !org_run {
                (deps.member_ids)(run_id.clone()).await?
            } else {
                Vec::new()
            };
            (deps.write_report)(WriteReportArgs {
                artifact_id: artifact_id.clone(),
                body: report.body.clone(),
                owner_user_id: input.owner_user_id.clone(),
                org_run,
                agent_label: (deps.agent_label)(&input.agent_model),
                agent_model: input.agent_model.clone(),
                member_ids,
            })
            .await?;
            (deps.save_sources)(run_id.clone(), cp.sources.clone()).await?;
            (deps.finish_row)(FinishRowArgs {
                run_id: run_id.clone(),
                artifact_id: artifact_id.clone(),
                stats: json!({
                    "queries": cp.queries_run,
                    "sources": cp.sources.len(),
                    "cited": report.cited,
                    "dropped": report.dropped,
                    "ungrounded": report.ungrounded,
                }),
            })
            .await?;
            let mut next = cp.clone();
            next.stage = ResearchStage::Publish;
            step_next(&next, "publishing")
        }

        // ── publish: the last step, and deliberately the smallest ──────────
        ResearchStage::Publish => {
            let (Some(report), Some(artifact_id)) = (cp.report.clone(), cp.artifact_id.clone())
            else {
                return Ok(StepResult::Done {
                    result: json!({
                        "artifactId": cp.artifact_id,
                        "sources": cp.sources.len(),
                    }),
                });
            };
            stop_if_abandoned(ctx)?;
            // Indexing first because it is keyed on (sourceType, sourceId) and
            // hashes its content: a repeat is a no-op. The BELL is not keyed,
            // so it goes last and nothing follows it but the terminal write.
            // Org-ness re-derived here, not carried on the checkpoint — the
            // answer rides no serialized state, so a run resumed across a
            // deploy resolves it the way the start would now.
            let org_run = (deps.org_run)(run_id.clone()).await?;
            (deps.index)(IndexArgs {
                run_id: run_id.clone(),
                artifact_id: artifact_id.clone(),
                title: report.title.clone(),
                body: report.body.clone(),
                question: input.question.clone(),
                mode: input.mode,
                owner_user_id: input.owner_user_id.clone(),
                org_run,
            })
            .await?;
            if let Some(owner) = &input.owner_user_id
                && let Err(e) = (deps.notify)(NotifyArgs {
                    owner_user_id: owner.clone(),
                    run_id: run_id.clone(),
                    title: report.title.clone(),
                    agent_label: (deps.agent_label)(&input.agent_model),
                    mode: input.mode,
                    sources: cp.sources.len() as u64,
                })
                .await
            {
                tracing::error!(
                    "[runs/research] {run_id}: the report is saved but the notification failed: {e}"
                );
            }
            Ok(StepResult::Done {
                result: json!({
                    "artifactId": artifact_id,
                    "sources": cp.sources.len(),
                    "queries": cp.queries_run,
                }),
            })
        }
    }
}

/// Put the run's failure on the DOMAIN record too, then let it fail.
///
/// Not a second source of truth for whether the run is alive — the domain
/// module projects that from the `runs` row — but `research_runs.status` is
/// what /api/research's duplicate-question check reads in raw SQL, and a
/// question whose last run failed must be askable again.
///
/// NOT ON AN ABANDONED STEP. An aborted step is this driver being taken off
/// the run — by a lost lease or a deploy — and the run is very probably about
/// to be resumed by somebody else. Writing `error` there would put a failure
/// on the record of a run that is still working.
async fn mirror_failure(ctx: &RunStepContext, deps: &ResearchRunDeps, message: &str) {
    if ctx.signal.is_aborted() {
        return;
    }
    if let Err(write) = (deps.fail_row)(ctx.run.id.clone(), message.to_string()).await {
        tracing::error!(
            "[runs/research] {}: could not record \"{message}\" on the research row: {write}",
            ctx.run.id
        );
    }
}

async fn research_step(ctx: RunStepContext, deps: &ResearchRunDeps) -> Result<StepResult, String> {
    let input: ResearchInput =
        serde_json::from_value(ctx.input.clone()).map_err(|e| format!("research input: {e}"))?;
    // begin runs when there is no checkpoint at all.
    if ctx.checkpoint.is_null() {
        return begin(&ctx, deps, &input).await;
    }
    let cp: ResearchCheckpoint = serde_json::from_value(ctx.checkpoint.clone())
        .map_err(|e| format!("research checkpoint: {e}"))?;
    match advance(&ctx, deps, &input, &cp).await {
        Ok(r) => Ok(r),
        Err(e) => {
            mirror_failure(&ctx, deps, &e).await;
            Err(e)
        }
    }
}

/// The first step: write the domain row if it is missing, resolve the search
/// model and the depth budget. NO billed call. Split from `advance` because it
/// is the one step that takes no checkpoint — and the one whose failures
/// mirror to the domain row like any other.
async fn begin(
    ctx: &RunStepContext,
    deps: &ResearchRunDeps,
    input: &ResearchInput,
) -> Result<StepResult, String> {
    let result = async {
        (deps.ensure_row)(&ctx.run.id, input.clone()).await?;
        let Some(plan) = (deps.search_plan_for)(input.mode).await else {
            // The same refusal `start_research` makes up front, restated here
            // because a reclaimed run is entered without going through it —
            // and a gateway that lost its only proven search path between the
            // click and the resume is a real state.
            return Err(NO_SEARCH_REASON.to_string());
        };
        let search_model = plan.model.clone();
        let budget = adapt_budget(budget_for(input.mode), &search_model);
        // A FOLLOW-UP CONTINUES ITS PARENT'S NUMBERING. Seeded here, into the
        // checkpoint, so [1]..[n] keep meaning what the parent's
        // already-written prose says they mean and anything new starts above
        // the highest — and so that a reclaim rebuilds the same registry
        // rather than restarting at [1] and silently re-aiming every citation
        // a human already read.
        let parent_sources = match &input.parent_run_id {
            Some(pid) => (deps.sources_of)(pid.clone()).await?,
            None => Vec::new(),
        };
        let (stage, plan_round) = first_round(input);
        let checkpoint = ResearchCheckpoint {
            stage,
            search_model,
            search_supplier: plan.supplier,
            rounds: budget.rounds,
            per_round: budget.queries,
            round: 1,
            done: 0,
            queries_run: 0,
            plan: plan_round,
            findings: Vec::new(),
            sources: parent_sources,
            search_failed: false,
            retries: 0,
            report: None,
            artifact_id: None,
        };
        step_next(&checkpoint, "starting")
    }
    .await;
    match result {
        Ok(r) => Ok(r),
        Err(e) => {
            mirror_failure(ctx, deps, &e).await;
            Err(e)
        }
    }
}

/// WHO MAY DECIDE, and it is the run's owner rather than its members.
///
/// Research is owner-scoped: the owner gets the run and everyone else a read
/// through `research_members` or through the run having no owner at all.
/// MEMBERS ARE DELIBERATELY NOT IN HERE — `audience` is synchronous (called
/// from inside the approvals census, on a row) so it cannot query the
/// membership table, and a membership list copied onto the run's input at
/// enqueue time would be a stale copy of a thing people are added to while a
/// run is in flight.
///
/// AN ORG RUN (no owner — a general agent researching for the workspace) goes
/// to the admins. That is a NARROWING and not a widening: an ownerless run is
/// already readable by anyone signed in, so its question is not a disclosure
/// to the admins, and the admins are the people who can act on "the search
/// models are not answering".
fn audience(run: &RunRow) -> Authority {
    match &run.owner_user_id {
        Some(owner) => Authority::User {
            user_ids: vec![owner.clone()],
        },
        None => Authority::Admin { on_board: None },
    }
}

/// The real step deps. They need the AppState, which the boot wiring owns, so
/// they are installed separately from registration; an unarmed step is
/// the loud refusal below — reached only by a driver armed before its deps,
/// which is a wiring bug.
static ARMED_DEPS: OnceLock<ResearchRunDeps> = OnceLock::new();

pub fn arm_research_step(deps: ResearchRunDeps) {
    let _ = ARMED_DEPS.set(deps);
}

/// The registered definition, exactly once per process — registered on the
/// first call, which `start_research`
/// makes before any enqueue, so the row's kind is always registered before it
/// is written. The returned `&'static Arc` is the same one the registry holds.
pub fn research_run() -> &'static Arc<RunDefinition> {
    static DEF: OnceLock<Arc<RunDefinition>> = OnceLock::new();
    DEF.get_or_init(|| {
        register_run(RunDefinition {
            kind: RESEARCH_RUN_KIND.into(),
            label: "Research".into(),
            step: Arc::new(|ctx| {
                Box::pin(async move {
                    let Some(deps) = ARMED_DEPS.get().cloned() else {
                        return Err(
                            "research steps are armed by the scheduler's boot wiring; this \
                             step was reached by a driver armed before its deps were"
                                .into(),
                        );
                    };
                    research_step(ctx, &deps).await
                })
            }),
            audience: Arc::new(audience),
            // ONE STAGE'S BUDGET, and it has to be bigger than the transport's
            // worst case rather than a guess at the typical one: a step that
            // blows this is filed as an error and NOT retried (the step is
            // probably still running, and re-entering would put two copies in
            // flight). The ceiling underneath it is the gateway's own timeout,
            // ten minutes, and a search against a deep-research-class model
            // genuinely takes minutes.
            //
            // THE PRICE, stated: this is also the lease TTL, so a research run
            // whose driver was killed mid-step is reclaimable about eleven
            // minutes later (plus one 30s sweep). That is the cost of never
            // abandoning a step that might still be spending — measured
            // against the behavior it replaces, which was to wait forty-five
            // minutes and then mark the run failed.
            max_step_ms: 11 * 60_000,
            max_attempts: DEFAULT_MAX_ATTEMPTS,
        })
    })
}

#[cfg(test)]
mod tests {
    // What a research run is worth is what it keeps when the process dies, so
    // that is what this file measures: every test below kills a driver
    // somewhere and then asks what the resumed run PAID for and what it
    // PRODUCED.
    //
    // THE DRIVER IS SIMULATED, not mocked. `research_step` is the definition's
    // whole contract with runs/run.rs, and the two rules that make an
    // assertion here mean anything are the two the real driver follows: a
    // `next` result's checkpoint is what the NEXT entry is given, and `killAt`
    // drops one on the floor — exactly what a crash between the step and the
    // checkpoint write does, and how the at-least-once cost is
    // stated as a number rather than as a paragraph.
    use super::*;
    use crate::runs::define::{RunState, StepSignal};

    // ── The fake world ──────────────────────────────────────────────────────

    #[derive(Default)]
    struct World {
        /// Every query actually sent to a search model, in order. Its LENGTH
        /// is the bill: one entry is one paid sonar call.
        searched: Vec<String>,
        /// The supplier each search call was handed — the plan's path, threaded
        /// through the checkpoint. None entries are native searches.
        suppliers: Vec<Option<Supplier>>,
        planned: u32,
        synthesized: u32,
        /// Artifacts created. More than one is the failure this file exists to
        /// make impossible.
        created: Vec<String>,
        /// Bodies written, (artifact id, body).
        written: Vec<(String, String)>,
        /// The org flag each report write was handed — one entry per run.
        report_org: Vec<bool>,
        /// How many times the member-grant list was fetched. An org run never
        /// fetches it; a personal run does.
        member_fetches: u32,
        /// What the org_run edge answers — seeded from the spec, so the fake
        /// stays one thing.
        org_answer: bool,
        indexed: u32,
        notified: u32,
        finished: Vec<(String, Value)>,
        failed: Vec<String>,
        sources_saved: Vec<Vec<ResearchSource>>,
        /// A follow-up's parent sources, seeded into the registry at `begin`.
        parent_sources: Vec<ResearchSource>,
        /// The artifact_links row, which is what makes a created artifact
        /// findable by the next entry.
        link: Option<String>,
        row_exists: bool,
        /// Queries whose search should throw, by query text.
        dead_queries: HashSet<String>,
        all_searches_dead: bool,
        /// With all_searches_dead: how many searches stay dead before the
        /// outage passes. None = dead for the whole run.
        revive_after: Option<u32>,
        artifact_seq: u32,
    }

    /// The per-test overrides, so the world fake itself stays one thing.
    struct WorldSpec {
        plan_model: &'static str,
        plan_supplier: Option<Supplier>,
        empty_plan: bool,
        synthesize_error: Option<String>,
        /// What the row would say: an org agent started this run.
        org_run: bool,
        /// Fired inside the search fake before it errs — the lost-lease case.
        abort_tx: Option<tokio::sync::watch::Sender<bool>>,
    }

    impl Default for WorldSpec {
        fn default() -> Self {
            WorldSpec {
                plan_model: "sonar-pro",
                plan_supplier: None,
                empty_plan: false,
                synthesize_error: None,
                org_run: false,
                abort_tx: None,
            }
        }
    }

    fn make_world(spec: WorldSpec) -> (Arc<std::sync::Mutex<World>>, ResearchRunDeps) {
        let WorldSpec {
            plan_model,
            plan_supplier,
            empty_plan,
            synthesize_error,
            org_run,
            abort_tx,
        } = spec;
        let w: Arc<std::sync::Mutex<World>> = Arc::new(std::sync::Mutex::new(World {
            row_exists: true,
            org_answer: org_run,
            ..Default::default()
        }));

        let deps = ResearchRunDeps {
            search_plan_for: Arc::new(move |_mode| {
                let plan = SearchPlan {
                    model: plan_model.to_string(),
                    via: if plan_supplier.is_some() {
                        SearchVia::Tool
                    } else {
                        SearchVia::Native
                    },
                    supplier: plan_supplier.clone(),
                };
                Box::pin(async move { Some(plan) })
            }),
            plan_queries: {
                let w = w.clone();
                Arc::new(move |args: PlanQueriesArgs| {
                    let w = w.clone();
                    Box::pin(async move {
                        let mut g = w.lock().expect("the world is never held across an await");
                        g.planned += 1;
                        if empty_plan {
                            return Vec::new();
                        }
                        (0..args.max)
                            .map(|i| format!("angle {}.{}", g.planned, i + 1))
                            .collect()
                    })
                })
            },
            search: {
                let w = w.clone();
                Arc::new(move |args: SearchArgs| {
                    let w = w.clone();
                    let abort_tx = abort_tx.clone();
                    Box::pin(async move {
                        let mut g = w.lock().expect("the world is never held across an await");
                        g.searched.push(args.query.clone());
                        g.suppliers.push(args.supplier.clone());
                        if let Some(tx) = &abort_tx {
                            let _ = tx.send(true);
                            return Err("aborted".into());
                        }
                        // `revive_after`: a transient outage — dead for the
                        // first N searches of the run, healthy from the next
                        // one on. What the retry has to survive.
                        let n = g.searched.len() as u32;
                        let dead = g.all_searches_dead && g.revive_after.is_none_or(|r| n <= r);
                        if dead || g.dead_queries.contains(&args.query) {
                            return Err(format!("search stage 502 on \"{}\"", args.query));
                        }
                        Ok(SearchHit {
                            content: format!("findings for {} [1]", args.query),
                            sources: vec![SourceSeed {
                                url: format!("https://example.com/{}", args.query),
                                title: Some(args.query.clone()),
                                snippet: Some("s".into()),
                            }],
                        })
                    })
                })
            },
            synthesize: {
                let w = w.clone();
                Arc::new(move |_args: SynthesizeArgs| {
                    let w = w.clone();
                    let err = synthesize_error.clone();
                    Box::pin(async move {
                        let mut g = w.lock().expect("the world is never held across an await");
                        g.synthesized += 1;
                        if let Some(e) = err {
                            return Err(e);
                        }
                        Ok(SynthOutcome {
                            doc: "# A report\n\nthe vendor published a SOC 2 Type II [1]".into(),
                            ungrounded: 0,
                        })
                    })
                })
            },
            agent_label: Arc::new(|m: &str| m.to_string()),
            ensure_row: {
                let w = w.clone();
                Arc::new(move |_run_id: &str, _input: ResearchInput| {
                    let w = w.clone();
                    Box::pin(async move {
                        w.lock()
                            .expect("the world is never held across an await")
                            .row_exists = true;
                        Ok(())
                    })
                })
            },
            sources_of: {
                let w = w.clone();
                Arc::new(move |_run_id: String| {
                    let w = w.clone();
                    Box::pin(async move {
                        Ok(w.lock()
                            .expect("the world is never held across an await")
                            .parent_sources
                            .clone())
                    })
                })
            },
            row_exists: {
                let w = w.clone();
                Arc::new(move |_run_id: String| {
                    let w = w.clone();
                    Box::pin(async move {
                        Ok(w.lock()
                            .expect("the world is never held across an await")
                            .row_exists)
                    })
                })
            },
            member_ids: {
                let w = w.clone();
                Arc::new(move |_run_id: String| {
                    let w = w.clone();
                    Box::pin(async move {
                        w.lock()
                            .expect("the world is never held across an await")
                            .member_fetches += 1;
                        Ok(vec!["member-1".to_string()])
                    })
                })
            },
            org_run: {
                let w = w.clone();
                Arc::new(move |_run_id: String| {
                    let w = w.clone();
                    Box::pin(async move {
                        Ok(w
                            .lock()
                            .expect("the world is never held across an await")
                            .org_answer)
                    })
                })
            },
            save_sources: {
                let w = w.clone();
                Arc::new(move |_run_id: String, sources: Vec<ResearchSource>| {
                    let w = w.clone();
                    Box::pin(async move {
                        w.lock()
                            .expect("the world is never held across an await")
                            .sources_saved
                            .push(sources);
                        Ok(())
                    })
                })
            },
            finish_row: {
                let w = w.clone();
                Arc::new(move |args: FinishRowArgs| {
                    let w = w.clone();
                    Box::pin(async move {
                        w.lock()
                            .expect("the world is never held across an await")
                            .finished
                            .push((args.artifact_id, args.stats));
                        Ok(())
                    })
                })
            },
            fail_row: {
                let w = w.clone();
                Arc::new(move |_run_id: String, error: String| {
                    let w = w.clone();
                    Box::pin(async move {
                        w.lock()
                            .expect("the world is never held across an await")
                            .failed
                            .push(error);
                        Ok(())
                    })
                })
            },
            linked_artifact: {
                let w = w.clone();
                Arc::new(move |_run_id: String| {
                    let w = w.clone();
                    Box::pin(async move {
                        Ok(w.lock()
                            .expect("the world is never held across an await")
                            .link
                            .clone())
                    })
                })
            },
            create_report: {
                let w = w.clone();
                Arc::new(move |_args: CreateReportArgs| {
                    let w = w.clone();
                    Box::pin(async move {
                        let mut g = w.lock().expect("the world is never held across an await");
                        g.artifact_seq += 1;
                        let id = format!("art-{}", g.artifact_seq);
                        g.created.push(id.clone());
                        // The real `create_report` links in the same breath as
                        // it creates, which is what makes the id addressable by
                        // the next entry. The fake does the same, or the "two
                        // artifacts" test would be testing the fake.
                        g.link = Some(id.clone());
                        Ok(id)
                    })
                })
            },
            write_report: {
                let w = w.clone();
                Arc::new(move |args: WriteReportArgs| {
                    let w = w.clone();
                    Box::pin(async move {
                        let mut g =
                            w.lock().expect("the world is never held across an await");
                        g.written.push((args.artifact_id, args.body));
                        g.report_org.push(args.org_run);
                        Ok(())
                    })
                })
            },
            index: {
                let w = w.clone();
                Arc::new(move |_args: IndexArgs| {
                    let w = w.clone();
                    Box::pin(async move {
                        w.lock()
                            .expect("the world is never held across an await")
                            .indexed += 1;
                        Ok(())
                    })
                })
            },
            notify: {
                let w = w.clone();
                Arc::new(move |_args: NotifyArgs| {
                    let w = w.clone();
                    Box::pin(async move {
                        w.lock()
                            .expect("the world is never held across an await")
                            .notified += 1;
                        Ok(())
                    })
                })
            },
        };
        (w, deps)
    }

    fn input(mode: ResearchDepth) -> ResearchInput {
        ResearchInput {
            question: "what changed in postgres 17".into(),
            mode,
            agent_model: "nomad".into(),
            owner_user_id: Some("user-1".into()),
            requested_by: "user-1".into(),
            parent_run_id: None,
        }
    }

    fn row_for(input: &ResearchInput) -> RunRow {
        RunRow {
            id: "run-1".into(),
            kind: RESEARCH_RUN_KIND.into(),
            owner_user_id: input.owner_user_id.clone(),
            subject_type: Some("research".into()),
            subject_id: Some("run-1".into()),
            state: RunState::Running,
            phase: "queued".into(),
            checkpoint: Value::Null,
            input: serde_json::to_value(input).expect("the test input serializes"),
            result: Value::Null,
            error: None,
            attempt: 0,
            lease_owner: Some("token".into()),
            lease_expires_at: None,
            approval_key: None,
            decision: None,
            created_at: "2026-08-06T00:00:00.000Z".into(),
            updated_at: "2026-08-06T00:00:00.000Z".into(),
            started_at: Some("2026-08-06T00:00:00.000Z".into()),
            finished_at: Some("2026-08-06T00:00:00.000Z".into()),
        }
    }

    struct DriveOutcome {
        steps: u32,
        result: Option<Value>,
        error: Option<String>,
    }

    /// The driver's loop, honestly: persist the checkpoint, then the next step.
    async fn drive_run(
        deps: &ResearchRunDeps,
        input: &ResearchInput,
        kill_at: Option<u32>,
    ) -> DriveOutcome {
        let mut checkpoint: Value = Value::Null;
        let mut attempt: i32 = 0;
        let mut steps: u32 = 0;
        let log: Arc<dyn Fn(String) + Send + Sync> = Arc::new(|_| ());
        loop {
            assert!(
                steps < 200,
                "the run did not stop after 200 steps — the budget does not bound the loop"
            );
            steps += 1;
            let mut row = row_for(input);
            row.attempt = attempt;
            let ctx = RunStepContext {
                run: row,
                input: serde_json::to_value(input).expect("the test input serializes"),
                checkpoint: checkpoint.clone(),
                decision: None,
                signal: StepSignal::channel().1,
                log: log.clone(),
                attempt,
            };
            match research_step(ctx, deps).await {
                Err(e) => {
                    return DriveOutcome {
                        steps,
                        result: None,
                        error: Some(e),
                    };
                }
                Ok(StepResult::Done { result }) => {
                    return DriveOutcome {
                        steps,
                        result: Some(result),
                        error: None,
                    };
                }
                Ok(StepResult::Next { checkpoint: cp, .. }) => {
                    if kill_at == Some(steps) {
                        // THE CRASH. The checkpoint never landed, so the next
                        // driver re-enters with the one before it — and
                        // `attempt` moves, which is the only thing that tells
                        // the step it has been here before.
                        attempt += 1;
                    } else {
                        checkpoint = cp;
                    }
                }
                other => panic!("research does not park or defer: {other:?}"),
            }
        }
    }

    fn stats_sources(g: &World) -> u64 {
        g.finished[0].1["sources"]
            .as_u64()
            .expect("sources is a number")
    }

    // ── The budgets still bound the loop ────────────────────────────────────

    #[tokio::test]
    async fn recon_asks_the_question_itself_one_search_no_planner_call() {
        let (w, deps) = make_world(WorldSpec::default());
        let out = drive_run(&deps, &input(ResearchDepth::Recon), None).await;
        assert!(out.error.is_none());
        let g = w.lock().unwrap();
        assert_eq!(g.planned, 0);
        assert_eq!(g.searched, ["what changed in postgres 17"]);
    }

    #[tokio::test]
    async fn brief_plans_once_and_runs_three_queries() {
        let (w, deps) = make_world(WorldSpec::default());
        let out = drive_run(&deps, &input(ResearchDepth::Brief), None).await;
        assert!(out.error.is_none());
        let g = w.lock().unwrap();
        assert_eq!(g.planned, 1);
        assert_eq!(g.searched.len(), 3);
    }

    #[tokio::test]
    async fn expedition_runs_three_rounds_of_four_and_stops_on_its_own() {
        let (w, deps) = make_world(WorldSpec::default());
        let out = drive_run(&deps, &input(ResearchDepth::Expedition), None).await;
        assert!(out.error.is_none());
        let g = w.lock().unwrap();
        assert_eq!(g.planned, 3);
        assert_eq!(g.searched.len(), 12);
        assert_eq!(g.synthesized, 1);
    }

    #[tokio::test]
    async fn a_deep_research_search_model_shrinks_the_loop() {
        let (w, deps) = make_world(WorldSpec {
            plan_model: "perplexity/sonar-deep-research",
            ..Default::default()
        });
        let out = drive_run(&deps, &input(ResearchDepth::Expedition), None).await;
        assert!(out.error.is_none());
        let g = w.lock().unwrap();
        // rounds min(3,2) = 2, one query each.
        assert_eq!(g.planned, 2);
        assert_eq!(g.searched.len(), 2);
    }

    #[tokio::test]
    async fn a_tool_plan_carries_its_supplier_to_every_search_call() {
        // The regression this pins: `plan_search` could say `via: 'tool'` and
        // the run used to throw that half away, posting a bare completion at a
        // model chosen precisely because it CANNOT search. The supplier
        // resolves once at `begin` and rides the checkpoint, so every query of
        // every round takes the same path — including the queries a reclaim
        // re-runs.
        let (w, deps) = make_world(WorldSpec {
            plan_model: "deepseek/deepseek-v4-flash",
            plan_supplier: Some(Supplier {
                server: "talaria".into(),
                tool: "web_search".into(),
            }),
            ..Default::default()
        });
        let out = drive_run(&deps, &input(ResearchDepth::Expedition), None).await;
        assert!(out.error.is_none());
        let g = w.lock().unwrap();
        assert_eq!(g.suppliers.len(), 12);
        assert!(g.suppliers.iter().all(|s| {
            s.as_ref()
                .is_some_and(|s| s.server == "talaria" && s.tool == "web_search")
        }));
    }

    #[tokio::test]
    async fn an_empty_plan_ends_the_round_loop_instead_of_spinning_it() {
        let (w, deps) = make_world(WorldSpec {
            empty_plan: true,
            ..Default::default()
        });
        let out = drive_run(&deps, &input(ResearchDepth::Expedition), None).await;
        // No sources and nothing more to search — the point here is that an
        // empty plan ends the ROUND LOOP instead of spinning it. The run then
        // retries the same empty round its two times (plan → synthesize, three
        // pairs) and ends on the no-sources sentence: 7 steps, not the 2 a
        // straight error would take.
        assert_eq!(out.error.as_deref(), Some(NO_SOURCES));
        assert_eq!(w.lock().unwrap().searched.len(), 0);
        assert_eq!(out.steps, 7);
    }

    // ── Resume ──────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn re_enters_mid_round_without_re_running_a_completed_query() {
        let (w, deps) = make_world(WorldSpec::default());
        // Step 1 is `begin`, 2 is `plan`, 3/4/5 are the three searches. Killing
        // the checkpoint AFTER the second search is the deploy landing
        // mid-round.
        let out = drive_run(&deps, &input(ResearchDepth::Brief), Some(4)).await;
        assert!(out.error.is_none());
        let g = w.lock().unwrap();
        // Three distinct angles, and the ONE re-billed call is the query that
        // was in flight when the process died — not the round, and not the run.
        assert_eq!(
            g.searched,
            ["angle 1.1", "angle 1.2", "angle 1.2", "angle 1.3"]
        );
        assert_eq!(g.planned, 1);
        // And the report still cites every source exactly once: the registry is
        // rebuilt from the checkpoint, so the resumed run does not renumber.
        assert_eq!(g.finished.len(), 1);
        assert_eq!(stats_sources(&g), 3);
    }

    #[tokio::test]
    async fn a_restart_is_not_an_error_anywhere() {
        for kill_at in 1..=8 {
            let (w, deps) = make_world(WorldSpec::default());
            let out = drive_run(&deps, &input(ResearchDepth::Brief), Some(kill_at)).await;
            // The sentence this whole port exists to delete has no path left to
            // reach: nothing wrote a failure on the research record, and the
            // run finished with a report.
            assert!(out.error.is_none(), "killed after step {kill_at}");
            let g = w.lock().unwrap();
            assert!(g.failed.is_empty(), "killed after step {kill_at}");
            assert_eq!(g.finished.len(), 1, "killed after step {kill_at}");
        }
    }

    #[tokio::test]
    async fn a_resumed_expedition_re_searches_one_query_not_the_run() {
        let (w, deps) = make_world(WorldSpec::default());
        let out = drive_run(&deps, &input(ResearchDepth::Expedition), Some(9)).await;
        assert!(out.error.is_none());
        // Twelve queries plus exactly one repeat.
        assert_eq!(w.lock().unwrap().searched.len(), 13);
    }

    // ── The report artifact ─────────────────────────────────────────────────
    //
    // A recon run's steps, in order: 1 begin, 2 search, 3 plan (the round is
    // over, so this one only advances it), 4 synthesize, 5 artifact, 6 save,
    // 7 publish.

    #[tokio::test]
    async fn a_killed_synthesis_does_not_produce_two_artifacts() {
        let (w, deps) = make_world(WorldSpec::default());
        let out = drive_run(&deps, &input(ResearchDepth::Recon), Some(4)).await;
        assert!(out.error.is_none());
        let g = w.lock().unwrap();
        assert_eq!(g.synthesized, 2); // re-billed — the declared at-least-once cost
        assert_eq!(g.created.len(), 1);
        assert_eq!(g.written.len(), 1);
        assert_eq!(g.finished[0].0, "art-1");
    }

    #[tokio::test]
    async fn a_crash_between_creating_and_checkpointing_still_writes_one_report() {
        // THE window: the artifact exists and no checkpoint says so. The link
        // is what the next entry finds it by.
        let (w, deps) = make_world(WorldSpec::default());
        let out = drive_run(&deps, &input(ResearchDepth::Recon), Some(5)).await;
        assert!(out.error.is_none());
        let g = w.lock().unwrap();
        assert_eq!(g.created.len(), 1);
        assert_eq!(g.written.len(), 1);
        assert_eq!(g.written[0].0, "art-1");
    }

    #[tokio::test]
    async fn reuses_the_artifact_a_previous_entry_linked_even_with_no_checkpoint() {
        // The worst version: the process died between `create_artifact` and the
        // link's own statement never mattered because the link IS how the next
        // entry finds it. A run resumed against a link it did not write reuses
        // it.
        let (w, deps) = make_world(WorldSpec::default());
        w.lock().unwrap().link = Some("art-from-a-dead-driver".into());
        let out = drive_run(&deps, &input(ResearchDepth::Recon), None).await;
        assert!(out.error.is_none());
        let g = w.lock().unwrap();
        assert_eq!(g.created.len(), 0);
        assert_eq!(g.written[0].0, "art-from-a-dead-driver");
    }

    #[tokio::test]
    async fn a_repeated_save_rewrites_the_same_body_not_a_second_report() {
        let (w, deps) = make_world(WorldSpec::default());
        drive_run(&deps, &input(ResearchDepth::Recon), Some(6)).await;
        let g = w.lock().unwrap();
        assert_eq!(g.created.len(), 1);
        assert_eq!(g.written.len(), 2);
        assert_eq!(g.written[0].1, g.written[1].1);
        assert_eq!(g.finished.len(), 2);
        assert_eq!(g.finished[0].0, g.finished[1].0);
    }

    #[tokio::test]
    async fn notifies_once_on_a_clean_run_and_the_bell_is_last() {
        let (w, deps) = make_world(WorldSpec::default());
        let out = drive_run(&deps, &input(ResearchDepth::Recon), None).await;
        assert!(out.error.is_none());
        let g = w.lock().unwrap();
        assert_eq!(g.indexed, 1);
        assert_eq!(g.notified, 1);
    }

    #[tokio::test]
    async fn an_owned_org_run_publishes_org_and_skips_the_personal_grants() {
        // The ladder stamps the chatting human as an org-agent run's owner, so
        // "owned" no longer implies "personal": the row's org flag rides the
        // report write (org reach, however owned), and the personal share
        // grants are never even fetched.
        let (w, deps) = make_world(WorldSpec {
            org_run: true,
            ..Default::default()
        });
        // The default input IS an owned run (owner user-1) — that is the
        // point: owned and org at once.
        let out = drive_run(&deps, &input(ResearchDepth::Brief), None).await;
        assert!(out.error.is_none());
        let g = w.lock().unwrap();
        assert_eq!(g.report_org, [true]);
        assert_eq!(g.member_fetches, 0);
    }

    #[tokio::test]
    async fn a_personal_run_fetches_its_share_grants() {
        let (w, deps) = make_world(WorldSpec::default());
        let out = drive_run(&deps, &input(ResearchDepth::Brief), None).await;
        assert!(out.error.is_none());
        let g = w.lock().unwrap();
        assert_eq!(g.report_org, [false]);
        assert_eq!(g.member_fetches, 1);
    }

    // ── Nothing citable: the run answers itself ─────────────────────────────
    // This suite was rewritten when the park was removed (2026-08-28, ticket
    // #5: two runs sat 'awaiting' for hours reading as working ones). The
    // harness retries by itself and then fails; nobody is ever asked.

    #[tokio::test]
    async fn retries_by_itself_nobody_is_asked_the_run_never_parks() {
        let (w, deps) = make_world(WorldSpec::default());
        w.lock().unwrap().all_searches_dead = true;
        let out = drive_run(&deps, &input(ResearchDepth::Recon), None).await;
        // One initial pass plus two retries — MAX_NO_SOURCE_RETRIES, then the
        // end.
        assert_eq!(out.error.as_deref(), Some(NO_SOURCES));
        let g = w.lock().unwrap();
        // A retried RECON re-searches its one query — it does not quietly
        // acquire the planning stage its mode does not have.
        assert_eq!(
            g.searched,
            [
                "what changed in postgres 17",
                "what changed in postgres 17",
                "what changed in postgres 17"
            ]
        );
        assert_eq!(g.planned, 0);
        // And the domain record carries the failure, so the question can be
        // asked again.
        assert_eq!(g.failed, [NO_SOURCES]);
    }

    #[tokio::test]
    async fn a_transient_outage_answers_itself_the_retry_round_completes_the_run() {
        let (w, deps) = make_world(WorldSpec::default());
        {
            let mut g = w.lock().unwrap();
            g.all_searches_dead = true;
            g.revive_after = Some(1); // dead for the first search only
        }
        let out = drive_run(&deps, &input(ResearchDepth::Recon), None).await;
        assert!(out.error.is_none());
        let g = w.lock().unwrap();
        assert_eq!(
            g.searched,
            ["what changed in postgres 17", "what changed in postgres 17"]
        );
        assert_eq!(stats_sources(&g), 1);
        assert!(g.written[0].1.contains("A report"));
    }

    #[tokio::test]
    async fn one_dead_query_costs_one_angle_not_the_run() {
        let (w, deps) = make_world(WorldSpec::default());
        w.lock().unwrap().dead_queries.insert("angle 1.2".into());
        let out = drive_run(&deps, &input(ResearchDepth::Brief), None).await;
        assert!(out.error.is_none());
        let g = w.lock().unwrap();
        assert_eq!(stats_sources(&g), 2);
        assert!(g.written[0].1.contains("A report"));
    }

    // ── The failure mirror, and who may decide ──────────────────────────────

    #[tokio::test]
    async fn a_failed_synthesis_lands_on_the_research_record_and_writes_no_artifact() {
        let (w, deps) = make_world(WorldSpec {
            synthesize_error: Some(
                "harness \"research-synthesis\" could not reach \"nomad\": persona gateway 502"
                    .into(),
            ),
            ..Default::default()
        });
        let out = drive_run(&deps, &input(ResearchDepth::Recon), None).await;
        assert!(out.result.is_none());
        let g = w.lock().unwrap();
        assert!(g.created.is_empty());
        assert!(g.written.is_empty());
        assert!(g.failed[0].contains("persona gateway 502"));
    }

    #[tokio::test]
    async fn stops_when_the_research_record_has_been_deleted() {
        let (w, deps) = make_world(WorldSpec::default());
        w.lock().unwrap().row_exists = false;
        let inp = input(ResearchDepth::Brief);
        let checkpoint = json!({
            "stage": "plan",
            "searchModel": "sonar",
            "searchSupplier": null,
            "rounds": 1,
            "perRound": 3,
            "round": 1,
            "plan": [],
            "done": 0,
            "queriesRun": 0,
            "findings": [],
            "sources": [],
            "searchFailed": false,
            "retries": 0,
            "report": null,
            "artifactId": null
        });
        let mut row = row_for(&inp);
        row.checkpoint = checkpoint.clone();
        let res = research_step(
            RunStepContext {
                run: row,
                input: serde_json::to_value(&inp).unwrap(),
                checkpoint,
                decision: None,
                signal: StepSignal::channel().1,
                log: Arc::new(|_| ()),
                attempt: 0,
            },
            &deps,
        )
        .await
        .unwrap();
        let StepResult::Done { result } = res else {
            panic!("a deleted record ends the run, got {res:?}");
        };
        assert_eq!(result, json!({ "deleted": true }));
        assert_eq!(w.lock().unwrap().planned, 0);
    }

    #[tokio::test]
    async fn asks_the_owner_and_the_admins_only_for_a_run_nobody_owns() {
        let row = row_for(&input(ResearchDepth::Brief));
        assert_eq!(
            audience(&row),
            Authority::User {
                user_ids: vec!["user-1".into()]
            }
        );
        let ownerless = RunRow {
            owner_user_id: None,
            ..row.clone()
        };
        assert_eq!(audience(&ownerless), Authority::Admin { on_board: None });
    }

    #[tokio::test]
    async fn an_abandoned_step_is_not_recorded_as_a_research_failure() {
        // A lost lease aborts the step; the run is not failed, another instance
        // resumes it. Writing `error` on the research record there would put a
        // failure on a run that is still working.
        let (tx, signal) = StepSignal::channel();
        let (w, deps) = make_world(WorldSpec {
            abort_tx: Some(tx),
            ..Default::default()
        });
        let inp = input(ResearchDepth::Recon);
        let checkpoint = json!({
            "stage": "search",
            "searchModel": "sonar",
            "searchSupplier": null,
            "rounds": 1,
            "perRound": 1,
            "round": 1,
            "plan": ["q"],
            "done": 0,
            "queriesRun": 0,
            "findings": [],
            "sources": [],
            "searchFailed": false,
            "retries": 0,
            "report": null,
            "artifactId": null
        });
        let mut row = row_for(&inp);
        row.checkpoint = checkpoint.clone();
        let res = research_step(
            RunStepContext {
                run: row,
                input: serde_json::to_value(&inp).unwrap(),
                checkpoint,
                decision: None,
                signal,
                log: Arc::new(|_| ()),
                attempt: 0,
            },
            &deps,
        )
        .await;
        assert!(res.is_err());
        assert!(w.lock().unwrap().failed.is_empty());
    }

    // ── Report grants ─────────────────────────────────────────────────────

    #[test]
    fn a_personal_report_grants_its_members_and_its_agent() {
        let grants = personal_report_grants(&["u-1".into()], "gregasaurus-personal");
        assert_eq!(grants.len(), 2, "the shared member and the run's agent");
        assert_eq!(grants[0].principal_type, "user");
        assert_eq!(grants[0].principal_id, "u-1");
        assert_eq!(grants[0].role, "editor");
        assert_eq!(grants[1].principal_type, "agent");
        assert_eq!(grants[1].principal_id, "gregasaurus-personal");
        assert_eq!(grants[1].role, "viewer");
    }

    #[test]
    fn an_unshared_personal_report_is_still_readable_by_its_agent() {
        // The gate's own vocabulary, so the grant and the gate cannot drift:
        // a private report with these grants opens for the run's agent and
        // for no other.
        let grants = personal_report_grants(&[], "gregasaurus-personal");
        let private = crate::kb::perms::Guarded {
            owner_user_id: None,
            created_by: None,
            visibility: "private".into(),
            edit_policy: "editors".into(),
        };
        assert!(crate::kb::perms::can_read_agent(
            &private,
            "gregasaurus-personal",
            None,
            &grants
        ));
        assert!(!crate::kb::perms::can_read_agent(
            &private,
            "leo-engineering",
            None,
            &grants
        ));
    }
}
