// Research runs — Perplexity-grade cited research, Talaria-native.
// Port of ui/src/server/research.ts.
//
// Three modes, mapped to depth budgets and sonar horsepower:
//   recon       one search pass, minutes — a cited answer
//   brief       planned queries + one synthesis round — a briefing document
//   expedition  iterative plan → search → gap-check rounds — a deep report
//
// THE PIPELINE ITSELF lives in runs/defs/research.rs — the durable run. What
// stays here is everything AROUND it: the projection the surfaces read, the
// sharing/membership writes, the start gate, the conversation a run is
// discussed in, and the report-extension helpers a follow-up's document is
// merged by.
//
// THE PROJECTION IS THE MODULE. `status` and `phase` are read off the RUN,
// not off the research record — one source of truth for whether a run is
// alive. The run row is the authority because it is the thing being driven;
// the research record's own columns are a TERMINAL outcome, and they are what
// this reads when there is no run at all, which is every row created before
// the port.

use std::collections::HashSet;
use std::sync::LazyLock;

use regex::Regex;
use serde::Serialize;
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

// RE-EXPORTED, not re-declared. `plan_search` and its companions moved into
// the run definition with the pipeline they belong to; the module that owns
// the work owns its resolution, and every caller keeps the import it had.
use crate::realtime::RealtimeDeps;
pub use crate::runs::defs::research::{
    ModeBudget, NO_SEARCH_REASON, SearchPlan, budget_for, plan_search, research_modes,
};
use crate::runs::defs::research::{ResearchInput, research_run};
use crate::runs::run::{EnqueueOptions, cancel_run, enqueue};
use crate::source_registry::{MARKER_RE, ResearchSource};
use crate::work_dispatch::dispatch_deps;

/// What the agent in a research conversation is for: answer from the report,
/// cite the same [n] markers, and never state as established anything the
/// sources do not support. Used by the chat plane when it crosses; declared
/// here because it is research's own contract, not chat's.
pub const RESEARCH_MODE_PROMPT: &str = "This is a conversation ABOUT a research report you produced, on the Research surface. Several teammates may be in it; the report and its numbered sources sit beside the chat.

Answer from the report and its sources first. Cite the same [n] markers the report uses so anyone can check you, and never state something as established that the sources do not support — this is a surface where everything looks cited, so an uncited claim of yours will be read as a finding.

When the answer genuinely is not in what was found, say so plainly and offer to look into it. If a teammate asks you to dig further, commission follow-up research with the research tool rather than answering from memory; its findings are added to the same report, so the document stays the one place the answer lives.

Do not re-research something the report already covers — say what it says and point at the section.";

// ── The report-side machinery ─────────────────────────────────────────────────
//
// The citation registry itself (`SourceRegistry`, `MARKER_RE`,
// `ResearchSource`) is in source_registry.rs, the one leaf both hosts share.
// What stays here consumes it.

/// THE SOURCES SECTION THE PIPELINE APPENDS, matched so it can be replaced.
///
/// Anchored to a line start and to the END of the document, because a report
/// may legitimately discuss the word "Sources" in its prose and a loose match
/// would truncate a report at the first mention of one. (The TS's trailing
/// `$` after `[\s\S]*` matches nothing further and is dropped here.)
static SOURCES_SECTION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)\n*^## Sources\s*$[\s\S]*").expect("compiles"));

/// The report's prose, without the mechanical source list.
pub fn report_body_only(body: &str) -> String {
    SOURCES_SECTION.replace(body, "").trim_end().to_string()
}

