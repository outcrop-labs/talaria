// The Titler platform agent — names things as they take shape. Chats and
// plans get retitled once their first exchange lands (only while the title is
// still the mechanical first-message truncation — a name a user typed or
// another flow chose is never clobbered); research runs get a title from
// their question the moment they start.
//
// The sweep (retroactive + ongoing naming, hourly, kicked from the comms
// read) crossed with the channels family — its kick lives on /api/channels.
// The interactive `maybeRetitleConversation` walk belongs to the chat
// family's plane and lands with it.
//
// Everything here is fire-and-forget by contract: naming must never block or
// fail the work it names, so `generate_title` answers None on every failure
// and callers KEEP THE CURRENT TITLE on None. Nothing here may ever start
// returning a placeholder on failure.

use crate::harness::defs::titler::{TitleKind, TitlerInput, titler_harness};
use crate::harness::run::{RunContext, run_harness};
use crate::state::AppState;
use serde_json::json;

/// One short completion → a clean title, or None when nothing routes / the
/// model rambles. Callers keep their existing title on None.
///
/// Everything this used to do by hand — resolving the model down four
/// fallback steps, catching the upstream hiccup, taking the first non-empty
/// line and stripping the quotes off it — is declared in the titler harness
/// and done by the runner. None still means exactly what it meant.
pub async fn generate_title(state: &AppState, kind: TitleKind, text: &str) -> Option<String> {
    // Ahead of the harness on purpose: an empty transcript has no title in
    // it, and this early-out is what keeps a chat with no user message from
    // spending a model call and a harness_runs row to discover that.
    if text.trim().is_empty() {
        return None;
    }
    let input = json!(TitlerInput {
        kind,
        text: text.to_string(),
    });
    let run = run_harness(
        state,
        &titler_harness(),
        &input,
        RunContext {
            caller: "platform:titler".into(),
            ..RunContext::default()
        },
    )
    .await
    // A harness that cannot run is one of the cases "None" was kept for —
    // the caller keeps the title it has.
    .ok()?;
    run.value.and_then(|v| v.as_str().map(str::to_string))
}

/// The mechanical default chat.ts stamps at creation — a title still equal to
/// it means nobody has named the conversation on purpose.
fn mechanical_from(s: &str) -> String {
    // JS: s.replace(/\s+/g, ' ').trim().slice(0, 80) — collapse runs of any
    // whitespace to one space, trim, first 80 chars.
    let collapsed: Vec<&str> = s.split_whitespace().collect();
    collapsed.join(" ").chars().take(80).collect()
}

// ── The sweep: retroactive + ongoing naming ─────────────────────────────────
// Anything that predates the Titler (or whose naming call failed) gets picked
// up here: research runs with no title, and live conversations still wearing
// the mechanical truncation. Batched per pass so one sweep never burns much;
// failures simply wait for the next pass.

/// How many model calls one sweep pass may spend (SWEEP_LLM_BUDGET).
const SWEEP_LLM_BUDGET: u32 = 12;

pub async fn sweep_titles(state: &AppState) -> u32 {
    let mut spent: u32 = 0;

    let runs: Vec<(String, String)> = sqlx::query_as(
        "select id::text, question from research_runs where title is null \
         order by created_at desc limit $1",
    )
    .bind(SWEEP_LLM_BUDGET as i64)
    .fetch_all(&state.pg)
    .await
    .unwrap_or_default();
    for (id, question) in &runs {
        if spent >= SWEEP_LLM_BUDGET {
            break;
        }
        spent += 1;
        let Some(t) = generate_title(state, TitleKind::Research, question).await else {
            // Model down/rate-limited — stop burning the batch; the next pass
            // retries.
            return spent;
        };
        let _ = sqlx::query("update research_runs set title = $1 where id = $2::uuid")
            .bind(t)
            .bind(id)
            .execute(&state.pg)
            .await;
    }

    // Live conversations whose title still equals the truncated first user
    // message (or the bare 'chat' fallback) — i.e., nobody named them yet.
    let convs: Vec<(String, Option<String>, String, Option<String>)> = sqlx::query_as(
        "select c.id::text, c.title, c.kind, \
                (select m.content from messages m \
                  where m.conversation_id = c.id and m.role = 'user' and m.content <> '' \
                  order by m.seq asc limit 1) as first \
         from conversations c \
         where c.archived = false \
           and exists (select 1 from messages m2 \
             where m2.conversation_id = c.id and m2.role = 'assistant' and m2.content <> '') \
         order by c.updated_at desc limit 100",
    )
    .fetch_all(&state.pg)
    .await
    .unwrap_or_default();
    for (id, title, kind, first) in &convs {
        if spent >= SWEEP_LLM_BUDGET {
            break;
        }
        let mechanical = match title.as_deref() {
            None => true,
            Some("chat") => true,
            Some(t) => first.as_deref().is_some_and(|f| t == mechanical_from(f)),
        };
        if !mechanical {
            continue;
        }
        spent += 1;
        // Sweep-side retitle: same gate, but without the first-messages-only
        // limit — a pre-Titler conversation can be long and still
        // mechanically titled. `title` is the snapshot the gate approved and
        // the write asserts it is still there: the sweep holds up to a dozen
        // sequential model calls against a hundred rows, and every one of
        // them is renameable from the same screen that kicked the sweep.
        if !retitle_conversation_any_length(state, id, kind, title.as_deref()).await {
            return spent; // ditto — stop burning the batch
        }
    }
    spent
}

/// The sweep's per-conversation naming call; false = keep stopping the batch
/// (the model is down), true = named or nothing to do.
async fn retitle_conversation_any_length(
    state: &AppState,
    conversation_id: &str,
    kind: &str,
    expect: Option<&str>,
) -> bool {
    let msgs: Vec<(String, String)> = sqlx::query_as(
        "select role, content from messages \
         where conversation_id = $1::uuid and content <> '' order by seq asc limit 6",
    )
    .bind(conversation_id)
    .fetch_all(&state.pg)
    .await
    .unwrap_or_default();
    let transcript = msgs
        .iter()
        .map(|(role, content)| format!("{role}: {}", clip_chars(content, 1200)))
        .collect::<Vec<_>>()
        .join("\n\n");
    let title = generate_title(
        state,
        if kind == "plan" {
            TitleKind::Plan
        } else {
            TitleKind::Chat
        },
        &transcript,
    )
    .await;
    match title {
        Some(t) => {
            // `is not distinct from` because the mechanical state includes a
            // NULL title — a rename landing between gate and write is never
            // silently overwritten.
            let _ = sqlx::query(
                "update conversations set title = $1 \
                 where id = $2::uuid and title is not distinct from $3",
            )
            .bind(&t)
            .bind(conversation_id)
            .bind(expect)
            .execute(&state.pg)
            .await;
            true
        }
        None => false,
    }
}

/// JS `s.slice(0, n)` on a UTF-16 string — a char-boundary take is the same
/// for every string without surrogates, and no boundary splits a char here.
fn clip_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// Opportunistic scheduling: any comms read may kick a sweep, hourly,
/// detached. The throttle is process-local, exactly as TS's module boolean is.
static LAST_TITLE_SWEEP_MS: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);

pub fn maybe_sweep_titles(state: AppState) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let last = LAST_TITLE_SWEEP_MS.load(std::sync::atomic::Ordering::Relaxed);
    if now - last < 60 * 60_000 {
        return;
    }
    LAST_TITLE_SWEEP_MS.store(now, std::sync::atomic::Ordering::Relaxed);
    tokio::spawn(async move {
        let _ = sweep_titles(&state).await;
    });
}
