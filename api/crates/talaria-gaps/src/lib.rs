// Capability gaps — the honesty loop's memory. The contract with agents:
// competence first, and when work genuinely can't be done properly (missing
// tools/access, org-specific process the agent would be guessing at), report
// the gap instead of improvising. The contract with humans: no nagging —
// one row per work-shape ever, repeats bump seen_count (frequency is ranking
// signal), and the Studio's Suggested queue is where a gap gets ratified.
//
// ONE notification, on the FIRST sighting of a work-shape, and never again for
// that shape. That is not a softening of "no inbox pings" — it is the same
// promise, kept: the rule was always one-per-shape, and a queue that nothing
// ever announces is a queue nobody opens. The `gap_reported` class defaults to
// in-app, so the bell learns about a NEW kind of gap and no mail leaves the
// building unless someone asks for it. Repeats — the seen_count bumps that
// make a shape rank — say nothing at all.
//
// The refusal ladder this module guards (the
// THE RETRY argument) is behavior, not comment: `remember_ticket_refusal` and
// `agent_text_authority` carry the correlation rule verbatim.

use serde::Serialize;
use sqlx::PgPool;

use talaria_agent_auth::epoch_ms_to_iso;
use talaria_agent_writes::{WriteAuthor, guard_agent_fields};
use talaria_notify::{NotificationInput, NotifyDeps, add_notification};

use futures_util::future::BoxFuture;
use std::sync::{Arc, OnceLock};
use talaria_agent_auth::now_ms;
use talaria_runs_define::Authority;

pub static AUDIENCE: OnceLock<
    Arc<
        dyn Fn(sqlx::PgPool, Authority) -> BoxFuture<'static, (Vec<String>, Vec<String>)>
            + Send
            + Sync,
    >,
> = OnceLock::new();

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityGap {
    pub id: String,
    pub kind: String,
    pub board_id: Option<String>,
    pub agent_model: String,
    pub missing: String,
    pub needs: String,
    pub example_task_id: Option<String>,
    pub seen_count: i32,
    pub status: String,
    pub created_at: String,
    pub last_seen: String,
}

const REFUSAL_KEY: &str = "agent_ticket_refusals";
const REFUSAL_TTL_MS: i64 = 30 * 60 * 1000;

/// `slug` — the agent's own name for the kind of work, folded to a signature
/// atom: lowercase, runs of anything else become one dash, no edge dashes, at
/// most 60 chars, and an empty result is still a classifiable something.
pub fn slug(v: &str) -> String {
    let folded: String = v
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    // JS `[^a-z0-9]+` → '-' collapses runs in one pass; the map above emits
    // runs of '-', which this squeeze then collapses.
    let mut squeezed = String::with_capacity(folded.len());
    let mut prev_dash = false;
    for c in folded.chars() {
        if c == '-' {
            if !prev_dash {
                squeezed.push('-');
            }
            prev_dash = true;
        } else {
            squeezed.push(c);
            prev_dash = false;
        }
    }
    let trimmed = squeezed.trim_matches('-');
    let taken: String = trimmed.chars().take(60).collect();
    if taken.is_empty() {
        "unclassified".to_string()
    } else {
        taken
    }
}

/// Work-shape identity: the board plus the agent's own name for the kind of
/// work. Same shape reported again — from any agent — lands on the same row.
fn signature_of(board_id: Option<&str>, kind: &str) -> String {
    format!("{}|{}", board_id.unwrap_or("any"), slug(kind))
}

/// Remember that this agent was just refused a ticket.
///
/// Keyed by the AGENT and by nothing else — varying one character between the
/// refusal and the retry walks past any finer key, and the only cost of the
/// coarse one is a quieter announcement for the agent's next half hour of
/// reports, which still reaches every admin.
///
/// Never throws: this runs on the way to a 403 and must not turn a refusal
/// into a 500 — but a write that fails FAILS OPEN, which is why the read side
/// (`agent_text_authority`) fails closed. Stale entries are pruned in the same
/// statement, and the merge is `||` on jsonb so two refusals in flight at once
/// cannot erase each other.
pub async fn remember_ticket_refusal(pg: &PgPool, agent_model: &str, board_id: Option<&str>) {
    let entry = serde_json::json!({
        agent_model: {
            "boardId": board_id,
            "at": epoch_ms_to_iso(now_ms()),
        }
    });
    let cutoff = epoch_ms_to_iso(now_ms() - REFUSAL_TTL_MS);
    let res = sqlx::query(
        "insert into app_settings (key, value) values ($1, $2::jsonb) \
         on conflict (key) do update set \
           value = coalesce(( \
             select jsonb_object_agg(e.key, e.value) \
             from jsonb_each(app_settings.value) as e \
             where (e.value ->> 'at') >= $3 \
           ), '{}'::jsonb) || $2::jsonb, \
           updated_at = now()",
    )
    .bind(REFUSAL_KEY)
    .bind(&entry)
    .bind(&cutoff)
    .execute(pg)
    .await;
    if let Err(e) = res {
        tracing::error!("[gaps] could not remember the ticket refusal for \"{agent_model}\": {e}");
    }
}