/// EXTEND A REPORT WITH WHAT A FOLLOW-UP FOUND, keeping it one document.
///
/// A follow-up used to produce a SECOND report about the same subject, with
/// its own source numbering, and nothing linking the two. The answer to one
/// question lived in two places and the reader assembled it.
///
/// WHAT THIS HAS TO GET RIGHT, and each one silently corrupts a document if
/// it does not:
///
///   THE OLD PROSE IS UNTOUCHED. Somebody has read it and may have quoted it.
///   THE SOURCE LIST IS REBUILT, not appended to — it is mechanical output,
///     and two of them at the bottom of one document is the failure that made
///     the old `## Sources` regex worth anchoring.
///   CITED IS RECOMPUTED OVER THE WHOLE DOCUMENT. A source the parent cites
///     and the follow-up does not must not become "(consulted)" because this
///     pass only looked at the new section.
///   THE NEW SECTION SAYS WHAT ASKED FOR IT. A reader coming back a week
///     later needs to see that the last three paragraphs answer a different
///     question than the top of the document does.
pub fn extend_report(
    parent_body: &str,
    question: &str,
    markdown: &str,
    sources: &[ResearchSource],
) -> String {
    let prose = report_body_only(parent_body);
    // The follow-up's own `# Title` is dropped: the document already has one,
    // and a second H1 mid-document reads as a new document to every renderer
    // and every table of contents.
    static H1: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^#\s+.*$").expect("compiles"));
    let section = H1.replace(markdown.trim(), "").trim().to_string();
    let heading = format!("## Follow-up: {}", question.trim());
    let merged = format!("{prose}\n\n{heading}\n\n{section}");
    let cited: HashSet<u64> = MARKER_RE
        .find_iter(&merged)
        .filter_map(|m| m.as_str()[1..m.as_str().len() - 1].parse().ok())
        .collect();
    let mut ordered = sources.to_vec();
    ordered.sort_by_key(|s| s.idx);
    let list = ordered
        .iter()
        .map(|s| {
            let label = s.title.as_deref().unwrap_or(&s.url);
            let consulted = if cited.contains(&s.idx) {
                ""
            } else {
                " *(consulted)*"
            };
            format!("{}. [{}]({}){}", s.idx, label, s.url, consulted)
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!("{merged}\n\n## Sources\n\n{list}\n")
}

/// KEEP ONLY THE CITATIONS THAT RESOLVE, and count what was thrown away.
///
/// `dropped` is COUNTED, not merely stripped. Deleting an invented citation
/// is the right thing to save, but it also made a model that fabricates half
/// its markers look identical to one that cites perfectly — the exact
/// model-fitness signal this run is in the best position to report, thrown
/// away by the line that fixed the symptom. (The durable run's synthesize
/// arm computes this inline from its own registry; this is the same rule in
/// the shape its test drives.)
pub fn strip_unknown_markers(doc: &str, known_idx: &[u64]) -> (String, u64, HashSet<u64>) {
    let known: HashSet<u64> = known_idx.iter().copied().collect();
    let dropped = MARKER_RE
        .find_iter(doc.trim())
        .filter(|m| {
            let n = &m.as_str()[1..m.as_str().len() - 1];
            n.parse::<u64>().is_ok_and(|n| !known.contains(&n))
        })
        .count() as u64;
    // Fences before markers, the TS order: a fence line is structural, and
    // stripping it first is what keeps the marker pass from ever seeing a
    // bracket the fence itself introduced.
    let unfenced = FENCE_CLEAN.replace_all(doc.trim(), "");
    let cleaned = MARKER_RE.replace_all(&unfenced, |c: &regex::Captures| {
        let m = c.get(0).expect("the whole match exists").as_str();
        match c.get(1).and_then(|g| g.as_str().parse::<u64>().ok()) {
            Some(n) if known.contains(&n) => m.to_string(),
            _ => String::new(),
        }
    });
    let cited: HashSet<u64> = MARKER_RE
        .find_iter(&cleaned)
        .filter_map(|m| m.as_str()[1..m.as_str().len() - 1].parse().ok())
        .collect();
    (cleaned.to_string(), dropped, cited)
}

/// The wrapping fence the TS strips in the same expression as the markers.
static FENCE_CLEAN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^```[a-z]*\n?|\n?```$").expect("compiles"));

// ── The projection ────────────────────────────────────────────────────────────

/// One research run as the surfaces read it (research.ts ResearchRun). The
/// wire is camelCase because these rows are also written by TS during
/// coexistence; the field set is the join below, verbatim.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResearchRun {
    pub id: String,
    pub owner_user_id: Option<String>,
    pub requested_by: String,
    pub agent_model: String,
    pub mode: String,
    pub question: String,
    pub title: Option<String>,
    /// 'queued' | 'running' | 'done' | 'error' — the four-value wire
    /// constraint; a parked run spells its open question in `awaiting`
    /// instead of needing a fifth value.
    pub status: String,
    pub phase: Option<String>,
    /// THE QUESTION A PARKED RUN IS WAITING ON, verbatim off the run's
    /// decision column. None unless the run is `awaiting` with no answer yet
    /// — a decided run never shows its last question as if it were still
    /// open. `status` still says 'running'; this field is what makes a
    /// parked run LOOK parked, and the research surface is where it gets
    /// answered.
    pub awaiting: Option<Value>,
    pub artifact_id: Option<String>,
    /// NOT on this wire shape: the run's conversation_id and parent_run_id
    /// exist as columns (and on the TS interface, as `string | null`), but no
    /// TS response carries them — the ROW projection never selects them, so
    /// JSON.stringify never emits the keys. The conversation id is served by
    /// its own route (`ensure_research_conversation`); the parent link is
    /// write-only. Emitting them as nulls here broke the list's byte-parity.
    pub error: Option<String>,
    pub stats: Value,
    pub created_at: String,
    pub updated_at: String,
    pub completed_at: Option<String>,
}

/// THE four-value projection, on its own so it has exactly one spelling. Two
/// reads need it in different shapes (a full row, and the briefing's filtered
/// subquery) and a second copy of this CASE is a second answer to "is this
/// run alive" — which is the whole thing this file is trying not to have.
///
/// `awaiting` maps to 'running' because the status field has four values, all
/// of them on the wire and in the client. A parked run is not idle — it is in
/// somebody's approvals queue with the question on it — and 'running' is the
/// honest four-value spelling of that. `cancelled` maps to 'error' the same
/// way, with the cancel's own reason carried in `error`.
const STATUS: &str = "case \
    when r.state is null then research_runs.status \
    when r.state in ('running', 'awaiting') then 'running' \
    when r.state = 'cancelled' then 'error' \
    when r.state = 'error' then 'error' \
    when r.state = 'done' then 'done' \
    else 'queued' end";

/// The full row, in the field order ResearchRun serializes. Timestamps come
/// back as epoch milliseconds and are spelled by `epoch_ms_to_iso` — the same
/// JS-ISO string the TS projection rendered its Dates into.
const ROW: &str = "research_runs.id::text, research_runs.owner_user_id::text, \
    research_runs.requested_by, research_runs.agent_model, research_runs.mode, \
    research_runs.question, research_runs.title, \
    {STATUS} as status, \
    case when r.state in ('queued', 'running', 'awaiting') then nullif(r.phase, '') \
         else research_runs.phase end as phase, \
    case when r.state = 'awaiting' and r.decision->'request' is not null \
              and r.decision->'answer' is null \
         then r.decision->'request' else null end as awaiting, \
    research_runs.artifact_id::text, \
    coalesce(research_runs.error, r.error) as error, \
    research_runs.stats, \
    (trunc(extract(epoch from research_runs.created_at) * 1000))::bigint, \
    (trunc(extract(epoch from greatest(research_runs.updated_at, \
        coalesce(r.updated_at, research_runs.updated_at))) * 1000))::bigint, \
    (trunc(extract(epoch from research_runs.completed_at) * 1000))::bigint";

/// Always joined, never selected from alone: the projection above needs `r`.
/// ONE RUN PER RESEARCH RECORD, AND THE SAME UUID NAMES BOTH — that is what
/// makes this a primary-key join rather than a scan for the newest run with
/// a matching subject.
const FROM: &str =
    "from research_runs left join runs r on r.id = research_runs.id and r.kind = 'research'";

fn projection_sql(where_clause: &str) -> String {
    // The `select` lives HERE, not in the callers: the query is one spelling,
    // and a caller-side prefix is how the list route came to ship a statement
    // starting "research_runs.id::text, …" — a syntax error at the first read.
    "select ".to_string() + &ROW.replace("{STATUS}", STATUS) + " " + FROM + " " + where_clause
}

/// Map one row of the projection. Written against column INDEX rather than
/// name: eighteen columns is past sqlx's tuple ceiling, and a name-keyed read
/// of a format!-built list would drift silently if the list reordered — an
/// index-keyed one fails loudly.
fn row_of(row: &sqlx::postgres::PgRow) -> ResearchRun {
    use sqlx::Row;
    // The epoch-millisecond columns are read into named locals: `get` infers
    // its index generic from the turbofish otherwise, and `get::<_, i64>`
    // binds i64 to the INDEX (usize-only) rather than the value.
    let created_ms: i64 = row.get(13);
    let updated_ms: i64 = row.get(14);
    let completed_ms: Option<i64> = row.get(15);
    ResearchRun {
        id: row.get(0),
        owner_user_id: row.get(1),
        requested_by: row.get(2),
        agent_model: row.get(3),
        mode: row.get(4),
        question: row.get(5),
        title: row.get(6),
        status: row.get(7),
        phase: row.get(8),
        awaiting: row.get(9),
        artifact_id: row.get(10),
        error: row.get(11),
        stats: row.get(12),
        created_at: crate::agent_auth::epoch_ms_to_iso(created_ms),
        updated_at: crate::agent_auth::epoch_ms_to_iso(updated_ms),
        completed_at: completed_ms.map(crate::agent_auth::epoch_ms_to_iso),
    }
}

/// Runs a viewer may see: their own, ones shared with them, and org runs
/// (no owner — general agents researching for the org). None viewer (a
/// general agent) sees org runs only.
pub async fn list_research_runs(
    pg: &PgPool,
    viewer_user_id: Option<&str>,
    limit: i64,
) -> Result<Vec<ResearchRun>, sqlx::Error> {
    if let Some(viewer) = viewer_user_id {
        let sql = projection_sql(
            "where research_runs.owner_user_id is null or research_runs.owner_user_id = $1::uuid \
             or exists(select 1 from research_members rm \
                       where rm.run_id = research_runs.id and rm.user_id = $1::uuid) \
             order by research_runs.created_at desc limit $2",
        );
        let rows = sqlx::query(sqlx::AssertSqlSafe(sql.as_str()))
            .bind(viewer)
            .bind(limit)
            .fetch_all(pg)
            .await?;
        Ok(rows.iter().map(row_of).collect())
    } else {
        let sql = projection_sql(
            "where research_runs.owner_user_id is null \
             order by research_runs.created_at desc limit $1",
        );
        let rows = sqlx::query(sqlx::AssertSqlSafe(sql.as_str()))
            .bind(limit)
            .fetch_all(pg)
            .await?;
        Ok(rows.iter().map(row_of).collect())
    }
}

/// The viewer's standing on a run: owner, member (incl. org runs), or none.
/// 'owner' | 'member' — None is "no standing", which every route spells as
/// its own not-found.
pub async fn research_role(
    pg: &PgPool,
    viewer_user_id: Option<&str>,
    run_id: &str,
) -> Result<Option<&'static str>, sqlx::Error> {
    let row: Option<(Option<String>, bool)> = sqlx::query_as(
        "select owner_user_id::text, \
                exists(select 1 from research_members rm \
                       where rm.run_id = research_runs.id and rm.user_id = $1::uuid) \
         from research_runs where id = $2::uuid",
    )
    .bind(viewer_user_id)
    .bind(run_id)
    .fetch_optional(pg)
    .await?;
    let Some((owner, member)) = row else {
        return Ok(None);
    };
    if owner.is_none() {
        return Ok(Some("member")); // org run — anyone signed in
    }
    if viewer_user_id.is_some_and(|v| owner.as_deref() == Some(v)) {
        return Ok(Some("owner"));
    }
    Ok(if member { Some("member") } else { None })
}

