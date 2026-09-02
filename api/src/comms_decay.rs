// Distill-then-archive: conversations with agents don't accumulate forever.
// Idle agent DMs get their durable substance summarized into the activity
// brain (owner-scoped), then archive out of the sidebar — context survives,
// scrollback doesn't.
//
// The scheduled half is the sweep and its job registration; `conclude_relay`
// — the comms family's own surface — is called by the conclude route.
//
// THE ACCOUNTING IS THE MODULE. Two of distillation's three outcomes mean
// "archived NOTHING", and neither may be counted as an archive. Every count
// below is taken from what came back, never from "it did not throw", and
// `considered === archived + skipped-no-model
// + skipped-empty + failed` is the property that makes the hourly log line
// checkable rather than decorative.

use std::sync::Arc;

use crate::fleet::describe_agent;
use crate::harness::defs::distiller::distiller_harness;
use crate::harness::run::{RunContext, run_harness};
use crate::retrieval::index::IndexDoc;
use crate::retrieval::sources::{index_activity, index_personal};
use crate::retrieval::{embed, qdrant};
use crate::scheduler::{JobName, JobSpec};
use crate::state::AppState;
use serde_json::json;
use sqlx::PgPool;

/// `TALARIA_CHAT_TTL_DAYS`, floored at one day. Read per pass, so a config
/// change needs no restart.
fn ttl_days() -> i64 {
    std::env::var("TALARIA_CHAT_TTL_DAYS")
        .ok()
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(14)
        .max(1)
}

/// The deploy-day bound. This sweep ARCHIVES people's conversations, and
/// the first pass may find a long backlog. One pass never touches more than this
/// many, and a pass runs at most hourly, so the worst case is ~8 archives an
/// hour (~190/day) draining gradually rather than a wall of "where did my
/// chats go" on the morning of a deploy.
const SWEEP_BATCH: i64 = 8;

/// Clip a transcript for the prompt. The bound is advisory (it guards prompt
/// size, not a byte contract), so it cuts at the last whole character — the
/// house rule the blurb clamp already records for Rust strings.
fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let cut: String = s.chars().take(max).collect();
    format!("{cut}\n…(truncated)")
}

/// What one conversation's turn actually did. The sweep counts these rather
/// than assuming, because two of the three are ways the distill returns
/// having archived NOTHING.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DistillOutcome {
    /// Distilled (or empty and nothing to distill), indexed, and archived.
    Archived,
    /// No Distiller model and no muse for the owner — nothing can summarize
    /// it, so it is left exactly where it was for a sweep that has one.
    NoModel,
    /// The model answered with nothing. Left unarchived on purpose:
    /// archiving on a failed distillation is how the substance is lost.
    EmptyDistillation,
}

/// What a harness run means to this module: a usable distillation, or the
/// non-archiving outcome it has to be counted as.
///
/// THE TWO NULLS ARE NOT THE SAME EVENT, and this function exists so nothing
/// ever has to re-derive which is which. `HarnessResult` reports `model` and
/// `value` separately for exactly this reason:
///
///   model === None    nothing is CONFIGURED to summarize with. Every
///                     conversation in the batch will hit it, it will still
///                     be true in an hour, and the sweep escalates it to a
///                     human because only a human can assign a model.
///   value === None    a model was asked and could not answer. This one
///                     conversation is left alone and retried next pass.
///
/// Collapsing them loses the escalation. Treating either as success archives
/// a conversation whose substance was never captured — the exact failure this
/// whole module is written around.
pub fn distill_outcome(run: &crate::harness::run::HarnessResult) -> Result<String, DistillOutcome> {
    // The model fact wins the ambiguous case: "there is nothing to summarize
    // with" is what makes the whole batch fail and the only one of the two an
    // operator can act on.
    if run.model.is_none() {
        return Err(DistillOutcome::NoModel);
    }
    let Some(v) = run.value.as_ref().and_then(|v| v.as_str()) else {
        return Err(DistillOutcome::EmptyDistillation);
    };
    Ok(v.to_string())
}

/// One idle conversation as the due query hands it back.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct IdleConv {
    pub id: String,
    pub user_id: String,
    pub agent_model: String,
    pub title: Option<String>,
}