/// THE authority an agent's own free text may be announced under. Both
/// agent-raised subjects ask this — `report_gap` below and the agent-problem
/// route — so the answer cannot differ between them, and neither of them
/// decides it at the call site. The result goes straight to `audience_for`,
/// which is the only thing in the product that turns an authority into people.
///
/// · a ticket the agent WAS allowed to name → that ticket's board.
/// · no ticket, no live refusal → org-wide admin; every admin gets the words.
/// · no ticket, a live refusal we could place → that board.
/// · no ticket, a live refusal we could NOT place → `Nobody`: every admin
///   still learns the report exists, none is sent the agent's words.
///
/// FAILS CLOSED. A memo we could not READ is not evidence that there was no
/// refusal; an unreadable memo is `Nobody`, not the widest option.
pub async fn agent_text_authority(
    pg: &PgPool,
    agent_model: &str,
    board_id: Option<&str>,
) -> Authority {
    if let Some(b) = board_id {
        return Authority::Admin {
            on_board: Some(b.to_string()),
        };
    }
    let memo: Option<serde_json::Value> = match sqlx::query_scalar::<_, Option<serde_json::Value>>(
        "select value -> $2 from app_settings where key = $1",
    )
    .bind(REFUSAL_KEY)
    .bind(agent_model)
    .fetch_optional(pg)
    .await
    {
        // No row at all (no memo has ever been written) reads as null — a
        // normal read, handled below, not an error path.
        Ok(memo) => memo.flatten(),
        Err(e) => {
            tracing::error!(
                "[gaps] could not read the refusal memo for \"{agent_model}\" — announcing the fact only: {e}"
            );
            return Authority::Nobody;
        }
    };
    let at_ms = memo
        .as_ref()
        .and_then(|m| m.get("at"))
        .and_then(|v| v.as_str())
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|t| t.timestamp_millis());
    match at_ms {
        Some(at) if at >= now_ms() - REFUSAL_TTL_MS => match memo
            .as_ref()
            .and_then(|m| m.get("boardId"))
            .and_then(|v| v.as_str())
        {
            Some(b) => Authority::Admin {
                on_board: Some(b.to_string()),
            },
            None => Authority::Nobody,
        },
        // Missing, malformed, or stale — the NaN-comparison ladder all lands
        // here too.
        _ => Authority::Admin { on_board: None },
    }
}

// ── WORK-SHAPE ALIGNMENT ─────────────────────────────────────────────────────
//
// WHAT THE SLUG CANNOT DO. `signature_of` is `board_id | slug(kind)`, and
// `kind` is the agent's own free-text name for the sort of work it was doing.
// So "cannot access staging DB" and "no staging database credentials" are two
// rows with `seen_count: 1` each — and `seen_count` is the ranking signal the
// Studio's Suggested queue orders by. A gap reported five ways by five agents
// never ranks, which is the exact opposite of what the honesty loop promises:
// "repeats bump seen_count (frequency is ranking signal)".
//
// THE ASYMMETRY RUNS THE OTHER WAY FROM EVERY OTHER SITE ON THIS PORT, and it
// is why the floor here is the highest of them. A false SPLIT costs ranking: a
// real gap looks rarer than it is. A false MERGE costs a gap entirely — two
// genuinely different problems collapse into one row and the second is never
// seen again, and nothing downstream can tell. So a merge must be nearly
// certain, `no_match` is always offered, and the fallback is the slug identity
// this has always used.
//
// The 0.85 and that reasoning now live in `talaria_decide::sites`, beside the
// ledger an operator reads before moving it — and the site is registered to
// read its floor as CERTAINTY, which for a Choice is the concentration of the
// distribution. There is no "probability of yes" to lean on when the answer is
// one option out of many.