/// One member of a run (research.ts ResearchMember).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResearchMember {
    pub user_id: String,
    pub name: Option<String>,
    pub email: Option<String>,
    pub role: &'static str, // 'owner' | 'collaborator'
}

pub async fn list_research_members(
    pg: &PgPool,
    run_id: &str,
) -> Result<Vec<ResearchMember>, sqlx::Error> {
    let rows: Vec<(String, Option<String>, Option<String>, String)> = sqlx::query_as(
        "select u.id::text, u.name, u.email, 'owner' as role \
         from research_runs r join users u on u.id = r.owner_user_id where r.id = $1::uuid \
         union all \
         select u.id::text, u.name, u.email, 'collaborator' as role \
         from research_members rm join users u on u.id = rm.user_id where rm.run_id = $1::uuid",
    )
    .bind(run_id)
    .fetch_all(pg)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(user_id, name, email, role)| ResearchMember {
            user_id,
            name,
            email,
            role: if role == "owner" {
                "owner"
            } else {
                "collaborator"
            },
        })
        .collect())
}

pub async fn add_research_member(
    pg: &PgPool,
    run_id: &str,
    user_id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "insert into research_members (run_id, user_id) values ($1::uuid, $2::uuid) \
         on conflict do nothing",
    )
    .bind(run_id)
    .bind(user_id)
    .execute(pg)
    .await?;
    Ok(())
}