/// The per-conversation work as one injected edge — everything below
/// `sweep_idle_chats`'s accounting: read the messages, run the distiller,
/// index the distillation twice, file the artifact, archive. The seam
/// exists so the accounting can be pinned in tests without any of that.
/// `Err` is the caught-throw path: an infrastructure failure the
/// sweep counts as `failed` and moves on.
pub type DistillFn = Arc<
    dyn Fn(IdleConv) -> futures_util::future::BoxFuture<'static, Result<DistillOutcome, String>>
        + Send
        + Sync,
>;

pub struct DecayDeps {
    pub pg: PgPool,
    pub distill: DistillFn,
}

/// The production edge: `distill_conversation` whole.
pub fn real_decay_deps(state: &AppState) -> DecayDeps {
    let st = state.clone();
    DecayDeps {
        pg: state.pg.clone(),
        distill: Arc::new(move |conv| {
            let st = st.clone();
            Box::pin(async move { distill_conversation(&st, conv).await })
        }),
    }
}

/// Distill one idle agent DM into the activity brain, then archive it.
async fn distill_conversation(state: &AppState, conv: IdleConv) -> Result<DistillOutcome, String> {
    let msgs: Vec<(String, String)> = sqlx::query_as(
        "select role, content from messages \
         where conversation_id = $1::uuid and content <> '' order by seq asc",
    )
    .bind(&conv.id)
    .fetch_all(&state.pg)
    .await
    .map_err(|e| format!("message read failed: {e}"))?;
    let label = describe_agent(&conv.agent_model).label;
    let transcript = clip(
        &msgs
            .iter()
            .map(|(role, content)| {
                format!(
                    "{}: {}",
                    if role == "assistant" { &label } else { "User" },
                    content
                )
            })
            .collect::<Vec<_>>()
            .join("\n\n"),
        60_000,
    );

    // A conversation with nothing in it archives without needing a model at
    // all — resolving the chain first would leave a wholly empty chat on an
    // install with no Distiller model stuck 'no-model': unarchivable for
    // ever, and counted into the skipped-no-model total that makes the job
    // fail. There is no substance to lose here, so the never-archive-on-a-
    // failed-distillation rule has nothing to protect.
    if !transcript.trim().is_empty() {
        // `user_id` is what turns on the owner's preferred model and the
        // member allowlist when the harness resolves the distiller chain.
        let run = run_harness(
            state,
            &distiller_harness(),
            &json!({ "agentLabel": label, "transcript": transcript }),
            RunContext {
                caller: format!("platform:distiller:{}", conv.user_id),
                user_id: Some(conv.user_id.clone()),
                ..RunContext::default()
            },
        )
        .await
        .map_err(|e| e.to_string())?;
        let text = match distill_outcome(&run) {
            Ok(text) => text,
            Err(outcome) => return Ok(outcome),
        };
        let title = format!(
            "Distilled: {}",
            conv.title
                .as_deref()
                .unwrap_or(&format!("chat with {label}"))
        );
        // Twice on purpose: the activity copy keeps the owner's ambient search
        // working as before (owner-scoped), and the personal-brain copy is what
        // their assistant retrieves — its private memory of this user's
        // history. Search merges dedupe by source, so the owner never sees it
        // doubled.
        let distill_doc = IndexDoc {
            source_type: "chat-distill".into(),
            source_id: conv.id.clone(),
            title: Some(title.clone()),
            text: text.clone(),
            payload: Some(
                vec![("ownerUserId".to_string(), json!(conv.user_id))]
                    .into_iter()
                    .collect(),
            ),
            href: Some("/comms".into()),
        };
        let qd = qdrant::real_deps();
        let ed = embed::real_deps();
        index_activity(&state.pg, &qd, &ed, &distill_doc).await?;
        index_personal(&state.pg, &qd, &ed, &conv.user_id, &distill_doc).await;
        // The distill is also a browsable artifact — PRIVATE to the chat's
        // owner (a DM's substance is theirs), filed under the agent's
        // "Chat summaries". Filing is best-effort: the distillation is
        // already indexed, and a failed filing must not leave the chat
        // unarchived to distill a second time next pass.
        if let Err(e) = file_distill_artifact(state, &conv, &label, &title, &text).await {
            tracing::warn!("[comms-decay] distill artifact filing failed: {e}");
        }
    }
    sqlx::query("update conversations set archived = true where id = $1::uuid")
        .bind(&conv.id)
        .execute(&state.pg)
        .await
        .map_err(|e| format!("archive write failed: {e}"))?;
    Ok(DistillOutcome::Archived)
}

