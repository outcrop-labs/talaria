// The Titler platform agent — names things as they take shape. Chats and
// plans get retitled once their first exchange lands (only while the title is
// still the mechanical first-message truncation — a name a user typed or
// another flow chose is never clobbered); research runs get a title from
// their question the moment they start.
//
// Port of titler.ts, REDUCED to the half this batch needs: `generateTitle`.
// The retitle walk and the sweep belong to the chat family's plane (they
// touch conversations' message tails), and they land with it — `maybeRetitle
// Conversation`, `sweepTitles` and `maybeSweepTitles` are not forgotten, they
// are the next batch's first slice.
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
