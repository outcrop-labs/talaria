// The boot assembly — the one place the job bodies become a running schedule,
// and the one place the six armed run steps get their deps.
//
// Every job module owns its own `register_*_job(deps)`: the deps are runtime
// values (the pool, the realtime fan-out, the secretbox), so registration
// cannot live at module load — SOMETHING has to build the real edges at boot
// and hand them over. This module is that something, and its one rule is
// COMPLETENESS: every job is declared here in one visible list, because a
// register call that falls out of this function is work that silently never
// happens. `start_scheduler`'s boot check enforces the required nine against
// this list; the test below pins the whole table.
//
// THE OTHER HALF of the assembly is the run kinds. The defs self-register on
// first getter call, so `try_arm` touches each getter and then REFUSES to arm
// unless the census's six kinds are all present. The refusal is a boot list
// made executable: `run-reclaim` is one of the ten jobs, and a sweep that
// cannot define a kind leaves those rows to an instance that can — no such
// instance exists, and the rows would sit forever with only a warn line.
// A kind reappearing in it is a def module that fell out of the boot list
// below.
//
// update-check rides the same table now (api/src/update/job.rs): the
// restart-topology HOLD it sat under is retired by the update engine — a
// roll replaces the whole CONTAINER (slots behind an edge), so the binary
// that choreographs a roll is under no obligation to survive it. The job
// stays out of REQUIRED_JOBS: on the dormant installs (checkout, dev,
// off) it is an honest no-op, and a missing no-op is not work that
// silently never happened.

use std::sync::Arc;
use std::time::Duration;

use talaria_agent_auth::now_ms as wall_ms;
use talaria_comms_decay::real_decay_deps;
use talaria_daily_brief::real_brief_deps;
use talaria_digest::real_digest_deps;
use talaria_model_info::BlurbDeps;
use talaria_notify::real_drain_deps;
use talaria_outreach::OutreachDeps;
use talaria_price_oracle::PriceRefreshDeps;
use talaria_realtime::RealtimeDeps;
use talaria_runs_decide::assembly::real_run_deps;
use talaria_runs_define::run_definition;
use talaria_runs_reclaim::{ReclaimDeps, drive_fn, due_fn};
use talaria_runs_run::RunDeps;
use talaria_scheduler as scheduler;
use talaria_scheduler::REQUIRED_JOBS;
use talaria_secretbox::SecretBox;
use talaria_state::AppState;

const LOG: &str = "[jobs]";

/// The run kinds the boot census carries, spelled as strings because that is
/// what the registry keys on. Arming with fewer means the sweep strands rows
/// of the missing kinds (see the header).
const BOOT_RUN_KINDS: &[&str] = &[
    "agent-hire",
    "plan-draft",
    "rag-backfill",
    "rag-reindex",
    "research",
    "work-session",
];