async fn file_distill_artifact(
    state: &AppState,
    conv: &IdleConv,
    label: &str,
    title: &str,
    text: &str,
) -> Result<(), sqlx::Error> {
    let folder =
        crate::artifacts::agent_category_folder(&state.pg, label, "Chat summaries", label).await;
    let artifact = crate::artifacts::create_artifact(
        &state.pg,
        Some("doc"),
        Some(title),
        label,
        Some(&conv.user_id),
        folder.as_deref(),
    )
    .await?;
    crate::artifacts::save_artifact(
        &state.pg,
        &artifact.id,
        crate::artifacts::SaveArtifactPatch {
            body: Some(text),
            ..Default::default()
        },
        label,
    )
    .await?;
    Ok(())
}

/// One pass, counted. `considered == archived + skipped_no_model +
/// skipped_empty_distillation + failed` always — the sweep accounts for every
/// conversation it touched.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DecaySweepResult {
    pub considered: usize,
    pub archived: usize,
    pub skipped_no_model: usize,
    pub skipped_empty_distillation: usize,
    pub failed: usize,
}

/// The accounting loop, split from the query so the invariants can be pinned
/// without a database.
async fn run_sweep(idle: Vec<IdleConv>, distill: &DistillFn) -> DecaySweepResult {
    let mut result = DecaySweepResult {
        considered: idle.len(),
        ..Default::default()
    };
    for conv in idle {
        // Counted by what came back, never by "it did not throw". One bad
        // conversation must not abandon the rest of the batch — but a
        // conversation that fails every pass forever must also be named. The
        // next sweep retries it.
        match distill(conv.clone()).await {
            Ok(DistillOutcome::Archived) => result.archived += 1,
            Ok(DistillOutcome::NoModel) => result.skipped_no_model += 1,
            Ok(DistillOutcome::EmptyDistillation) => result.skipped_empty_distillation += 1,
            Err(e) => {
                result.failed += 1;
                tracing::error!(
                    "[comms-decay] conversation {} could not be distilled: {e}",
                    conv.id
                );
            }
        }
    }
    result
}

/// One pass: distill + archive up to SWEEP_BATCH idle agent DMs. Plans are
/// exempt — they're durable documents, not chat scrollback.
pub async fn sweep_idle_chats(deps: &DecayDeps) -> Result<DecaySweepResult, String> {
    let idle: Vec<IdleConv> = sqlx::query_as(
        "select id::text, user_id::text, agent_model, title from conversations \
         where kind = 'chat' and archived = false \
           and updated_at < now() - make_interval(days => $1::int) \
         order by updated_at asc limit $2",
    )
    .bind(ttl_days())
    .bind(SWEEP_BATCH)
    .fetch_all(&deps.pg)
    .await
    .map_err(|e| format!("idle chat query failed: {e}"))?;
    Ok(run_sweep(idle, &deps.distill).await)
}

/// The job's sentence. `Ok(None)` is a quiet pass; `Err` is the escalation
/// below, which is the module's sharpest edge.
fn decay_message(r: &DecaySweepResult) -> Result<Option<String>, String> {
    if r.considered == 0 {
        return Ok(None);
    }
    let mut line = format!("{} idle chat(s) distilled + archived", r.archived);
    if r.skipped_empty_distillation > 0 {
        line.push_str(&format!(
            ", {} left alone (the summary came back empty)",
            r.skipped_empty_distillation
        ));
    }
    if r.failed > 0 {
        line.push_str(&format!(", {} failed", r.failed));
    }
    // NOTHING TO SUMMARIZE WITH IS A FAILED RUN, NOT A QUIET ONE. The sweep
    // picked these conversations up, could not act on a single one of them,
    // and will pick the same ones up again in an hour and every hour after
    // that — for ever, silently. Err so it lands in the scheduler's error
    // state, which is what `unhealthy_jobs` reads and what puts it in front
    // of an operator who can assign a model.
    if r.skipped_no_model > 0 {
        return Err(format!(
            "{} of {} idle chat(s) could not be distilled: no model is assigned to the \
             Distiller platform agent (Admin → Platform agents) and their owners have no muse \
             model either, so nothing can summarize them and they will not decay. {line}.",
            r.skipped_no_model, r.considered
        ));
    }
    Ok(Some(line))
}