/// A gap already on the board, as the alignment judgment sees it.
struct GapCandidate {
    signature: String,
    kind: String,
    missing: String,
}

/// Open gaps on this board, newest first and bounded. Capped because the
/// judgment is a Choice over them and a hundred options is not a decision —
/// and because the recent ones are where a duplicate of a fresh report lives.
async fn open_candidates(pg: &PgPool, board_id: Option<&str>) -> Vec<GapCandidate> {
    let rows: Vec<(String, String, String)> = sqlx::query_as(
        "select signature, kind, missing from capability_gaps \
          where status = 'open' \
            and (($1::text is null and board_id is null) or board_id::text = $1) \
          order by last_seen desc limit 20",
    )
    .bind(board_id)
    .fetch_all(pg)
    .await
    .unwrap_or_default();
    rows.into_iter()
        .map(|(signature, kind, missing)| GapCandidate {
            signature,
            kind,
            missing,
        })
        .collect()
}

/// The signature this report should file under: an existing open gap's when a
/// decision model is nearly certain it is the same work-shape, else `None` and
/// the caller files under the report's own slug.
///
/// Answers `None` for every reason the port can have nothing — off (the
/// default), unconfigured, unable to serve a Choice, below the floor — so an
/// install that never configures a decision model behaves exactly as it always
/// has.
pub async fn aligned_signature(
    state: &talaria_state::AppState,
    board_id: Option<&str>,
    kind: &str,
    missing: &str,
) -> Option<String> {
    let candidates = open_candidates(&state.pg, board_id).await;
    if candidates.is_empty() {
        return None;
    }
    // An exact slug hit needs no judgment: the insert already collapses it.
    let own = signature_of(board_id, kind);
    if candidates.iter().any(|c| c.signature == own) {
        return None;
    }

    let mut options = vec![talaria_decide::Opt::new(
        NO_MATCH,
        "none of them — this is a different problem from every gap listed, and belongs in its own row",
    )];
    for (i, c) in candidates.iter().enumerate() {
        options.push(talaria_decide::Opt::new(
            i.to_string(),
            format!("{} — {}", c.kind.trim(), c.missing.trim()),
        ));
    }

    let ask = talaria_decide::Ask::new(serde_json::json!({
        "new_report": { "kind": kind, "missing": missing },
    })).q(
        "same",
        talaria_decide::Question::choice(
            "Which of the listed gaps is THE SAME underlying problem as `new_report`? Two reports are the same problem when fixing one would fix the other — not merely when they are about the same system or the same board.",
            options,
        ),
    );

    let http = talaria_retrieval_http::real_http();
    let answers = talaria_decide::decide(state, &http, &ask).await?;
    let j = answers.get("same")?;
    // Every judgment is recorded, including the no-matches and the ones below
    // the floor: an agreement rate over only the merges is not one.
    talaria_decide::shadow::record(
        &state.pg,
        &talaria_decide::shadow::Compare {
            site: "gap-align",
            subject_ref: Some(own.clone()),
            // The slug pass said "no existing row" about all of these; that is
            // what made them candidates.
            baseline: NO_MATCH.to_string(),
            agrees: |a, baseline| a.chosen() == Some(baseline),
        },
        Some(j),
    )
    .await;

    // The switch and the floor are the operator's, read per call through the
    // site census — `None` means "run your own path", which here is opening a
    // new row, exactly what the slug pass already decided to do.
    let j = talaria_decide::acted(&state.pg, "gap-align", Some(j.clone())).await?;
    let chosen = j.answer.chosen()?;
    if chosen == NO_MATCH {
        return None;
    }
    let i: usize = chosen.parse().ok()?;
    candidates.get(i).map(|c| c.signature.clone())
}

/// The option id meaning "a new row". A named constant because it is compared
/// in two places and a typo would silently turn every no-match into a parse
/// failure — which answers None too, so nothing would ever look wrong.
const NO_MATCH: &str = "none";

/// What `report_gap` hands back to the route: the row's id, its frequency,
/// and whether THIS call was the shape's first sighting.
pub struct ReportedGap {
    pub id: String,
    pub seen_count: i32,
    pub first: bool,
}