/// Declare the whole job table, one register call per job. The `run` and
/// `rt` edges are parameters (not built here) only so
/// the completeness test can inject fakes: registration only CAPTURES deps,
/// never invokes them, and `try_arm` builds the real ones. Every other deps
/// constructor in this list is pure closure-building over the lazy pool.
pub async fn register_all(state: &AppState, run: Arc<RunDeps>, rt: RealtimeDeps, sb: &SecretBox) {
    talaria_comms_decay::register_comms_decay_job(Arc::new(real_decay_deps(state)));
    talaria_outreach::register_outreach_job(Arc::new(OutreachDeps {
        state: state.clone(),
    }));
    let _ =
        talaria_web_search::CALL_MCP_TOOL.set(std::sync::Arc::new(|pg, sb, server, tool, args| {
            Box::pin(async move {
                talaria_mcp::registry::call_mcp_tool(&pg, &sb, &server, &tool, &args)
                    .await
                    .map(|out| (out.structured, out.text))
            })
        }));
    let _ = talaria_fleet_create::RENDER_FLEET.set(std::sync::Arc::new(|pg, sb| {
        Box::pin(async move {
            talaria_fleet_render::render_fleet(&pg, &sb, None)
                .await
                .map(|_| ())
        })
    }));
    let _ = talaria_mcp_apply::ROLL_AGENT.set(std::sync::Arc::new(|pg, sb, dept| {
        Box::pin(async move { talaria_fleet_reconcile::roll_agent(&pg, &sb, &dept).await })
    }));
    let _ = talaria_update_job::ROLL_FLEET.set(std::sync::Arc::new(|pg, sb| {
        Box::pin(async move { talaria_fleet_reconcile::roll_running_agents(&pg, &sb).await })
    }));
    // THE ANNOUNCE EDGE FOR A QUEUED GOOGLE WRITE. `queue_action` calls this
    // the instant a confirm-send lands, so the person who has to approve it
    // hears immediately rather than on the approval sweep's next tick — which
    // is what its own comment says it does.
    //
    // It was declared and never set. The call site reads
    // `if let Some(f) = ANNOUNCE_APPROVAL.get()`, so an unwired edge is not a
    // compile error and not a log line: the announce silently did nothing and
    // every queued write waited up to five minutes for the sweep, with an
    // agent stopped in front of it. Same completeness rule as the roll edges
    // above — an edge nothing sets is a feature nothing runs.
    let _ =
        talaria_google_pending::ANNOUNCE_APPROVAL.set(std::sync::Arc::new(|pg, realtime, key| {
            Box::pin(async move {
                let deps = talaria_approvals::ApprovalDeps::new(
                    pg,
                    realtime,
                    std::sync::Arc::new(run_definition),
                    std::sync::Arc::new(wall_ms),
                );
                talaria_approvals::announce_approval(&deps, &key).await;
            })
        }));
    // THE ROLL'S OWN TWO EDGES. `roll_agent` reaches the renderer through
    // them — a roll renders the incoming slot, brings it up, flips the
    // manifest, then re-renders — and an edge nothing sets is a roll that
    // returns "… not wired" into callers that discard it: the roster never
    // changes, the logs stay silent, and the agent keeps running the config
    // the operator just replaced. Same completeness rule as the job table
    // above, one layer down.
    let _ = talaria_fleet_reconcile::RENDER_FLEET.set(std::sync::Arc::new(|pg, sb, roll| {
        Box::pin(async move {
            let overlay =
                roll.as_ref()
                    .map(|(slug, slot, port)| talaria_fleet_render::RollOverlay {
                        slug,
                        slot: *slot,
                        port: *port,
                    });
            talaria_fleet_render::render_fleet(&pg, &sb, overlay)
                .await
                .map(|render| (render.agents.len(), render.warnings))
        })
    }));
    let _ = talaria_fleet_reconcile::NEXT_FREE_PORT.set(std::sync::Arc::new(|pg| {
        Box::pin(async move {
            talaria_fleet_render::next_free_port(&pg)
                .await
                .map_err(|e| e.to_string())
        })
    }));
    let _ = talaria_fleet_docker::PREFLIGHT.set(|pool| {
        tokio::spawn(async move {
            let _ = talaria_fleet_preflight::run_fleet_preflight(&pool).await;
        });
    });
    let _ = talaria_gateway::usage::NUDGE_AUTO_PRICES.set(talaria_price_oracle::nudge_auto_prices);
    let spend_state = state.clone();
    let _ = talaria_price_oracle::LOAD_SPEND_KEYS.set(std::sync::Arc::new(move || {
        let state = spend_state.clone();
        Box::pin(async move { talaria_price_oracle::keys_for(&state).await })
    }));
    // THE RUN ASSEMBLY plan-draft, research, and reindex enqueue through.
    // The seam and its constructor (`work_dispatch::dispatch_deps`, which is
    // `real_run_deps` — the same assembly agent-hire calls directly) both
    // existed; this wiring did not, so every plan-draft POST answered 500
    // "could not start the plan draft" — `BUILD_DISPATCH.get()` is None on
    // every install. Research start and guided reindex die the same way.
    let _ = talaria_tasks_types::BUILD_DISPATCH.set(Arc::new(talaria_work_dispatch::dispatch_deps));
    // THE ATTRIBUTION LADDER'S CHATTER RUNG. The seam (CONVERSATION_OWNER) and
    // the resolver it wants (conversations::conversation_owner — whose doc
    // comment names this ladder) both existed; this wiring did not, so the
    // live-turn rung read an unset OnceLock and silently fell to the hirer on
    // every install. Nothing noticed for the same reason nothing ever does: an
    // unset optional rung answers SOMEONE, just the wrong one. The live suite's
    // first CI run (api-integration.yml, attribution's
    // a_live_turn_outranks_the_hirer) is what caught it.
    let _ = talaria_attribution::CONVERSATION_OWNER.set(std::sync::Arc::new(|pg, id| {
        Box::pin(async move { talaria_conversations::conversation_owner(&pg, &id).await })
    }));
    // ── THE EXTRACTION WAVE'S TWELVE ────────────────────────────────────────
    //
    // Between 2026-09-18 and 2026-09-19 a burst of crate extractions moved a
    // dozen modules out of the monolith, each correctly breaking its new
    // crate's dependency on a heavier one by declaring an injected edge — and
    // not one of them was set here. Every extraction verified with
    // `cargo check`, which is exactly the tool that cannot see an unset
    // `OnceLock`: the declaration compiles, the read compiles, and the `None`
    // arm is a valid answer.
    //
    // The live instance dated the damage. `harness_runs` was active to
    // 2026-10-05, so the instance was plainly running — and `skill_summaries`
    // (29 rows), `gap_reported` notifications (15) and `titler` runs (49) all
    // stopped writing on 2026-09-18, the day their crate was extracted, and
    // wrote nothing for the eighteen days after. COUNTS LOOKED HEALTHY; the
    // dates were the evidence.
    //
    // Each edge below is the pre-extraction call restored. The
    // `oncelock-seam-never-set` invariant and the boot assertions in this
    // crate's tests are what stop the thirteenth.

    // The honesty loop's voice. Unset, `audience_for` never ran and the
    // notification fan-out iterated two empty lists — so a capability gap was
    // filed and NOBODY was told, which is the one promise that queue makes.
    let _ = talaria_gaps::AUDIENCE.set(Arc::new(|pg, authority| {
        Box::pin(async move {
            let d = talaria_approvals::audience_for(&pg, &authority).await;
            (d.content, d.fact)
        })
    }));

    // Home's alert glance. Unset, an admin's Home always answered 0 — and a
    // zero is indistinguishable from a healthy instance, which is the whole
    // failure mode `talaria-alerts` exists to prevent.
    let _ = talaria_home::COMPUTE_ALERT_COUNT.set(Arc::new(|state, user_id| {
        Box::pin(async move { talaria_alerts::compute_alerts(&state, &user_id).await.len() as i32 })
    }));

    // The Titler. Unset, `generate_title` returned None on its `?` and nothing
    // was ever renamed: every chat and plan kept its mechanical first-message
    // truncation.
    let _ = talaria_titler::GENERATE_TITLE.set(Arc::new(|state, kind, text| {
        Box::pin(async move {
            use talaria_harness_defs::defs::titler as def;
            let kind = match kind {
                talaria_titler::TitleKind::Chat => def::TitleKind::Chat,
                talaria_titler::TitleKind::Plan => def::TitleKind::Plan,
                talaria_titler::TitleKind::Research => def::TitleKind::Research,
            };
            let input = serde_json::json!(def::TitlerInput { kind, text });
            talaria_harness::run::run_harness(
                &state,
                &def::titler_harness(),
                &input,
                talaria_harness::run::RunContext {
                    caller: "platform:titler".into(),
                    ..talaria_harness::run::RunContext::default()
                },
            )
            .await
            .ok()
            .and_then(|r| r.value)
            // A text harness's value is a `Value::String` (define.rs's
            // type-erasure states it), so the title is read out of it rather
            // than off a field.
            .and_then(|v| v.as_str().map(str::to_string))
        })
    }));

    // The skill summarizer. Unset, the line was always None, so nothing was
    // ever inserted into `skill_summaries` — the catalogue kept whatever it
    // had on 2026-09-18.
    let _ = talaria_agent_skills::SUMMARIZE_SKILL.set(Arc::new(|state, md| {
        Box::pin(async move {
            let input =
                serde_json::json!(talaria_harness_defs::defs::summarizer::SummarizerInput { md });
            talaria_harness::run::run_harness(
                &state,
                &talaria_harness_defs::defs::summarizer::summarizer_harness(),
                &input,
                talaria_harness::run::RunContext {
                    caller: "platform:summarizer".into(),
                    ..talaria_harness::run::RunContext::default()
                },
            )
            .await
            .ok()
            .and_then(|r| r.value)
            .and_then(|v| v.as_str().map(str::to_string))
        })
    }));

    // The push side. Unset, a ticket entering an agent-start column dispatched
    // NOTHING — the agent was never handed the work.
    let _ = talaria_tasks_types::MAYBE_DISPATCH_TICKET.set(Arc::new(
        |pg, dispatch, ticket, only_agents| {
            Box::pin(async move {
                // The Option is vestigial: both callers in `talaria-tasks`
                // already do `let Some(dispatch) = … else { return }` and pass
                // `&Some(dispatch)`. Unreachable in practice, and a dispatch
                // with no run assembly has nothing to dispatch WITH, so the
                // honest answer is to do nothing rather than invent deps.
                let Some(deps) = dispatch else { return };
                talaria_work_dispatch::maybe_dispatch_ticket(
                    &pg,
                    &deps,
                    &ticket,
                    only_agents.as_deref(),
                )
                .await;
            })
        },
    ));

    // The task writer every non-engine caller goes through. Unset, it answered
    // a refusal — "task update is not wired" — rather than writing.
    let _ = talaria_tasks_types::UPDATE_TASK.set(Arc::new(|deps, id, patch, actor| {
        Box::pin(async move {
            // `update_task` answers `Option<Task>` — None is "no such task".
            // The seam's contract is a `Task`, so the miss becomes a refusal
            // rather than being flattened into a success nobody can read.
            match talaria_tasks::update_task(&deps, &id, patch, &actor).await {
                Ok(Some(t)) => Ok(t),
                Ok(None) => Err(talaria_tasks_types::TaskError::Refusal(
                    "no such task".into(),
                )),
                Err(e) => Err(e),
            }
        })
    }));

    // The ticket room's fan-out. Unset, an agent's reply in a task room stayed
    // in the room and never became a ticket comment.
    let _ = talaria_channel_replies::ROOM_COMMENT_FANOUT.set(Arc::new(
        |pg, realtime, meta, mid, author, body| {
            Box::pin(async move {
                talaria_tasks::room_comment_fanout(&pg, &realtime, &meta, &mid, &author, &body)
                    .await;
            })
        },
    ));

    // A personal assistant's private-doc sync. Unset, a fresh assistant never
    // indexed its owner's private docs.
    let _ = talaria_personal_agent::SYNC_PRIVATE_DOCS.set(Arc::new(|pg, user_id| {
        Box::pin(async move {
            // The seam carries only (pg, user_id); the retrieval edges are
            // this layer's to supply, exactly as every other indexing caller
            // supplies them.
            let qd = talaria_retrieval_qdrant::real_deps();
            let ed = talaria_retrieval_embed::real_deps();
            let _ = talaria_kb::sync_user_private_docs(&pg, &qd, &ed, &user_id).await;
        })
    }));

    // The Workbench's half of the agent toolkit. Unset, `workbench_tools()`
    // answered an empty list and the toolkit simply did not offer them.
    let _ = talaria_mcp::WORKBENCH_TOOLS.set(Arc::new(talaria_workbench_mcp::workbench_tools));

    // The attach-chip resolvers, and the ACL is the point: both restore the
    // pre-extraction read AND its permission check, so a ref to a doc the
    // reader may not see still resolves to nothing. Unset, every `@`-reference
    // in a message resolved to nothing at all, which looked identical to
    // "forbidden" and was not.
    let _ = talaria_refs::RESOLVE_KB_REF.set(Arc::new(|pg, user_id, author, ref_id| {
        Box::pin(async move {
            let doc = talaria_kb::get_doc(&pg, &ref_id).await.ok().flatten()?;
            let effective = talaria_kb::effective_doc_perms(&pg, &doc).await.ok()?;
            let team_ids = talaria_teams::team_ids_for_user(&pg, &user_id)
                .await
                .unwrap_or_default();
            if !talaria_kb_perms::can_read(
                &effective.perms,
                Some(&user_id),
                author.as_deref(),
                &effective.grants,
                &team_ids,
            ) {
                return None;
            }
            let filename = if doc.title.is_empty() {
                "Untitled".to_string()
            } else {
                doc.title.clone()
            };
            Some((doc.id.clone(), filename, doc.body.clone()))
        })
    }));
    let _ = talaria_refs::RESOLVE_ARTIFACT_REF.set(Arc::new(|pg, user_id, author, ref_id| {
        Box::pin(async move {
            let artifact = talaria_artifacts::get_artifact(&pg, &ref_id)
                .await
                .ok()
                .flatten()?;
            let grants =
                talaria_kb_perms::list_editors(&pg, talaria_kb_perms::ITEM_ARTIFACT, &artifact.id)
                    .await
                    .ok()?;
            let team_ids = talaria_teams::team_ids_for_user(&pg, &user_id)
                .await
                .unwrap_or_default();
            if !talaria_kb_perms::can_read(
                &talaria_artifacts::guarded(&artifact),
                Some(&user_id),
                author.as_deref(),
                &grants,
                &team_ids,
            ) {
                return None;
            }
            let filename = if artifact.title.is_empty() {
                "Untitled".to_string()
            } else {
                artifact.title.clone()
            };
            Some((
                artifact.id.clone(),
                filename,
                talaria_artifacts::artifact_to_markdown(&artifact),
            ))
        })
    }));

    // A plain fn pointer rather than an Arc, so it sets differently — and it
    // is the reason a prior scan for `.get()` arms missed it. Unset, the
    // agent's own prior messages carried no reference blocks, so an agent
    // re-reading a thread lost every `@`-reference it had already been given.
    let _ = talaria_conversations::REF_BLOCKS.set(talaria_refs::ref_blocks);
    // THE TICKET-THREAD GATE, and it had been dark for sixteen days.
    //
    // `ticket_message_relevant` is the only door between a human message on a
    // ticket's discussion and the assigned agent's turn. It settles the
    // structural cases itself (an attachments-only handoff, a bare empty turn,
    // a message naming the ticket ref) and hands everything else to this
    // seam — so an unset seam is not a degraded gate, it is NO gate: the
    // `None` arm answers `true`, every remaining message reads as the agent's
    // business, and the agent is a roommate again. Exactly the failure the
    // harness's own header costed as unacceptable in the other direction.
    //
    // HOW IT WENT DARK, because the shape is worth recognising. 82eb786e
    // ("api: extract ticket-thread model surfaces") moved this module into its
    // own crate and introduced the OnceLock to break the dependency on the
    // harness runner — correctly — but never added the setter. Its own
    // verification line reads "Verified: cargo check -p talaria-api", and
    // `cargo check` cannot see an unset `OnceLock`: the declaration compiles,
    // the read compiles, and the fallback is a valid answer. The live
    // instance's `harness_runs` shows the shape precisely — nine
    // 'ticket-relevance' rows between 2026-09-18 02:37 and 2026-09-19 00:28
    // UTC, the extraction landing at 04:28 that morning, and not one row
    // since.
    //
    // The fold below is the pre-extraction code's, verbatim in behaviour: a
    // harness error, a null verdict, or any value that is not
    // {"relevant": bool} all answer TRUE. The gate may cost an unneeded
    // reply; it may never cost an unanswered one.
    // The census id this site records and reads its switch under.
    const SITE_TICKET: &str = "ticket-relevance";
    let _ = talaria_ticket_chat::TICKET_RELEVANT.set(Arc::new(
        |state, ticket, work, message, recent| {
            Box::pin(async move {
                // Kept before the move into the harness input: the shadow
                // comparison below needs the same state, and the ticket line
                // is the row an operator opens to audit a disagreement.
                let ticket_line = ticket.clone();
                let subject = serde_json::json!({
                    "ticket": ticket,
                    "work": work,
                    "message": message,
                    "recent": recent,
                });
                let input = serde_json::json!(
                    talaria_harness_defs::defs::ticket_relevance::TicketRelevanceInput {
                        ticket,
                        work,
                        message,
                        recent,
                    }
                );
                // THE QUESTION, built once and used by whichever path runs.
                // Both askers get the harness's OWN definitions
                // (`RELEVANT_MEANS` / `NOT_RELEVANT_MEANS`), because a
                // comparison between two differently-worded questions
                // measures the wording.
                let ask = talaria_decide::Ask::new(subject).q(
                    "relevant",
                    talaria_decide::Question::noul_meaning(
                        talaria_harness_defs::defs::ticket_relevance::RELEVANCE_QUESTION,
                        talaria_harness_defs::defs::ticket_relevance::RELEVANT_MEANS,
                        talaria_harness_defs::defs::ticket_relevance::NOT_RELEVANT_MEANS,
                    ),
                );

                // ── THE CASCADE ─────────────────────────────────────────────
                //
                // Switched OFF (the default): the harness decides and the port
                // is asked the same question detached, with nobody listening.
                // That is double cost, which is the price of the measurement
                // and a reason not to leave a site there forever.
                //
                // Switched ON: the port is asked FIRST and a confident answer
                // is taken — skipping the harness turn entirely, which is the
                // gate's whole economics ("cheaper than the reply it
                // prevents"). An UNCERTAIN answer falls through to the
                // harness, so the expensive judge still runs on exactly the
                // messages it is worth running on. The floor is read as
                // CERTAINTY rather than as a lean, because a confident NO is
                // the answer that saves the turn and a confident YES says what
                // the fail-open default already said.
                let gate = talaria_decide::gate_of(&state.pg, SITE_TICKET).await;
                let def = talaria_decide::sites::def_of(SITE_TICKET);
                let judged = if gate.on {
                    let http = talaria_retrieval_http::real_http();
                    talaria_decide::decide(&state, &http, &ask)
                        .await
                        .and_then(|mut m| m.remove("relevant"))
                } else {
                    None
                };
                if let Some(def) = def
                    && let Some(j) = talaria_decide::sites::acts_on(def, &gate, judged.clone())
                    && let Some(p) = j.answer.probability()
                {
                    // No baseline: the harness did not run, so there is no
                    // second answer. `record_acted` is what keeps that honest
                    // in the ledger.
                    talaria_decide::shadow::record_acted(
                        &state.pg,
                        SITE_TICKET,
                        Some(&ticket_line),
                        &j,
                    )
                    .await;
                    return p >= 0.5;
                }

                // THE FAIL-OPEN FOLD, the pre-extraction code's verbatim:
                // every way this comes back empty — harness error, null
                // verdict, a value shaped like anything but
                // {"relevant": bool} — is TRUE.
                let relevant = talaria_harness::run::run_harness(
                    &state,
                    &talaria_harness_defs::defs::ticket_relevance::ticket_relevance_harness(),
                    &input,
                    talaria_harness::run::RunContext {
                        caller: "platform:ticket-relevance".into(),
                        ..talaria_harness::run::RunContext::default()
                    },
                )
                .await
                .ok()
                .and_then(|r| r.value)
                .and_then(|v| v.get("relevant").and_then(serde_json::Value::as_bool))
                .unwrap_or(true);
                // SHADOW MODE, and it is deliberately the last thing that
                // happens: `relevant` is already decided and about to be
                // returned, so nothing below can change the answer. It is a
                // no-op unless an operator has configured a decision model —
                // `off` on every install by default.
                let cmp = talaria_decide::shadow::Compare {
                    site: SITE_TICKET,
                    subject_ref: Some(ticket_line),
                    baseline: relevant.to_string(),
                    agrees: talaria_decide::shadow::noul_agrees,
                };
                match judged {
                    // The site is switched on and the port answered, but below
                    // its floor — so the harness ran and we have a real
                    // baseline for an answer already paid for. Recording it is
                    // what makes the uncertain middle visible: a floor lowered
                    // from here is lowered against these rows.
                    Some(_) => {
                        talaria_decide::shadow::record(&state.pg, &cmp, judged.as_ref()).await;
                    }
                    // Nothing asked yet (the common case: the site is off), or
                    // asked and silent. `compare` asks detached and costs this
                    // turn nothing.
                    None if !gate.on => {
                        talaria_decide::shadow::compare(&state, cmp, ask);
                    }
                    // Asked and the port said nothing. The silence belongs in
                    // the denominator, and asking again would not change it.
                    None => {
                        talaria_decide::shadow::record(&state.pg, &cmp, None).await;
                    }
                }
                relevant
            })
        },
    ));
    // THE TWO GET_TASK EDGES — same disease as CONVERSATION_OWNER, found the
    // same week by the live suite's first runs: workchains' turn/pause paths
    // and inbox-focus's focus scoring both `.expect("GET_TASK")` on a seam
    // nothing ever set, so those paths PANICKED in production (caught by
    // catch-panic as opaque 500s) instead of doing their work.
    let _ = talaria_workchains::GET_TASK.set(std::sync::Arc::new(|pg, id| {
        Box::pin(async move { talaria_tasks::get_task(&pg, &id).await })
    }));
    let _ = talaria_inbox_focus::GET_TASK.set(std::sync::Arc::new(|pg, id| {
        Box::pin(async move { talaria_tasks::get_task(&pg, &id).await })
    }));
    // THE THIRD EDGE, same disease, missed when the two above were fixed:
    // inbox-focus's review card resolves its Approve / Request-changes click
    // through COMPLETE_QUALITY_REVIEW and `.expect()`s it, so every click on a
    // review card in the brief PANICKED on an unset seam. Registering it is
    // what makes that card's two buttons do their work.
    let _ = talaria_inbox_focus::COMPLETE_QUALITY_REVIEW.set(std::sync::Arc::new(
        |deps, task_id, reviewer, review_status, next_status| {
            Box::pin(async move {
                talaria_tasks::complete_quality_review(
                    &deps,
                    &task_id,
                    &reviewer,
                    &review_status,
                    &next_status,
                )
                .await
            })
        },
    ));
    // THE KB-BODY RUNG — same disease, found before it bit this time. The
    // seam (uploads' KB_DOC_ALLOWS_READ) shipped in the extraction commit
    // with its resolver never registered, so can_access_upload's "an upload
    // embedded in a KB doc is readable by whoever can read the doc" branch
    // read an unset OnceLock and fell through to the reach query — every
    // non-owner reader of an embedded image got a 404. Mirrors the
    // pre-extraction logic (get_doc → effective_doc_perms → can_read,
    // teams for grant matching); fails closed like every other rung: a
    // perms read that errors answers false.
    let _ =
        talaria_uploads::KB_DOC_ALLOWS_READ.set(std::sync::Arc::new(|pg, doc_id, user_id, who| {
            Box::pin(async move {
                let Ok(Some(doc)) = talaria_kb::get_doc(&pg, &doc_id).await else {
                    return false;
                };
                let Ok(effective) = talaria_kb::effective_doc_perms(&pg, &doc).await else {
                    return false;
                };
                let team_ids = talaria_teams::team_ids_for_user(&pg, &user_id)
                    .await
                    .unwrap_or_default();
                talaria_kb_perms::can_read(
                    &effective.perms,
                    Some(&user_id),
                    who.as_deref(),
                    &effective.grants,
                    &team_ids,
                )
            })
        }));
    talaria_price_oracle::register_price_refresh_job(Arc::new(PriceRefreshDeps {
        pg: state.pg.clone(),
    }));
    talaria_digest::register_digest_job(real_digest_deps(state, rt.clone()));
    talaria_digest::register_approval_escalation_job(real_digest_deps(state, rt.clone()));
    talaria_notify::register_notification_mail_job(real_drain_deps(state.pg.clone(), sb.clone()));
    talaria_work_dispatch::register_redispatch_job(state.pg.clone(), run.clone());
    talaria_runs_reclaim::register_reclaim_job(Arc::new(ReclaimDeps {
        due: due_fn(run.store.clone()),
        definition_for: run.definition_for.clone(),
        drive: drive_fn(run.clone()),
        now: run.now.clone(),
    }));
    talaria_daily_brief::register_daily_brief_job(real_brief_deps(state).await);
    talaria_model_info::register_blurb_rewrite_job(Arc::new(BlurbDeps {
        state: state.clone(),
    }));
    talaria_scheduler::register_job(talaria_fleet_resources::resource_job_spec(state.pg.clone()));
    talaria_scheduler::register_job(talaria_workbench_queue::promotion_job_spec(
        state.pg.clone(),
    ));
    talaria_scheduler::register_job(talaria_workbench_mcp::teardown::job_sweep_spec(
        state.pg.clone(),
    ));
    // The optional trio. mcp-library-refresh is per-instance cache warming
    // and arms on every Rust instance; update-check and update-reconcile
    // self-gate by install mode (a quiet no-op on checkout/dev/off) and by
    // adoption — the engine acts only on instances that handed over the
    // keys (the minute hand's reconcile is one settings-row read when idle).
    talaria_mcp_library::register_mcp_library_refresh_job(talaria_mcp_library::library());
    talaria_mcp::pkg::register_pkg_reconcile_job(state.pg.clone());
    talaria_update_job::register_update_check_job(Arc::new(talaria_update_job::UpdateDeps {
        state: state.clone(),
    }));
    talaria_update_job::register_update_reconcile_job(Arc::new(talaria_update_job::UpdateDeps {
        state: state.clone(),
    }));
}