pub async fn remove_research_member(
    pg: &PgPool,
    run_id: &str,
    user_id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("delete from research_members where run_id = $1::uuid and user_id = $2::uuid")
        .bind(run_id)
        .bind(user_id)
        .execute(pg)
        .await?;
    Ok(())
}

/// The in-flight run for a question, if there is one — the double-click
/// guard.
///
/// IT ASKS THE RUN, which is the point of it existing. The research record's
/// own `status` is a TERMINAL outcome rather than a live state: it is written
/// when a run finishes or when a step records its own failure, and NOT when a
/// driver gives up on a run from the outside (attempts spent, a step over its
/// budget, a cancel). Left on the raw column, one of those would make a
/// question unaskable for ever — the record would still read 'queued' with
/// nothing driving it.
///
/// `awaiting` counts as in flight: a run parked on a question is very much
/// the same question, still open, waiting on the person about to ask it
/// twice.
pub async fn active_research_on(
    pg: &PgPool,
    question: &str,
) -> Result<Option<String>, sqlx::Error> {
    let row: Option<(String,)> = sqlx::query_as(
        "select research_runs.id::text from research_runs \
         left join runs r on r.id = research_runs.id and r.kind = 'research' \
         where research_runs.question = $1 \
           and case when r.state is null \
                    then research_runs.status in ('queued', 'running') \
                    else r.state in ('queued', 'running', 'awaiting') end \
         limit 1",
    )
    .bind(question)
    .fetch_optional(pg)
    .await?;
    Ok(row.map(|(id,)| id))
}

