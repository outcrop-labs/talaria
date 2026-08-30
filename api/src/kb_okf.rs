// Per-document OKF — each promoted doc carries a hidden agent-facing summary
// in the Open Knowledge Format (YAML frontmatter + markdown concept body, per
// the OKF spec): type/title/description/resource/tags, a `generated` trust
// stamp, and lifecycle status. The LIBRARIAN platform agent writes it when a
// doc is promoted, refreshes it when promoted content changes, and it clears
// on demotion. Agents read it through the doc API; humans peek via the OKF
// chip in the doc header.
//
// Port of ui/src/server/kb-okf.ts. The prompt, the fallback chain, the tag
// parse and the failure policy all live in the harness definition
// (harness/defs/librarian.rs); what survives here is the only thing that was
// ever this file's business: turning a summary into an OKF concept, storing
// it, and the 15s debounce that collapses bursts.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use serde_json::json;

use crate::agent_auth::epoch_ms_to_iso;
use crate::harness::defs::librarian::librarian_harness;
use crate::harness::run::{RunContext, run_harness};
use crate::kb::get_doc;
use crate::state::AppState;

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// JSON.stringify for the two frontmatter values that may carry quotes or
/// newlines: a JSON string is a valid YAML scalar, and the escaping is the
/// same bytes TS emitted.
fn yaml_string(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

/// (Re)generate one doc's OKF. No-op for unpromoted docs (demote clears).
pub async fn generate_doc_okf(state: &AppState, doc_id: &str) -> Result<(), sqlx::Error> {
    let Some(doc) = get_doc(&state.pg, doc_id).await? else {
        return Ok(());
    };
    if !doc.official {
        sqlx::query("update kb_docs set okf = null where id = $1::uuid")
            .bind(doc_id)
            .execute(&state.pg)
            .await?;
        return Ok(());
    }
    if doc.body.trim().is_empty() {
        return Ok(());
    }

    let run = run_harness(
        state,
        &librarian_harness(),
        &json!({ "title": doc.title, "body": doc.body }),
        RunContext {
            caller: "platform:librarian".into(),
            ..RunContext::default()
        },
    )
    .await;
    // FIRE AND FORGET, exactly as TS: a model hiccup, an install whose gateway
    // serves nothing this harness can reach, and a reply with no usable body
    // all arrive here — and the doc keeps the OKF it already had rather than
    // losing it. `onFailure: 'null'` on the definition is the other half.
    let Ok(run) = run else { return Ok(()) };
    let Some(value) = run.value else {
        return Ok(());
    };
    let body = value.get("body").and_then(|v| v.as_str()).unwrap_or("");
    let tags: Vec<String> = value
        .get("tags")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|t| t.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    // Non-null whenever a value came back (nothing here declares a fallback),
    // but the trust stamp is persisted and read by agents, so it never guesses.
    let model = run.model.as_deref().unwrap_or("unknown");
    let description = body
        .split('\n')
        .find(|l| !l.trim().is_empty())
        .map(|l| l.chars().take(160).collect::<String>())
        .unwrap_or_else(|| doc.title.clone());

    // OKF concept per the spec: frontmatter (type/title/description/resource/
    // tags + generated trust stamp + lifecycle) over a markdown body. updated_at
    // is already the toISOString rendering, so re-rendering it through a Date
    // (TS's `new Date(...).toISOString()`) is the identity on our own output.
    let now = epoch_ms_to_iso(now_ms());
    let mut lines: Vec<String> = vec![
        "---".into(),
        "type: Knowledge Document".into(),
        format!("title: {}", yaml_string(&doc.title)),
        format!("description: {}", yaml_string(&description)),
        format!("resource: /knowledge?d={}", doc.id),
    ];
    if !tags.is_empty() {
        lines.push(format!("tags: [{}]", tags.join(", ")));
    }
    lines.push(format!(
        "generated: {{ by: talaria/librarian:{model}, at: {now} }}"
    ));
    lines.push("status: stable".into());
    lines.push("sources:".into());
    lines.push(format!("  - resource: /knowledge?d={}", doc.id));
    lines.push(format!("    last_modified: {}", doc.updated_at));
    lines.push("---".into());
    lines.push(String::new());
    lines.push(body.to_string());
    let okf = lines.join("\n");

    sqlx::query("update kb_docs set okf = $2 where id = $1::uuid")
        .bind(doc_id)
        .bind(&okf)
        .execute(&state.pg)
        .await?;
    Ok(())
}

// ── Debounced autonomy: promotions/saves queue their doc; bursts collapse. ──
// The generation counter is the one refinement over TS's Map-of-handles: a JS
// timer's callback cannot interleave with queueDocOkf, but a Rust task can
// wake between another task's lock and its remove — so a callback only clears
// the slot when the slot is still its own.
type Debounces = Mutex<HashMap<String, (u64, tokio::task::AbortHandle)>>;
static TIMERS: LazyLock<Debounces> = LazyLock::new(|| Mutex::new(HashMap::new()));

const DEBOUNCE: Duration = Duration::from_secs(15);

pub fn queue_doc_okf(state: AppState, doc_id: String) {
    let mut timers = TIMERS.lock().expect("okf debounce lock");
    let generation = match timers.get(&doc_id) {
        Some((g, handle)) => {
            handle.abort();
            g + 1
        }
        None => 0,
    };
    let queued = doc_id.clone();
    let handle = tokio::spawn(async move {
        tokio::time::sleep(DEBOUNCE).await;
        {
            let mut timers = TIMERS.lock().expect("okf debounce lock");
            if timers.get(&queued).is_some_and(|(g, _)| *g == generation) {
                timers.remove(&queued);
            }
        }
        let _ = generate_doc_okf(&state, &queued).await;
    });
    timers.insert(doc_id, (generation, handle.abort_handle()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yaml_string_matches_json_stringify() {
        // The frontmatter scalars are JSON strings: same quotes, same escapes.
        assert_eq!(yaml_string("plain"), "\"plain\"");
        assert_eq!(yaml_string("a\"b\\c"), "\"a\\\"b\\\\c\"");
        assert_eq!(yaml_string("line\nbreak"), "\"line\\nbreak\"");
    }

    #[test]
    fn debounce_is_fifteen_seconds() {
        assert_eq!(DEBOUNCE.as_millis(), 15_000);
    }
}