/// The census kinds this build cannot define. Empty is arming's
/// precondition; non-empty is the checklist, spelled out for the operator.
fn missing_run_kinds() -> Vec<&'static str> {
    BOOT_RUN_KINDS
        .iter()
        .filter(|k| run_definition(k).is_none())
        .copied()
        .collect()
}

/// The boot step: build every real edge, declare the table, arm.
///
/// RETRIED, not fatal. Boot is not allowed to depend on Postgres or Redis
/// being up (the pools are lazy for exactly that reason), but arming needs a
/// live ConnectionManager for the leases and a loaded secretbox for the mail
/// drain. Rather than exit — which would take the API down with the schedule,
/// a crash loop that serves nobody — this waits and tries again every 5s
/// until the dependencies answer. A schedule that never arms is the "work
/// that silently never happens" failure this whole plane exists to end, so
/// the first failure and then every ~30s worth is said out loud, and the
/// message names what is missing (a dead dependency, or run kinds this
/// build cannot define — the one condition that will not fix itself).
pub async fn arm(state: AppState) {
    let mut attempt: u32 = 0;
    loop {
        attempt += 1;
        match try_arm(&state).await {
            Ok(()) => return,
            Err(e) => {
                if attempt == 1 || attempt.is_multiple_of(6) {
                    tracing::warn!(
                        "{LOG} cannot arm the schedule yet (attempt {attempt}): {e} — retrying every 5s"
                    );
                }
            }
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

async fn try_arm(state: &AppState) -> Result<(), String> {
    let conn = state
        .redis()
        .await
        .map_err(|e| format!("redis is unreachable: {e}"))?;
    let sb = state
        .secretbox()
        .await
        .map_err(|e| format!("the secretbox did not load: {e}"))?;

    // The six armed run steps: their deps are the AppState's edges, and an
    // unarmed step is the loud refusal in the def — reached only by a driver
    // armed before its deps, which this order makes impossible.
    talaria_research_def::arm_research_step(talaria_research_def::real_research_deps(
        state.clone(),
    ));
    talaria_runs_plan_draft::arm_plan_draft_step(talaria_runs_plan_draft::real_plan_draft_deps(
        state.clone(),
    ));
    talaria_runs_work_session::arm_work_session_step(
        talaria_runs_work_session::real_work_session_deps(state.clone()),
    );
    talaria_runs_agent_hire::arm_agent_hire_step(talaria_runs_agent_hire::real_agent_hire_deps(
        state.clone(),
    ));
    talaria_runs_reindex::arm_backfill_step(talaria_runs_reindex::real_backfill_deps(
        state.clone(),
    ));
    talaria_runs_reindex::arm_reindex_step(talaria_runs_reindex::real_reindex_deps(state.clone()));
    // The boot kind list: touch each getter so its kind registers NOW, then
    // hold arming to the census's table.
    let _ = talaria_research_def::research_run();
    let _ = talaria_runs_plan_draft::plan_draft_run();
    let _ = talaria_runs_work_session::work_session_run();
    let _ = talaria_runs_agent_hire::agent_hire_run();
    let _ = talaria_runs_reindex::backfill_run();
    let _ = talaria_runs_reindex::reindex_run();
    let missing = missing_run_kinds();
    if !missing.is_empty() {
        return Err(format!(
            "run kinds with no definition in this build: {} — the sweep would strand their rows. \
             The schedule arms when the census's kind table is whole.",
            missing.join(", ")
        ));
    }

    let rt = RealtimeDeps::publish_only(Some(conn.clone()));
    let run = Arc::new(real_run_deps(state.pg.clone(), conn.clone(), rt.clone()));
    register_all(state, run, rt, &sb).await;
    // The push plane rides the same boot, one line below registration: one
    // install and every row the single notification writer files can also
    // reach closed tabs. Here in try_arm rather than register_all on
    // purpose — the completeness test drives register_all with a dead lazy
    // pool, and the plane's OnceLock must hold the real one or nothing.
    talaria_push::install_push_plane(state.pg.clone(), sb.clone());
    // start_scheduler runs the missing-jobs boot check and logs the armed
    // summary itself; its return is that list again.
    let armed = scheduler::start_scheduler(conn);
    tracing::info!(
        "{LOG} the schedule is this process's — {} job(s) declared, {} armed",
        REQUIRED_JOBS.len() + 1,
        armed.len()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use talaria_scheduler::JobName;

    // The completeness test injects fake edges because registration only
    // captures them. The real constructors are each tested in their own
    // module; what this pins is the LIST — the one thing no other test can,
    // because a register call that falls out of `register_all` is silent
    // everywhere except production.
    #[tokio::test]
    async fn register_all_declares_the_whole_ported_table() {
        let url = "postgres://jobs-flip-test@localhost:5432/jobs-flip-test";
        let pg = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy(url)
            .expect("a lazy pool connects to nothing");
        let cfg = talaria_config::Config::from_parts(
            url.into(),
            "redis://jobs-flip-test@localhost:6379".into(),
            "test-root".into(),
            String::new(),
            String::new(),
            "0".into(),
        )
        .expect("the test config is valid on its face");
        let state = AppState::new(pg, Arc::new(cfg));
        let sb = SecretBox::from_parts([0u8; 32], std::collections::HashMap::new(), None);
        register_all(
            &state,
            Arc::new(test_run_deps()),
            RealtimeDeps::publish_only(None),
            &sb,
        )
        .await;

        let names: Vec<JobName> = scheduler::scheduler_status(0)
            .iter()
            .map(|s| s.name)
            .collect();
        for required in REQUIRED_JOBS {
            assert!(
                names.contains(required),
                "{} fell out of the job table",
                required.as_str()
            );
        }
        assert!(
            names.contains(&JobName::McpLibraryRefresh),
            "mcp-library-refresh fell out of the job table"
        );
        // The retired hold, pinned from the other side: update-check IS in
        // the table now (the update engine's restart topology landed with
        // it), and it stays out of REQUIRED_JOBS — a no-op job on dormant
        // installs must not fail anyone's boot.
        assert!(
            names.contains(&JobName::UpdateCheck),
            "update-check fell out of the job table — the scheduled check silently stopped"
        );
        assert!(
            !REQUIRED_JOBS.contains(&JobName::UpdateCheck),
            "update-check must stay optional: dormant installs legitimately run it as a no-op"
        );
        // The minute hand, same contract: a stranded cutover must not wait
        // hours for a reader, so its finisher must never silently stop.
        assert!(
            names.contains(&JobName::UpdateReconcile),
            "update-reconcile fell out of the job table — a stranded cutover would wait hours for a reader"
        );
        assert!(
            !REQUIRED_JOBS.contains(&JobName::UpdateReconcile),
            "update-reconcile must stay optional: dormant installs legitimately run it as a no-op"
        );

        // THE ROLL'S EDGES, the same completeness rule one layer down. These
        // OnceLocks are how the reconcile crate reaches the renderer, and an
        // unwired one makes `roll_agent` return "… not wired" — a sentence
        // the control route discards while answering `{"rolling": true}`.
        // Nothing else in the crate graph runs boot, so this is the only
        // place the absence can be caught before an operator notices that
        // their agents never changed.
        assert!(
            talaria_fleet_reconcile::RENDER_FLEET.get().is_some(),
            "the roll's overlay renderer fell out of the boot wiring — every roll would silently do nothing"
        );
        assert!(
            talaria_fleet_reconcile::NEXT_FREE_PORT.get().is_some(),
            "the roll's port allocator fell out of the boot wiring — every roll would silently do nothing"
        );
        assert!(
            talaria_update_job::ROLL_FLEET.get().is_some(),
            "the post-deploy fleet roll fell out of the boot wiring — an update would land and leave every agent on the old container"
        );
        // The announce edge a queued Google write calls. This assertion is the
        // one that was missing, and its absence is the whole story: the edge
        // was declared, the call site was written with a careful comment about
        // the five minutes it saves, and nothing ever set it. The call site
        // reads `if let Some(f) = …get()`, so an unwired edge compiles, logs
        // nothing, and quietly reverts to the sweep.
        assert!(
            talaria_google_pending::ANNOUNCE_APPROVAL.get().is_some(),
            "the queued-write announce fell out of the boot wiring — every confirm-send would wait for the approval sweep with an agent stopped in front of it"
        );
        // THE RUN ASSEMBLY request-path enqueues use. Unset, every plan-draft
        // POST is a 500 ("dispatch not wired" → "could not start the plan
        // draft"), and research start and guided reindex die the same way.
        // Nothing else in the crate graph runs boot, so this is the only
        // place the absence can be caught before drafting tickets does nothing.
        assert!(
            talaria_tasks_types::BUILD_DISPATCH.get().is_some(),
            "BUILD_DISPATCH fell out of the boot wiring — plan drafts, research, and reindex cannot enqueue"
        );
        // THE TICKET-THREAD GATE, and this assertion is the one that would
        // have caught sixteen days of no gate at all. `ticket_message_relevant`
        // is the only door between a human message on a ticket's discussion
        // and the assigned agent's turn, and its `None` arm answers TRUE — so
        // an unwired edge is not a worse gate, it is the agent answering every
        // message in the room. The extraction that introduced the edge
        // verified with `cargo check`, which cannot see this.
        assert!(
            talaria_ticket_chat::TICKET_RELEVANT.get().is_some(),
            "the ticket-thread gate fell out of the boot wiring — the assigned agent would reply to every message on every ticket discussion, including people talking to each other"
        );

        // THE REST OF THE EXTRACTION WAVE. The ticket gate above was one of
        // twelve edges a burst of crate extractions declared between
        // 2026-09-18 and 2026-09-19 and set none of. One assertion each,
        // naming what the silence costs, because the `None` arm of every one
        // is a valid answer and therefore invisible to the compiler.
        for (edge, cost) in [
            (
                talaria_gaps::AUDIENCE.get().is_some(),
                "the capability-gap audience: a gap is filed and nobody is told",
            ),
            (
                talaria_home::COMPUTE_ALERT_COUNT.get().is_some(),
                "Home's alert count: an admin's Home always reads 0, which looks like a healthy instance",
            ),
            (
                talaria_titler::GENERATE_TITLE.get().is_some(),
                "the Titler: every chat and plan keeps its mechanical first-message truncation",
            ),
            (
                talaria_agent_skills::SUMMARIZE_SKILL.get().is_some(),
                "the skill summarizer: nothing is ever written to skill_summaries",
            ),
            (
                talaria_tasks_types::MAYBE_DISPATCH_TICKET.get().is_some(),
                "the push side: a ticket entering an agent-start column dispatches nothing",
            ),
            (
                talaria_tasks_types::UPDATE_TASK.get().is_some(),
                "the task writer: every update through it refuses with 'task update is not wired'",
            ),
            (
                talaria_channel_replies::ROOM_COMMENT_FANOUT.get().is_some(),
                "the task room's fan-out: an agent's reply never becomes a ticket comment",
            ),
            (
                talaria_personal_agent::SYNC_PRIVATE_DOCS.get().is_some(),
                "a personal assistant's private-doc sync: a fresh assistant indexes nothing",
            ),
            (
                talaria_mcp::WORKBENCH_TOOLS.get().is_some(),
                "the Workbench's half of the toolkit: agents are offered an empty tool list",
            ),
            (
                talaria_refs::RESOLVE_KB_REF.get().is_some(),
                "the KB attach chip: every @-reference to a doc resolves to nothing",
            ),
            (
                talaria_refs::RESOLVE_ARTIFACT_REF.get().is_some(),
                "the artifact attach chip: every @-reference to an artifact resolves to nothing",
            ),
            (
                talaria_conversations::REF_BLOCKS.get().is_some(),
                "reference blocks on prior turns: an agent re-reading a thread loses its own refs",
            ),
        ] {
            assert!(edge, "{cost} — the edge fell out of the boot wiring");
        }
    }

    #[test]
    fn arming_refuses_without_the_whole_kind_table() {
        // Touch the getters exactly as try_arm does, so the registry reflects
        // a real boot and not whatever earlier tests loaded.
        let _ = talaria_research_def::research_run();
        let _ = talaria_runs_plan_draft::plan_draft_run();
        let _ = talaria_runs_work_session::work_session_run();
        let _ = talaria_runs_agent_hire::agent_hire_run();
        let _ = talaria_runs_reindex::backfill_run();
        let _ = talaria_runs_reindex::reindex_run();
        // The census's kind table is WHOLE. An empty list is the assertion
        // now, not the goal: a kind showing up here means a def module fell
        // out of try_arm's boot list, and the sweep would strand that kind's
        // rows from the moment arming succeeds.
        let missing = missing_run_kinds();
        assert!(
            missing.is_empty(),
            "the census's kind table has holes: {missing:?} — every BOOT_RUN_KINDS entry \
             needs its getter touched in try_arm"
        );
    }

    /// The lease edge no test can build (ConnectionManager dials on
    /// construction) and no registration ever invokes. Same shape as
    /// work_dispatch's test deps.
    fn test_run_deps() -> RunDeps {
        use futures_util::future::BoxFuture;
        use talaria_runs_run::{LeaseClaim, LeaseRenewal, PauseArgs, PauseOutcome, RunLease};
        use talaria_runs_store::WriteFailure;

        struct NeverLease;
        impl RunLease for NeverLease {
            fn acquire<'a>(&'a self, _: &'a str, _: i64) -> BoxFuture<'a, LeaseClaim> {
                Box::pin(std::future::pending())
            }
            fn renew<'a>(&'a self, _: &'a str, _: &'a str, _: i64) -> BoxFuture<'a, LeaseRenewal> {
                Box::pin(std::future::pending())
            }
            fn release<'a>(&'a self, _: &'a str, _: &'a str) -> BoxFuture<'a, ()> {
                Box::pin(std::future::pending())
            }
        }
        RunDeps {
            store: Arc::new(talaria_runs_store::PgRunStore::new(
                sqlx::postgres::PgPoolOptions::new()
                    .connect_lazy("postgres://jobs-flip-test@localhost:5432/jobs-flip-test")
                    .expect("a lazy pool connects to nothing"),
            )),
            lease: Arc::new(NeverLease),
            publish: Arc::new(|_, _| ()),
            pause: Arc::new(|_: PauseArgs| {
                Box::pin(async {
                    PauseOutcome::Refused {
                        reason: WriteFailure::Missing,
                        state: None,
                    }
                })
            }),
            definition_for: Arc::new(|_| None),
            now: Arc::new(|| 0),
            new_id: Arc::new(|| "new".into()),
        }
    }
}
