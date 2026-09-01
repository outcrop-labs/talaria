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
// Port of ui/src/server/gaps.ts. The refusal ladder this module guards (the
// THE RETRY argument) is behavior, not comment: `remember_ticket_refusal` and
// `agent_text_authority` carry the correlation rule verbatim.

use serde::Serialize;
use sqlx::PgPool;

use crate::agent_auth::epoch_ms_to_iso;
use crate::agent_writes::{WriteAuthor, guard_agent_fields};
use crate::approvals::audience_for;
use crate::notify::{NotificationInput, NotifyDeps, add_notification};
use crate::runs::define::Authority;

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
    // TS's `[^a-z0-9]+` → '-' collapses runs; the map above emits runs of '-'
    // which this squeeze then collapses to match.
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

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
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
        // No row at all (no memo has ever been written) is `rows[0]?.memo`
        // → null in TS — a normal read, handled below, not an error path.
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
        // Missing, malformed, or stale — TS's NaN comparison ladder all lands
        // here too.
        _ => Authority::Admin { on_board: None },
    }
}

/// What `report_gap` hands back to the route: the row's id, its frequency,
/// and whether THIS call was the shape's first sighting.
pub struct ReportedGap {
    pub id: String,
    pub seen_count: i32,
    pub first: bool,
}

/// `reportGap` — the write. The agent's `missing`/`needs` go through the one
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
    let sig = signature_of(board_id.as_deref(), input.kind);
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

/// TS `.slice(0, n)` — by UTF-16 code units. Byte-identical for the ASCII
/// these fields are validated as prose in, and char-boundary-safe here where
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
    }
}

/// Tell the admins a NEW kind of gap exists — and tell only the ones who can
/// see the work it quotes what it actually SAYS. WHO, and how much, is
/// `audience_for`; this function does not re-decide the authority, it is
/// handed one and asks the resolver, which is the whole point.
///
/// Never throws: an agent reported a gap honestly and the row is already
/// written. Losing the notification is a bad day; losing the row — or failing
/// the agent's POST — would teach the fleet that honesty costs it something.
async fn announce_gap(deps: &NotifyDeps, input: announce_gap::GapAnnounce<'_>) {
    use announce_gap::GapAnnounce;
    let GapAnnounce {
        agent_model,
        kind,
        missing,
        needs,
        authority,
    } = input;
    let who = audience_for(&deps.pg, authority).await;
    let placed = matches!(authority, Authority::Admin { on_board: Some(_) });
    let ratify = "\n\nRatify it in the Studio's Suggested queue, or dismiss it there.";
    for user_id in &who.content {
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
    for user_id in &who.fact {
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
    use crate::runs::define::Authority;
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

/// `listGaps` — the Studio's Suggested queue, ranked by frequency then
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
    fn slug_matches_ts() {
        // TS: trim, lower, [^a-z0-9]+ → '-', edge dashes off, 60 max, else
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
}