// ── Conclude a Relay ─────────────────────────────────────────────────────────

/// Conclude a Relay: post + index a summary of what was decided, then archive.
/// Returns the summary so the UI can show it after the relay leaves the list.
///
/// The Err strings are USER-FACING COPY shown by the conclude button (the
/// route maps them onto its 502 body verbatim) — not developer messages.
pub async fn conclude_relay(
    state: &AppState,
    channel_id: &str,
    by_user_id: &str,
    channel_name: &str,
) -> Result<String, String> {
    let deps = crate::notify::NotifyDeps::publishing(state.pg.clone(), state.redis().await.ok());
    let history = crate::channels::list_channel_messages(&deps.pg, channel_id, -1, 500, true)
        .await
        .map_err(|e| format!("message read failed: {e}"))?;
    let transcript = clip(
        &history
            .iter()
            .filter(|m| m.status == "complete" && !m.content.is_empty())
            .map(|m| {
                format!(
                    "{}: {}",
                    if m.author_type == "agent" {
                        describe_agent(&m.author).label
                    } else {
                        m.author.clone()
                    },
                    m.content
                )
            })
            .collect::<Vec<_>>()
            .join("\n\n"),
        60_000,
    );
    if transcript.trim().is_empty() {
        return Err("nothing to conclude: the relay has no messages".into());
    }

    // The model chain and the empty-reply check live in the concluder
    // harness; the failure arms are mapped BY HAND here, and the reason is
    // down to one: these strings are user-facing copy, not a developer's
    // error message.
    let run = run_harness(
        state,
        &crate::harness::defs::concluder::concluder_harness(),
        &json!({ "channelName": channel_name, "transcript": transcript }),
        RunContext {
            caller: format!("platform:concluder:{by_user_id}"),
            user_id: Some(by_user_id.to_string()),
            ..RunContext::default()
        },
    )
    .await
    .map_err(|e| e.to_string())?;
    if run.model.is_none() {
        return Err("no model configured to summarize with. Add an endpoint on /models.".into());
    }
    // THREE outcomes, not two. The runner also returns for a transport
    // failure, and folding that into "came back empty" told a user whose
    // provider was rate limiting to try again — into the same rate limit.
    // `answered` is "did the model speak" under its own name; the runner's
    // sentence is the right copy whenever it never did.
    if !run.answered
        && let Some(error) = run.error
    {
        return Err(error);
    }
    let Some(serde_json::Value::String(text)) = run.value else {
        return Err("the summary came back empty. Try again.".into());
    };

    // The summary is the relay's last word: posted into history (visible if
    // the relay is ever revisited) and indexed for retrieval
    // (channel-membership ACL).
    let agents = crate::channels::list_channel_agents(&deps.pg, channel_id)
        .await
        .map_err(|e| format!("agent list read failed: {e}"))?;
    crate::channels::insert_channel_message(
        &deps,
        channel_id,
        "agent",
        agents.first().map(String::as_str).unwrap_or("talaria"),
        &format!("**Relay concluded**. Summary:\n\n{text}"),
        "complete",
        &json!([]),
        None,
    )
    .await
    .map_err(|e| format!("summary post failed: {e}"))?;
    let doc = IndexDoc {
        source_type: "relay-summary".into(),
        source_id: channel_id.to_string(),
        title: Some(format!("Relay concluded: {channel_name}")),
        text: text.clone(),
        payload: Some(
            vec![("channelId".to_string(), json!(channel_id))]
                .into_iter()
                .collect(),
        ),
        href: Some("/comms".into()),
    };
    let qd = qdrant::real_deps();
    let ed = embed::real_deps();
    index_activity(&deps.pg, &qd, &ed, &doc).await?;
    crate::channels::archive_channel(&deps, channel_id)
        .await
        .map_err(|e| format!("archive failed: {e}"))?;
    Ok(text)
}

// ── The registration ────────────────────────────────────────────────────────