/// The write. The agent's `missing`/`needs` go through the one
/// door first (`capability-gap` surface: the likeliest of all the tools to
/// quote a credential, and none of its outputs were scanned before), then the
/// authority decides BOTH the row's board and the announcement's audience —
/// so a retry after a refusal collapses onto the honest report's signature
/// and announces nothing the second time.
pub async fn report_gap(
    deps: &NotifyDeps,
    input: report_gap::GapInput<'_>,
) -> Result<ReportedGap, sqlx::Error> {
    let mut guarded = [
        Some(input.missing.to_string()),
        input.needs.map(|n| n.to_string()),
    ];
    guard_agent_fields(
        &deps.pg,
        "capability-gap",
        WriteAuthor::Agent(input.agent_model),
        &mut guarded,
        None,
    )
    .await;
    let missing = guarded[0]
        .clone()
        .unwrap_or_else(|| input.missing.to_string());
    let needs = guarded[1].clone();

    let authority = agent_text_authority(&deps.pg, input.agent_model, input.board_id).await;
    let board_id = match &authority {
        Authority::Admin { on_board } => on_board.clone(),
        _ => None,
    };
    // The override is a SIGNATURE, not a row id, so the insert path does not
    // change: a resolved match collides on `signature` and bumps `seen_count`
    // exactly as a repeat of the same slug always has.
    let sig = match input.signature {
        Some(s) => s.to_string(),
        None => signature_of(board_id.as_deref(), input.kind),
    };
    let (id, seen_count, first): (String, i32, bool) = sqlx::query_as(
        "insert into capability_gaps (signature, kind, board_id, agent_model, missing, needs, example_task_id) \
         values ($1, $2, $3::uuid, $4, $5, $6, $7::uuid) \
         on conflict (signature) do update set \
           seen_count = capability_gaps.seen_count + 1, \
           last_seen = now(), \
           status = case when capability_gaps.status = 'dismissed' then 'open' else capability_gaps.status end, \
           example_task_id = coalesce(capability_gaps.example_task_id, excluded.example_task_id) \
         returning id::text, seen_count, (seen_count = 1) as first",
    )
    .bind(&sig)
    .bind(slug(input.kind))
    .bind(&board_id)
    .bind(input.agent_model)
    .bind(char_take(missing.as_str(), 300))
    .bind(match needs.as_deref() {
        Some(n) => char_take(n, 5000).to_string(),
        None => String::new(),
    })
    .bind(input.task_id)
    .fetch_one(&deps.pg)
    .await?;
    if first {
        announce_gap(
            deps,
            announce_gap::GapAnnounce {
                agent_model: input.agent_model,
                kind: input.kind,
                missing: &missing,
                needs: needs.as_deref(),
                authority: &authority,
            },
        )
        .await;
    }
    Ok(ReportedGap {
        id,
        seen_count,
        first,
    })
}

/// JS `.slice(0, n)` semantics — the same result for the ASCII these fields
/// are validated as prose in, and char-boundary-safe here where
/// a plain `&s[..300]` could panic on a multibyte edge.
fn char_take(s: &str, n: usize) -> &str {
    match s.char_indices().nth(n) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

pub mod report_gap {
    pub struct GapInput<'a> {
        pub agent_model: &'a str,
        pub kind: &'a str,
        pub missing: &'a str,
        pub needs: Option<&'a str>,
        pub board_id: Option<&'a str>,
        pub task_id: Option<&'a str>,
        /// File under an EXISTING row's signature instead of this report's
        /// own — `aligned_signature` resolves it, and the caller passes what
        /// it got. `None` keeps the slug-identity behaviour exactly.
        ///
        /// Resolved by the CALLER because this module reaches a `PgPool` and
        /// nothing more: `NotifyDeps` carries no `AppState`, and asking a
        /// decision model needs one.
        pub signature: Option<&'a str>,
    }
}