/// Research this person should be told about in an ambient briefing: what is
/// in flight, plus anything that failed in the last week. It exists so the
/// projection is spelled once — the briefing asked `research_runs` directly
/// once, and a stale column narrating a run nobody was driving is the same
/// class of bug as the sentence this port deleted.
pub async fn briefable_research(
    pg: &PgPool,
    user_id: &str,
    limit: i64,
) -> Result<Vec<(String, String, String)>, sqlx::Error> {
    let sql = format!(
        "select id::text, question, status from ( \
           select research_runs.id::text, \
                  coalesce(research_runs.title, research_runs.question) as question, \
                  {STATUS} as status, \
                  research_runs.created_at \
           {FROM} \
           where research_runs.owner_user_id = $1::uuid \
              or exists(select 1 from research_members rm \
                        where rm.run_id = research_runs.id and rm.user_id = $1::uuid) \
         ) s \
         where s.status in ('queued', 'running') \
            or (s.status = 'error' and s.created_at > now() - interval '7 days') \
         order by s.created_at desc limit $2"
    );
    let rows: Vec<(String, String, String)> = sqlx::query_as(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(user_id)
        .bind(limit)
        .fetch_all(pg)
        .await?;
    Ok(rows)
}

/// Throw the run away: STOP IT FIRST, then delete the record.
///
/// DELETE used to be one `delete from research_runs`, which under the port
/// stops nothing — the work lives on the `runs` row, and the driver holding
/// it goes on planning, searching and synthesizing a report for a record that
/// no longer exists. The run's own steps do check `row_exists` and stop, so
/// nothing was permanently wrong; the cost was up to one whole step (eleven
/// minutes, and a billed search inside it) spent on work somebody had just
/// thrown away.
///
/// THE ORDER IS THE POINT. `cancel_run` is a compare-and-set with no lease
/// predicate, so it lands from any instance and every subsequent write by the
/// driver that owns the run is refused — the stop is real before the row
/// goes. The other order leaves a window where the record is gone and the run
/// is still `running`, which is the state a reclaim sweep would happily
/// re-enter.
///
/// The report artifact SURVIVES, unchanged: deleting a run clears the queue
/// entry, not the knowledge. A cancel that cannot land is logged and the
/// delete proceeds — `missing` (a row from before the port) and `terminal`
/// (a finished run) are both perfectly normal, and neither is a reason to
/// leave the record behind.
pub async fn delete_research_run(
    state: &crate::state::AppState,
    run_id: &str,
) -> Result<(), sqlx::Error> {
    if let Ok(redis) = state.redis().await {
        let realtime = RealtimeDeps::publish_only(Some(redis.clone()));
        let deps = dispatch_deps(state.pg.clone(), redis, realtime);
        if let Err(e) = cancel_run(run_id, Some("the research run was deleted".into()), &deps).await
        {
            tracing::error!("[research] could not cancel {run_id} before deleting it: {e}");
        }
    }
    sqlx::query("delete from research_runs where id = $1::uuid")
        .bind(run_id)
        .execute(&state.pg)
        .await?;
    Ok(())
}

/// The run's report artifact (via artifact_links), if it exists yet.
pub async fn research_artifact_for(
    pg: &PgPool,
    run_id: &str,
) -> Result<Option<String>, sqlx::Error> {
    let row: Option<(String,)> = sqlx::query_as(
        "select artifact_id::text from artifact_links \
         where target_type = 'research' and target_id = $1 limit 1",
    )
    .bind(run_id)
    .fetch_optional(pg)
    .await?;
    Ok(row.map(|(id,)| id))
}

pub async fn get_research_run(
    pg: &PgPool,
    id: &str,
) -> Result<Option<(ResearchRun, Vec<ResearchSource>)>, sqlx::Error> {
    // NO SWEEP HERE, AND NONE IN THE LIST EITHER. A restart is not a failure;
    // no read of this table decides anything about a run's health any more,
    // and the event that used to produce an epitaph now produces a resume —
    // see runs/reclaim.rs, which is where that sweep went.
    let sql = projection_sql("where research_runs.id = $1::uuid");
    let row = sqlx::query(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(id)
        .fetch_optional(pg)
        .await?;
    let Some(row) = row else { return Ok(None) };
    let run = row_of(&row);
    let sources: Vec<(i32, String, Option<String>, Option<String>)> = sqlx::query_as(
        "select idx, url, title, snippet from research_sources \
         where run_id = $1::uuid order by idx asc",
    )
    .bind(id)
    .fetch_all(pg)
    .await?;
    Ok(Some((
        run,
        sources
            .into_iter()
            .map(|(idx, url, title, snippet)| ResearchSource {
                idx: idx.max(0) as u64,
                url,
                title,
                snippet,
            })
            .collect(),
    )))
}

// ── Start ─────────────────────────────────────────────────────────────────────

/// Create the run and start it. Returns the queued research record.
///
/// THE ORDER IS THE DURABILITY. The `runs` row goes in FIRST, because it is
/// the record that survives: it carries the question, the mode, the agent and
/// the owner on its `input`, so a process that dies between the two inserts
/// leaves something the reclaim sweep can pick up — and the run's first step
/// writes the research record from that input when it is not there. The other
/// order leaves the opposite: a research record nobody is driving, nothing to
/// reclaim, and no stale sweep left to notice it.
///
/// ONE RUN PER RESEARCH RECORD, AND ONE UUID NAMING BOTH. `enqueue`
/// deduplicates nothing above the row, so the id is passed in rather than
/// generated inside it: a caller that retried with the same id would collide
/// on the primary key instead of starting a second run doing the same work.
/// The duplicate-QUESTION check that stops a double click is /api/research's,
/// and it is unchanged.
///
/// SIGNATURE UNCHANGED, including the up-front refusal when the workspace
/// cannot search: that is a 400 the caller shows in the form, and turning it
/// into a run that fails a second later would be a worse answer to the same
/// question. The run's first step re-checks it, for the resume case.
///
/// ORDER (same as plan_drafts): enqueue with the inline drive — the run row,
/// its publish, and the drive; this process's scheduler advances the run,
/// and the reclaim sweep is the guarantee either way. (During coexistence
/// the enqueue was row-and-publish only, leaving the drive to the TS sweep;
/// that runtime left with the cutover.)
pub async fn start_research(
    state: &crate::state::AppState,
    input: ResearchInput,
) -> Result<ResearchRun, String> {
    // THE UP-FRONT GATE, and the sentence it throws is the exported one so
    // the route, the MCP tool and the run's own `begin` step all say the same
    // thing. It asks the PLAN, not just a model id: a workspace with no
    // proven search path — no native-searcher registered, no search backend
    // connected — must refuse here rather than pay a blind model to answer
    // from memory.
    if plan_search(&state.pg, input.mode).await.is_none() {
        return Err(NO_SEARCH_REASON.into());
    }
    let id = Uuid::new_v4().to_string();

    // The enqueue needs a live Redis for the lease and the publish — the run
    // row IS the feature here, so a start without it fails the start rather
    // than half-happening.
    let redis = state
        .redis()
        .await
        .map_err(|_| "the run could not be enqueued: redis is unavailable".to_string())?;
    let realtime = RealtimeDeps::publish_only(Some(redis.clone()));
    let deps = dispatch_deps(state.pg.clone(), redis, realtime);
    let run_input = serde_json::to_value(&input).expect("ResearchInput is plain data");
    enqueue(
        research_run(),
        run_input,
        EnqueueOptions {
            id: Some(id.clone()),
            owner_user_id: input.owner_user_id.clone(),
            subject_type: Some("research".into()),
            subject_id: Some(id.clone()),
            phase: Some("queued".into()),
            // The drive is inline: this process is the only runtime, so
            // enqueue means row + publish + drive.
            start: Some(true),
        },
        &deps,
    )
    .await
    .map_err(|e| format!("could not enqueue the research run: {e}"))?;

    sqlx::query(
        "insert into research_runs \
         (id, owner_user_id, requested_by, agent_model, mode, question, parent_run_id) \
         values ($1::uuid, $2::uuid, $3, $4, $5, $6, $7::uuid) on conflict (id) do nothing",
    )
    .bind(&id)
    .bind(&input.owner_user_id)
    .bind(&input.requested_by)
    .bind(&input.agent_model)
    .bind(crate::runs::defs::research::depth_str(input.mode))
    .bind(&input.question)
    .bind(&input.parent_run_id)
    .execute(&state.pg)
    .await
    .map_err(|e| e.to_string())?;
    // Read back through the projection rather than `returning`, so the row
    // this hands to the caller is spelled by the same join every other read
    // uses. One indexed read at the start of a run that is about to make
    // model calls.
    let created = get_research_run(&state.pg, &id)
        .await
        .map_err(|e| e.to_string())?;
    let created = created.ok_or_else(|| "could not create the research run".to_string())?;

    // The Titler names the run from its question — fire-and-forget, the list
    // shows the raw question until the title lands. Deliberately NOT a step:
    // its only output is one idempotent column write that nothing waits on,
    // and a step would put another billable call in the run's critical path
    // (and in the set of calls a reclaim repeats) to save a title.
    let question = input.question.clone();
    let run_id = id.clone();
    let st = state.clone();
    tokio::spawn(async move {
        if let Some(t) = crate::titler::generate_title(
            &st,
            crate::harness::defs::titler::TitleKind::Research,
            &question,
        )
        .await
            && let Err(e) = sqlx::query("update research_runs set title = $1 where id = $2::uuid")
                .bind(&t)
                .bind(&run_id)
                .execute(&st.pg)
                .await
        {
            tracing::error!("[research] title write failed for {run_id}: {e}");
        }
    });

    Ok(created.0)
}

// ── The conversation a run is discussed in ────────────────────────────────────
//
// A research run used to be a one-shot: ask, wait, read. The only thing
// anyone could do afterwards was read it again, and the two colleagues it was
// shared with had nowhere to say "dig into the second point" or "this source
// is a vendor blog". A view whose whole content is a document does not need
// to be a view.
//
// So a run gets a conversation of its own, exactly the shape a plan has —
// several people, one agent, one document that grows beside the talk.

/// The conversation for a run, created on first use.
///
/// ON DEMAND rather than at run creation, for a reason worth stating: most
/// runs are read once and never discussed, and a conversation row per run
/// would make the chat list a list of things nobody said anything about. The
/// first message is what makes it exist.
///
/// Owned by the run's owner and pinned to the agent that DID the research, so
/// the teammate answering questions about the report is the one that wrote
/// it. Returns None for a run that is gone, or one an agent with no human
/// owner started — it is still readable; it just cannot be talked in.
pub async fn ensure_research_conversation(
    pg: &PgPool,
    run_id: &str,
) -> Result<Option<String>, sqlx::Error> {
    #[derive(sqlx::FromRow)]
    struct RunHead {
        conversation_id: Option<String>,
        owner_user_id: Option<String>,
        agent_model: String,
        question: String,
        title: Option<String>,
    }
    let run: Option<RunHead> = sqlx::query_as(
        "select conversation_id::text, owner_user_id::text, agent_model, question, title \
             from research_runs where id = $1::uuid",
    )
    .bind(run_id)
    .fetch_optional(pg)
    .await?;
    let Some(RunHead {
        conversation_id,
        owner_user_id,
        agent_model,
        question,
        title,
    }) = run
    else {
        return Ok(None);
    };
    if let Some(id) = conversation_id {
        return Ok(Some(id));
    }
    // A run started by an agent with no human owner has nobody to own the
    // conversation. It is still readable; it just cannot be talked in.
    let Some(owner) = owner_user_id else {
        return Ok(None);
    };

    let heading = title.unwrap_or_else(|| question.chars().take(80).collect());
    let id = crate::conversations::create_conversation(
        pg,
        &owner,
        &agent_model,
        &heading,
        "research",
        None,
    )
    .await?;
    sqlx::query(
        "update research_runs set conversation_id = $2::uuid \
         where id = $1::uuid and conversation_id is null",
    )
    .bind(run_id)
    .bind(&id)
    .execute(pg)
    .await?;
    // Re-read rather than trusting the write: two clients opening the same
    // run at once both get here, and the loser must return the winner's
    // conversation instead of a second one nobody is reading.
    let after: Option<(Option<String>,)> =
        sqlx::query_as("select conversation_id::text from research_runs where id = $1::uuid")
            .bind(run_id)
            .fetch_optional(pg)
            .await?;
    Ok(after.and_then(|(v,)| v).or(Some(id)))
}

/// The run a conversation belongs to, for the surfaces that start from the
/// chat side rather than the report side.
pub async fn research_run_for_conversation(
    pg: &PgPool,
    conversation_id: &str,
) -> Result<Option<String>, sqlx::Error> {
    let row: Option<(String,)> =
        sqlx::query_as("select id::text from research_runs where conversation_id = $1::uuid")
            .bind(conversation_id)
            .fetch_optional(pg)
            .await?;
    Ok(row.map(|(id,)| id))
}

#[cfg(test)]
mod tests {
    // Port of research-extend.test.ts — the document-merge invariants, which
    // are the one place this module can corrupt a document a person has
    // already read.
    use super::*;

    fn src(idx: u64, url: &str) -> ResearchSource {
        ResearchSource {
            idx,
            url: url.into(),
            title: None,
            snippet: None,
        }
    }

    const PARENT: &str = "# Postgres 17\n\nIt rewrote the vacuum [1].\n\n## Sources\n\n1. [a](https://a.test)\n2. [b](https://b.test)\n";

    #[test]
    fn a_follow_up_extends_the_prose_and_renumbers_nothing() {
        let out = extend_report(
            PARENT,
            "What about enterprise tiers?",
            "# Enterprise\n\nThey negotiate [3].",
            &[
                src(1, "https://a.test"),
                src(2, "https://b.test"),
                src(3, "https://c.test"),
            ],
        );
        assert!(out.starts_with("# Postgres 17\n\nIt rewrote the vacuum [1]."));
        assert!(out.contains("## Follow-up: What about enterprise tiers?"));
        // The follow-up's H1 is dropped — one document, one title.
        assert!(!out.contains("\n# Enterprise\n"));
        assert!(out.contains("They negotiate [3]."));
        // Source 2 is genuinely consulted — the merged prose cites [1] and
        // [3], never [2]; "cited" is markers in prose, not list membership.
        assert!(out.ends_with(
            "## Sources\n\n1. [https://a.test](https://a.test)\n2. [https://b.test](https://b.test) *(consulted)*\n3. [https://c.test](https://c.test)\n"
        ));
    }

    #[test]
    fn the_source_list_is_rebuilt_not_appended() {
        let out = extend_report(
            PARENT,
            "q",
            "They negotiate [3].",
            &[
                src(1, "https://a.test"),
                src(2, "https://b.test"),
                src(3, "https://c.test"),
            ],
        );
        // Exactly ONE Sources section, however many passes merged into the
        // document.
        assert_eq!(out.matches("## Sources").count(), 1);
    }

    #[test]
    fn cited_is_recomputed_over_the_whole_document() {
        // The parent cites [1]; the follow-up does not. It must stay cited —
        // a pass that only looked at the new section would mark it consulted.
        let out = extend_report(
            PARENT,
            "q",
            "Only this one matters [3].",
            &[
                src(1, "https://a.test"),
                src(2, "https://b.test"),
                src(3, "https://c.test"),
            ],
        );
        assert!(out.contains("1. [https://a.test](https://a.test)\n"));
        assert!(out.contains("2. [https://b.test](https://b.test) *(consulted)*\n"));
        assert!(out.contains("3. [https://c.test](https://c.test)\n"));
    }

    #[test]
    fn a_source_the_followup_cited_stays_cited_even_when_new() {
        let out = extend_report(
            PARENT,
            "q",
            "More [3].",
            &[
                src(1, "https://a.test"),
                src(2, "https://b.test"),
                src(3, "https://c.test"),
            ],
        );
        // [3] is new THIS pass and cited by it — no consulted suffix; the
        // uncited [2] keeps its own.
        assert!(out.contains("3. [https://c.test](https://c.test)\n"));
        assert!(!out.contains("[https://c.test](https://c.test) *(consulted)*"));
        assert!(out.contains("2. [https://b.test](https://b.test) *(consulted)*\n"));
    }

    #[test]
    fn prose_mentioning_the_word_sources_is_not_truncated() {
        let chatty = "# T\n\nOur sources disagree. See the Sources below.\n\nA claim [1].\n\n## Sources\n\n1. [a](https://a.test)\n";
        let body = report_body_only(chatty);
        assert!(body.contains("Our sources disagree. See the Sources below."));
        assert!(!body.contains("1. [a]"));
    }

    #[test]
    fn a_parent_with_no_sources_section_gains_one() {
        let out = extend_report(
            "# Title\n\nA claim.",
            "q",
            "More [1].",
            &[src(1, "https://a.test")],
        );
        assert!(out.contains("A claim.\n\n## Follow-up: q"));
        assert!(out.ends_with("## Sources\n\n1. [https://a.test](https://a.test)\n"));
    }

    #[test]
    fn unknown_markers_are_dropped_and_counted() {
        let doc = "Claim [1]. Invented [9]. Kept [2].";
        let (cleaned, dropped, cited) = strip_unknown_markers(doc, &[1, 2]);
        assert_eq!(dropped, 1);
        assert_eq!(cleaned, "Claim [1]. Invented . Kept [2].");
        assert_eq!(cited, [1, 2].into_iter().collect());
    }

    #[test]
    fn a_fenced_reply_is_unwrapped() {
        let (cleaned, dropped, _) = strip_unknown_markers("```\nClaim [1].\n```", &[1]);
        assert_eq!(cleaned, "Claim [1].");
        assert_eq!(dropped, 0);
    }
}