/// The job the scheduler runs, from a built deps bag — the four declared
/// timings in their four slots, and nothing invented here. NOT `per_instance`:
/// the input is the `conversations` table, which every instance can reach, and
/// a second instance sweeping would archive the same chat twice.
pub fn comms_decay_job_spec(deps: Arc<DecayDeps>) -> JobSpec {
    JobSpec {
        name: JobName::CommsDecay,
        // Hourly.
        every_ms: 60 * 60_000,
        // Not at the instant of boot: a crash-looping instance should never
        // reach a job that archives, and a deploy should settle before it
        // starts writing.
        first_run_delay_ms: Some(2 * 60_000),
        // SWEEP_BATCH distillations, each an LLM round trip; generous, and the
        // lease renews while it runs anyway.
        max_run_ms: Some(15 * 60_000),
        per_instance: false,
        run: Arc::new(move || {
            let deps = deps.clone();
            Box::pin(async move {
                let r = sweep_idle_chats(&deps).await?;
                decay_message(&r)
            })
        }),
    }
}

/// Declare the sweep to the scheduler — called from boot. The deps are
/// runtime values, so the registration is a call rather than a static; it is
/// what puts the job in the runtime graph, and 'comms-decay' is in
/// REQUIRED_JOBS, so an instance that boots without reaching it prints a
/// MISSING JOBS error instead of quietly never decaying a chat.
pub fn register_comms_decay_job(deps: Arc<DecayDeps>) {
    crate::scheduler::register_job(comms_decay_job_spec(deps));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::run::HarnessResult;
    use serde_json::Value;
    use std::sync::Mutex;

    // THE INVARIANT UNDER TEST: `distill` has three outcomes and two of
    // them mean "archived NOTHING". Counting either as an archive is how
    // the substance of a conversation is destroyed. Everything below the
    // accounting loop is faked, so every assertion is about THIS module's
    // accounting.

    fn run_with(model: Option<&str>, value: Option<&str>) -> HarnessResult {
        HarnessResult {
            value: value.map(|v| Value::String(v.into())),
            model: model.map(str::to_string),
            ..bare_run()
        }
    }

    fn bare_run() -> HarnessResult {
        HarnessResult {
            value: None,
            model: None,
            step: None,
            widened: false,
            repairs: 0,
            schema_valid: true,
            answered: true,
            refused: false,
            findings: Vec::new(),
            raw: None,
            latency_ms: 1,
            escalate: false,
            error: None,
        }
    }

    fn conv(id: &str) -> IdleConv {
        IdleConv {
            id: id.into(),
            user_id: format!("user-{}", &id[id.len() - 1..]),
            agent_model: "nomad".into(),
            title: Some("ledger".into()),
        }
    }

    // ── the mapping, on its own ──────────────────────────────────────────────

    #[test]
    fn maps_a_null_model_to_no_model() {
        // Nothing is configured to summarize with. Every conversation in the
        // batch hits this, it is still true in an hour, and only a human can
        // fix it — so the sweep has to be able to escalate rather than shrug.
        assert_eq!(
            distill_outcome(&run_with(None, None)),
            Err(DistillOutcome::NoModel)
        );
    }

    #[test]
    fn maps_a_null_value_to_empty_distillation_not_no_model() {
        // A model was asked and could not answer. This one conversation is
        // retried next pass and nobody is paged.
        assert_eq!(
            distill_outcome(&run_with(Some("pl-main"), None)),
            Err(DistillOutcome::EmptyDistillation)
        );
    }

    #[test]
    fn passes_a_real_distillation_through() {
        assert_eq!(
            distill_outcome(&run_with(Some("pl-main"), Some("a summary"))),
            Ok("a summary".into())
        );
    }

    #[test]
    fn a_value_with_no_model_behind_it_is_still_no_model() {
        // The ambiguous case, and the model fact wins: "there is nothing to
        // summarize with" is what makes the whole batch fail and the only one
        // of the two an operator can act on.
        assert_eq!(
            distill_outcome(&run_with(None, Some("somehow a value"))),
            Err(DistillOutcome::NoModel)
        );
    }

    // ── the sweep, over the mapping it depends on ────────────────────────────

    /// A scripted distill edge: one outcome per conversation, in order.
    fn scripted(
        outcomes: Vec<Result<DistillOutcome, String>>,
    ) -> (DistillFn, Arc<Mutex<Vec<String>>>) {
        let queue = Arc::new(Mutex::new(outcomes));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen2 = seen.clone();
        let edge: DistillFn = Arc::new(move |conv| {
            let queue = queue.clone();
            let seen = seen2.clone();
            Box::pin(async move {
                seen.lock().unwrap().push(conv.id);
                queue.lock().unwrap().remove(0)
            })
        });
        (edge, seen)
    }

    #[tokio::test]
    async fn archives_when_the_distillation_lands() {
        let (edge, _) = scripted(vec![Ok(DistillOutcome::Archived)]);
        let r = run_sweep(vec![conv("conv-1")], &edge).await;
        assert_eq!(
            r,
            DecaySweepResult {
                considered: 1,
                archived: 1,
                ..Default::default()
            }
        );
    }

    #[tokio::test]
    async fn never_archives_when_no_model_resolved() {
        // The whole point of the module: a conversation archived without its
        // distillation is a conversation deleted with no record of what was
        // in it.
        let (edge, _) = scripted(vec![Ok(DistillOutcome::NoModel)]);
        let r = run_sweep(vec![conv("conv-1")], &edge).await;
        assert_eq!(r.skipped_no_model, 1);
        assert_eq!(r.archived, 0);
    }

    #[tokio::test]
    async fn never_archives_when_the_distillation_came_back_empty() {
        let (edge, _) = scripted(vec![Ok(DistillOutcome::EmptyDistillation)]);
        let r = run_sweep(vec![conv("conv-1")], &edge).await;
        assert_eq!(r.skipped_empty_distillation, 1);
        assert_eq!(r.skipped_no_model, 0);
        assert_eq!(r.archived, 0);
    }

    #[tokio::test]
    async fn accounts_for_every_conversation_it_considered() {
        // The property that makes the hourly log line checkable rather than
        // decorative.
        let (edge, _) = scripted(vec![
            Ok(DistillOutcome::Archived),
            Ok(DistillOutcome::NoModel),
            Ok(DistillOutcome::EmptyDistillation),
            Err("gateway 503".into()),
        ]);
        let r = run_sweep(
            vec![
                conv("conv-1"),
                conv("conv-2"),
                conv("conv-3"),
                conv("conv-4"),
            ],
            &edge,
        )
        .await;
        assert_eq!(
            r.archived + r.skipped_no_model + r.skipped_empty_distillation + r.failed,
            r.considered
        );
    }

    #[tokio::test]
    async fn one_bad_conversation_does_not_abandon_the_rest_of_the_batch() {
        let (edge, _) = scripted(vec![
            Err("gateway 503".into()),
            Ok(DistillOutcome::Archived),
        ]);
        let r = run_sweep(vec![conv("conv-1"), conv("conv-2")], &edge).await;
        assert_eq!(r.considered, 2);
        assert_eq!(r.archived, 1);
        assert_eq!(r.failed, 1);
    }

    #[tokio::test]
    async fn the_sweep_visits_each_conversation_once_in_order() {
        let (edge, seen) = scripted(vec![
            Ok(DistillOutcome::Archived),
            Ok(DistillOutcome::Archived),
            Ok(DistillOutcome::Archived),
        ]);
        run_sweep(vec![conv("conv-1"), conv("conv-2"), conv("conv-3")], &edge).await;
        assert_eq!(*seen.lock().unwrap(), vec!["conv-1", "conv-2", "conv-3"]);
    }

    // ── the job's sentence ───────────────────────────────────────────────────

    #[test]
    fn a_quiet_pass_is_nothing_to_do() {
        assert_eq!(decay_message(&DecaySweepResult::default()), Ok(None));
    }

    #[test]
    fn a_landed_pass_names_its_parts() {
        let r = DecaySweepResult {
            considered: 3,
            archived: 2,
            skipped_empty_distillation: 1,
            ..Default::default()
        };
        assert_eq!(
            decay_message(&r),
            Ok(Some(
                "2 idle chat(s) distilled + archived, 1 left alone (the summary came back empty)"
                    .into()
            ))
        );
    }

    #[test]
    fn nothing_to_summarize_with_is_a_failed_run_not_a_quiet_one() {
        let r = DecaySweepResult {
            considered: 2,
            skipped_no_model: 2,
            ..Default::default()
        };
        let msg = decay_message(&r).unwrap_err();
        // The sentence an operator can act on: where to assign the model, and
        // what the pass managed anyway.
        assert!(msg.contains("no model is assigned"), "{msg}");
        assert!(msg.contains("2 of 2"), "{msg}");
    }

    #[test]
    fn clip_cuts_at_a_char_boundary_and_marks_the_cut() {
        let clipped = clip("abcdefghij", 4);
        assert_eq!(clipped, "abcd\n…(truncated)");
        assert_eq!(clip("abc", 4), "abc");
    }
}