async fn announce_gap(deps: &NotifyDeps, input: announce_gap::GapAnnounce<'_>) {
    use announce_gap::GapAnnounce;
    let GapAnnounce {
        agent_model,
        kind,
        missing,
        needs,
        authority,
    } = input;
    let (content, fact) = match AUDIENCE.get() {
        Some(f) => f(deps.pg.clone(), authority.clone()).await,
        None => (Vec::new(), Vec::new()),
    };
    let placed = matches!(authority, Authority::Admin { on_board: Some(_) });
    let ratify = "\n\nRatify it in the Studio's Suggested queue, or dismiss it there.";
    for user_id in &content {
        let body = format!(
            "Reported while doing {kind} work.{}{ratify}",
            match needs.map(str::trim) {
                Some(n) if !n.is_empty() => format!("\n\nWhat it needs: {}", char_take(n, 800)),
                _ => String::new(),
            }
        );
        if let Err(e) = add_notification(
            deps,
            user_id,
            &NotificationInput {
                kind: "gap_reported",
                title: &format!(
                    "{agent_model} hit a capability gap: {}",
                    char_take(missing, 160)
                ),
                body: Some(&body),
                href: Some("/studio"),
            },
        )
        .await
        {
            tracing::error!("[gaps] could not notify {user_id} of a new gap: {e}");
        }
    }
    for user_id in &fact {
        let body = format!(
            "{}{ratify}",
            if placed {
                "It was raised while working a board you are not a member of, so what the agent wrote is not repeated here."
            } else {
                "It was raised against a ticket the agent was refused, so it is not an org-wide report and what the agent wrote is not repeated here."
            }
        );
        if let Err(e) = add_notification(
            deps,
            user_id,
            &NotificationInput {
                kind: "gap_reported",
                title: &format!("{agent_model} reported a new kind of capability gap"),
                body: Some(&body),
                href: Some("/studio"),
            },
        )
        .await
        {
            tracing::error!("[gaps] could not notify {user_id} that a new gap exists: {e}");
        }
    }
}

pub mod announce_gap {
    use talaria_runs_define::Authority;
    pub struct GapAnnounce<'a> {
        pub agent_model: &'a str,
        pub kind: &'a str,
        pub missing: &'a str,
        pub needs: Option<&'a str>,
        pub authority: &'a Authority,
    }
}

const GAP_COLS: &str = "id::text, kind, board_id::text, agent_model, missing, needs, \
    example_task_id::text, seen_count, status, \
    (trunc(extract(epoch from created_at) * 1000))::bigint as created_ms, \
    (trunc(extract(epoch from last_seen) * 1000))::bigint as seen_ms";

type GapRow = (
    String,
    String,
    Option<String>,
    String,
    String,
    String,
    Option<String>,
    i32,
    String,
    i64,
    i64,
);

impl From<GapRow> for CapabilityGap {
    fn from(r: GapRow) -> Self {
        CapabilityGap {
            id: r.0,
            kind: r.1,
            board_id: r.2,
            agent_model: r.3,
            missing: r.4,
            needs: r.5,
            example_task_id: r.6,
            seen_count: r.7,
            status: r.8,
            created_at: epoch_ms_to_iso(r.9),
            last_seen: epoch_ms_to_iso(r.10),
        }
    }
}

/// The Studio's Suggested queue, ranked by frequency then
/// recency, capped at 100.
pub async fn list_gaps(
    pg: &PgPool,
    status: Option<&str>,
) -> Result<Vec<CapabilityGap>, sqlx::Error> {
    let rows: Vec<GapRow> = match status {
        Some(s) => {
            let sql = format!(
                "select {GAP_COLS} from capability_gaps where status = $1 \
                 order by seen_count desc, last_seen desc limit 100"
            );
            sqlx::query_as(sqlx::AssertSqlSafe(sql.as_str()))
                .bind(s)
                .fetch_all(pg)
                .await?
        }
        None => {
            let sql = format!(
                "select {GAP_COLS} from capability_gaps \
                 order by seen_count desc, last_seen desc limit 100"
            );
            sqlx::query_as(sqlx::AssertSqlSafe(sql.as_str()))
                .fetch_all(pg)
                .await?
        }
    };
    Ok(rows.into_iter().map(CapabilityGap::from).collect())
}

pub async fn set_gap_status(pg: &PgPool, id: &str, status: &str) -> Result<(), sqlx::Error> {
    sqlx::query("update capability_gaps set status = $1 where id = $2::uuid")
        .bind(status)
        .bind(id)
        .execute(pg)
        .await?;
    Ok(())
}

pub async fn open_gap_count(pg: &PgPool) -> Result<i32, sqlx::Error> {
    let (n,): (i32,) =
        sqlx::query_as("select count(*)::int from capability_gaps where status = 'open'")
            .fetch_one(pg)
            .await?;
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_trims_folds_and_caps_at_sixty() {
        // trim, lower, [^a-z0-9]+ → '-', edge dashes off, 60 max, else
        // 'unclassified'.
        assert_eq!(slug("  Invoice Handling!! "), "invoice-handling");
        assert_eq!(slug("---"), "unclassified");
        assert_eq!(slug(""), "unclassified");
        assert_eq!(slug("A--B___C"), "a-b-c");
        assert_eq!(slug(&"x".repeat(80)).len(), 60);
    }

    #[test]
    fn signature_separates_any_board() {
        assert_eq!(signature_of(None, "Email"), "any|email");
        assert_eq!(signature_of(Some("b1"), "Email"), "b1|email");
    }
    // ── work-shape alignment ────────────────────────────────────────────────

    #[test]
    fn the_merge_floor_is_the_highest_on_the_port_because_a_false_merge_hides_a_gap() {
        // The asymmetry runs the other way from every other site: a false
        // SPLIT costs ranking (a real gap looks rarer than it is), a false
        // MERGE costs the gap entirely — two different problems collapse into
        // one row and the second is never seen again. Behaviour, not a literal
        // compared with itself.
        let def = talaria_decide::sites::def_of("gap-align").expect("a census entry");
        assert_eq!(
            def.reads,
            talaria_decide::sites::FLOOR_CERTAINTY,
            "a Choice has no probability of yes to lean on"
        );
        let gate = talaria_decide::SiteGate {
            on: true,
            floor: def.default_floor,
        };
        // A Choice's certainty IS its concentration, so this drives the real
        // predicate through a real Choice answer rather than a stand-in.
        let acts = |certainty: f64, calibrated: bool| {
            // Two options whose shares differ by `certainty` — the port's own
            // concentration measure over a two-way split.
            let hi = 0.5 + certainty / 2.0;
            talaria_decide::sites::acts_on(
                def,
                &gate,
                Some(talaria_decide::Judgment {
                    answer: talaria_decide::Answer::Choice {
                        id: "0".into(),
                        probabilities: std::collections::BTreeMap::from([
                            ("0".to_string(), hi),
                            (NO_MATCH.to_string(), 1.0 - hi),
                        ]),
                        confidence: certainty,
                    },
                    calibrated,
                    provider: "test".into(),
                    model: None,
                    latency_ms: 1,
                    tokens_in: None,
                    tokens_out: None,
                    fanned: 1,
                }),
            )
            .is_some()
        };
        assert!(acts(0.85, true), "at the floor");
        assert!(acts(0.99, true));
        assert!(!acts(0.84, true), "just under");
        assert!(
            !acts(0.75, true),
            "the workflow floor is not high enough here"
        );
        assert!(!acts(0.99, false), "uncalibrated never merges");
        // That `0.75` case is also the claim that this is the strictest floor
        // on the port: it is the workflow pass's bar, and it does not act here.
        let workflow = talaria_decide::sites::def_of("workflow-match").expect("a census entry");
        assert!(
            def.default_floor > workflow.default_floor,
            "gap merges must stay the strictest site on the port"
        );
    }

    #[test]
    fn no_match_is_a_named_option_so_a_typo_cannot_read_as_a_merge() {
        // Compared in two places. A literal in one of them would turn every
        // no-match into a parse failure, which answers None too — so nothing
        // would ever look wrong while alignment quietly stopped happening.
        assert_eq!(NO_MATCH, "none");
        // And it can never collide with a candidate id, which are indices.
        assert!(NO_MATCH.parse::<usize>().is_err());
    }

    #[test]
    fn a_candidate_renders_as_its_kind_and_what_was_missing() {
        // The model judges "is this the same problem", so the option text has
        // to carry the problem — a bare slug would ask it to compare labels.
        let c = GapCandidate {
            signature: "b1|staging-db".into(),
            kind: "Staging DB".into(),
            missing: "credentials for the staging database".into(),
        };
        let rendered = format!("{} — {}", c.kind.trim(), c.missing.trim());
        assert_eq!(
            rendered,
            "Staging DB — credentials for the staging database"
        );
        assert_eq!(c.signature, "b1|staging-db");
    }
}
