// Human-readable model identity, populated automatically — port of
// model-info.ts (both halves: the catalog read and the org-voice rewrite
// pass). OpenRouter's public catalog (no key) carries a pretty name +
// description for essentially every major model; we match registered ids
// against it the same way the price oracle does (full id for slashed ids,
// unambiguous suffix for bare ones) and serve a display label + a one-line
// "what it's good at" blurb. Unknown models (e.g. self-hosted) simply have
// no blurb — nothing is invented.

use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone)]
pub struct ModelInfo {
    /// Pretty display name, e.g. "Qwen: Qwen3 14B".
    pub label: String,
    /// First sentence of the catalog description, clamped.
    pub blurb: String,
}

/// `normalize`: lowercase, dots to dashes.
fn normalize(s: &str) -> String {
    s.to_lowercase().replace('.', "-")
}

/// JS `\s` — what the collapse regex and the sentence split both mean by
/// whitespace. Rust's `char::is_whitespace` differs at the edges (﻿ is
/// JS-\s and not Unicode White_Space; \u{85} is the reverse), and the port
/// keeps the JS set so a description's edge characters collapse the same way.
fn is_js_ws(c: char) -> bool {
    matches!(
        c,
        ' ' | '\t' | '\n' | '\u{b}' | '\u{c}' | '\r' | '\u{a0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200a}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202f}'
                | '\u{205f}'
                | '\u{3000}'
                | '\u{feff}'
    )
}

fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

/// One line out of a marketing paragraph: first sentence, clamped. The split
/// keeps the period (`split(/(?<=\.)\s/)` — break at whitespace PRECEDED by a
/// period), and the clamp is UTF-16 units because `String.length` is.
fn to_blurb(description: &str) -> String {
    // \s+ → ' ', then trim.
    let mut collapsed = String::with_capacity(description.len());
    let mut in_run = false;
    for c in description.chars() {
        if is_js_ws(c) {
            in_run = true;
        } else {
            if in_run {
                collapsed.push(' ');
                in_run = false;
            }
            collapsed.push(c);
        }
    }
    let trimmed = collapsed.trim_matches(' ');
    // First sentence: up to the first whitespace that follows a period.
    let first = {
        let mut prev: Option<char> = None;
        let mut cut = None;
        for (i, c) in trimmed.char_indices() {
            if is_js_ws(c) && prev == Some('.') {
                cut = Some(i);
                break;
            }
            prev = Some(c);
        }
        match cut {
            Some(i) => &trimmed[..i],
            None => trimmed,
        }
    };
    if utf16_len(first) > 160 {
        // JS slice(0, 157) cuts UTF-16 units and can split a surrogate pair;
        // Rust strings cannot hold half a pair, so cut at the last whole char
        // that fits. (Only reachable with an astral character straddling unit
        // 157 of a 160+-unit sentence.)
        let mut units = 0usize;
        let mut end = 0usize;
        for (i, c) in first.char_indices() {
            let w = c.len_utf16();
            if units + w > 157 {
                break;
            }
            units += w;
            end = i + c.len_utf8();
        }
        format!("{}…", &first[..end])
    } else {
        first.to_string()
    }
}

struct Catalog {
    by_id: HashMap<String, ModelInfo>,
    by_suffix: HashMap<String, Vec<ModelInfo>>,
}

/// Build the lookup tables from OpenRouter's `data` rows (pure, for tests).
/// Later duplicates overwrite earlier in `by_id` (Map.set), append in
/// `by_suffix` — the ambiguity signal.
fn build_catalog(data: &[serde_json::Value]) -> Catalog {
    let mut by_id = HashMap::new();
    let mut by_suffix: HashMap<String, Vec<ModelInfo>> = HashMap::new();
    for m in data {
        let (Some(id), Some(name)) = (
            m.get("id").and_then(|v| v.as_str()),
            m.get("name").and_then(|v| v.as_str()),
        ) else {
            continue;
        };
        let info = ModelInfo {
            label: name.to_string(),
            blurb: to_blurb(m.get("description").and_then(|d| d.as_str()).unwrap_or("")),
        };
        by_id.insert(normalize(id), info.clone());
        let rest: Vec<&str> = id.split('/').skip(1).collect();
        let suffix = normalize(&rest.join("/"));
        if !suffix.is_empty() {
            by_suffix.entry(suffix).or_default().push(info);
        }
    }
    Catalog { by_id, by_suffix }
}

/// Raw public-catalog info for one id, or None when nothing matches cleanly.
fn info_in<'a>(cat: &'a Catalog, model_id: &str) -> Option<&'a ModelInfo> {
    if model_id.contains('/') {
        return cat.by_id.get(&normalize(model_id));
    }
    let candidates = cat.by_suffix.get(&normalize(model_id))?;
    // Ambiguous bare id → no guess.
    (candidates.len() == 1).then(|| &candidates[0])
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

struct Cache {
    at: u64,
    catalog: Catalog,
}

fn cache_cell() -> &'static Mutex<Option<Cache>> {
    static CELL: OnceLock<Mutex<Option<Cache>>> = OnceLock::new();
    CELL.get_or_init(|| Mutex::new(None))
}

/// The public catalog, 6h-fresh, STALE ON FAILURE: an unreachable OpenRouter
/// must not blank every label in the product, so a failed fetch serves the
/// last good answer (or None before the first success).
async fn catalog() -> Option<Catalog> {
    const TTL_MS: u64 = 6 * 60 * 60_000;
    if let Ok(cell) = cache_cell().lock()
        && let Some(c) = cell.as_ref()
        && now_ms() - c.at < TTL_MS
    {
        return Some(clone_of(c).catalog);
    }
    let fetched = async {
        let r = crate::gateway::provider::http()
            .get("https://openrouter.ai/api/v1/models")
            .timeout(Duration::from_secs(10))
            .send()
            .await
            .ok()?;
        if !r.status().is_success() {
            return None;
        }
        let j: serde_json::Value = r.json().await.ok()?;
        let data = j.get("data")?.as_array()?;
        Some(build_catalog(data))
    }
    .await;
    match fetched {
        Some(c) => {
            if let Ok(mut cell) = cache_cell().lock() {
                *cell = Some(Cache {
                    at: now_ms(),
                    catalog: Catalog {
                        by_id: c.by_id.clone(),
                        by_suffix: c.by_suffix.clone(),
                    },
                });
            }
            Some(c)
        }
        None => cache_cell()
            .lock()
            .ok()
            .and_then(|cell| cell.as_ref().map(|c| clone_of(c).catalog)),
    }
}

fn clone_of(c: &Cache) -> Cache {
    Cache {
        at: c.at,
        catalog: Catalog {
            by_id: c.catalog.by_id.clone(),
            by_suffix: c.catalog.by_suffix.clone(),
        },
    }
}

/// Info for one registered model id — port of modelInfo: catalog label + the
/// org-voice blurb when it's been written, else the raw catalog blurb. None
/// when nothing matches. The blurb override read PROPAGATES its error: TS's
/// `await sql` rejects, and the caller (the /api/models Promise.all) turns
/// that into the same 500.
pub async fn model_info(pg: &PgPool, model_id: &str) -> Result<Option<ModelInfo>, sqlx::Error> {
    let Some(cat) = catalog().await else {
        return Ok(None);
    };
    let Some(info) = info_in(&cat, model_id) else {
        return Ok(None);
    };
    let rewritten: Option<String> =
        sqlx::query_scalar("select blurb from model_blurbs where model_id = $1")
            .bind(model_id)
            .fetch_optional(pg)
            .await?;
    Ok(match rewritten {
        Some(blurb) => Some(ModelInfo {
            label: info.label.clone(),
            blurb,
        }),
        None => Some(info.clone()),
    })
}

/// Drop the cached catalog — for tests.
pub fn clear_model_info_cache() {
    if let Ok(mut cell) = cache_cell().lock() {
        *cell = None;
    }
}

// ── The org-voice rewrite pass ───────────────────────────────────────────────
//
// One batched completion through the Catalog writer turns the vendor's
// marketing line into the org's own voice, cached in model_blurbs. The pass
// is OPPORTUNISTIC: any catalog read may kick it, throttled, detached, never
// blocking the request — the TS shape exactly.

const REWRITE_BATCH: usize = 10;
const REWRITE_THROTTLE_MS: u64 = 10 * 60_000;

/// The TS `blurb.trim().slice(0, 200)` — UTF-16 units, because `slice` is.
/// Cutting a string Rust can't hold half of means stopping at the last whole
/// character that fits (only reachable when an astral character straddles
/// unit 200).
fn clamp_200(s: &str) -> String {
    let mut units = 0usize;
    let mut end = 0usize;
    for (i, c) in s.char_indices() {
        let w = c.len_utf16();
        if units + w > 200 {
            break;
        }
        units += w;
        end = i + c.len_utf8();
    }
    s[..end].to_string()
}

/// The pass's whole write policy, on its own so it can be pinned without a
/// model or a database: string values only, trimmed-nonempty only, clamped to
/// 200 UTF-16 units, one row per pending id in batch order. A wrong type or a
/// blank line is skipped, not salvaged — the harness's contract already
/// rejected the shapes worth repairing, and a blank blurb is worse than the
/// catalog line it would replace. What is left here is tolerance: one bad
/// line must not cost the other nine.
fn written_rows(
    reply: &serde_json::Map<String, serde_json::Value>,
    pending: &[crate::harness::defs::blurb_writer::BlurbCandidate],
) -> Vec<(String, String)> {
    pending
        .iter()
        .filter_map(|p| {
            let blurb = reply.get(&p.id)?.as_str()?.trim();
            if blurb.is_empty() {
                return None;
            }
            Some((p.id.clone(), clamp_200(blurb)))
        })
        .collect()
}

/// Rewrite catalog blurbs for registered models that don't have one yet —
/// port of rewritePendingBlurbs. Returns how many were written. A PARTIAL
/// batch is still a good pass: the models the reply skipped stay pending and
/// come back around on the next kick, which is why the row selection walks
/// `pending` rather than iterating whatever the model returned. The harness
/// run's own failure is a no-write pass (warned, not propagated): TS's runner
/// answers `{ value: null }` rather than throwing, and the kick's `.catch`
/// swallows even a genuine throw — the worst outcome TS allowed was "nothing
/// written this pass", and the port keeps exactly that ceiling.
pub async fn rewrite_pending_blurbs(
    state: &crate::state::AppState,
    batch: usize,
) -> Result<usize, String> {
    use crate::harness::defs::blurb_writer::{BlurbBatch, BlurbCandidate, blurb_writer_harness};
    use crate::harness::run::{RunContext, run_harness};
    use crate::model_access::gateway_models;

    let all = gateway_models(&state.pg)
        .await
        .map_err(|e| format!("gateway catalog read failed: {e}"))?;
    // Bare ids only — a qualified "<endpoint>/<model>" pin is a routing
    // decision about one endpoint, not an identity the org voice rewrites.
    let bare: Vec<String> = all
        .iter()
        .filter(|m| !m.qualified)
        .map(|m| m.id.clone())
        .collect();
    if bare.is_empty() {
        return Ok(0);
    }
    let done: std::collections::HashSet<String> = sqlx::query_scalar::<_, String>(
        "select model_id from model_blurbs where model_id = any($1)",
    )
    .bind(&bare)
    .fetch_all(&state.pg)
    .await
    .map_err(|e| format!("model_blurbs read failed: {e}"))?
    .into_iter()
    .collect();

    // TS awaits catalogInfo per id; the catalog fetch is cached, so the Rust
    // pass resolves every id against the one snapshot the cache hands back.
    // No catalog → no pending candidate resolves → nothing to do.
    let cat = catalog().await;
    let mut pending: Vec<BlurbCandidate> = Vec::new();
    for id in &bare {
        if done.contains(id) || pending.len() >= batch {
            continue;
        }
        if let Some(info) = cat.as_ref().and_then(|c| info_in(c, id)) {
            pending.push(BlurbCandidate {
                id: id.clone(),
                name: info.label.clone(),
                description: info.blurb.clone(),
            });
        }
    }
    if pending.is_empty() {
        return Ok(0);
    }

    let org_name =
        crate::gateway::settings::get_setting(&state.pg, "org_name", serde_json::json!(""))
            .await
            .as_str()
            .unwrap_or("")
            .to_string();
    let input = serde_json::to_value(BlurbBatch {
        org_name,
        models: pending.clone(),
    })
    .map_err(|e| format!("blurb batch encode failed: {e}"))?;
    let run = run_harness(
        state,
        &blurb_writer_harness(),
        &input,
        RunContext {
            caller: "platform:blurb-writer".into(),
            user_id: None,
            model: None,
            step: None,
            tier: None,
            effort: None,
            ledger: None,
            deps: None,
        },
    )
    .await;
    let Some(reply) = run
        .map_err(|e| {
            tracing::warn!("[model-blurbs] the Catalog writer run failed: {e}");
            e
        })
        .ok()
        .and_then(|r| r.value)
        .and_then(|v| v.as_object().cloned())
    else {
        return Ok(0);
    };

    let rows = written_rows(&reply, &pending);
    for (id, blurb) in &rows {
        // AssertSqlSafe: static statement, binds only.
        sqlx::query(
            "insert into model_blurbs (model_id, blurb) values ($1, $2) \
             on conflict (model_id) do update set blurb = excluded.blurb",
        )
        .bind(id)
        .bind(blurb)
        .execute(&state.pg)
        .await
        .map_err(|e| format!("model_blurbs write failed for {id}: {e}"))?;
    }
    Ok(rows.len())
}

/// The throttled kick — port of maybeRewriteBlurbs. Any catalog read may fire
/// it; ten minutes is the floor; the pass runs detached and never blocks the
/// request it was kicked from. A failed pass only logs: the next kick retries
/// in ten minutes, and nothing about a missing blurb is worth a 500.
static LAST_KICK: AtomicU64 = AtomicU64::new(0);

pub fn maybe_rewrite_blurbs(state: &crate::state::AppState) {
    let now = now_ms();
    if now.saturating_sub(LAST_KICK.load(Ordering::Relaxed)) < REWRITE_THROTTLE_MS {
        return;
    }
    LAST_KICK.store(now, Ordering::Relaxed);
    let state = state.clone();
    tokio::spawn(async move {
        if let Err(e) = rewrite_pending_blurbs(&state, REWRITE_BATCH).await {
            tracing::warn!("[model-blurbs] rewrite pass failed: {e}");
        }
    });
}

// ── The registered job ───────────────────────────────────────────────────────
//
// The scheduler owns the cadence; the module owns the work. The job runs the
// same pass the kick runs (same batch, same floor), on the kick's own
// throttle as its interval — the number was always the intended cadence; the
// kick was just the only trigger that existed. NOT in REQUIRED_JOBS, same as
// TS's optional jobs: its failure mode is a stale blurb next to a catalog
// that self-heals on the next pass, not work that silently never happens.

/// The job the scheduler runs. NOT `per_instance`: it writes `model_blurbs`
/// rows every instance can read, and two instances passing at once would
/// spend two model calls on one batch of pending ids.
pub fn blurb_rewrite_job_spec(deps: std::sync::Arc<BlurbDeps>) -> crate::scheduler::JobSpec {
    use crate::scheduler::{JobName, JobSpec};
    JobSpec {
        name: JobName::BlurbRewrite,
        every_ms: REWRITE_THROTTLE_MS,
        // Early: the whole point of the kick was that blurbs are ready
        // before anyone opens the model picker, and the job can be earlier
        // still — a minute lets the instance come up first.
        first_run_delay_ms: Some(60_000),
        // One harness call over a batch of ten candidates.
        max_run_ms: Some(2 * 60_000),
        per_instance: false,
        run: std::sync::Arc::new(move || {
            let deps = deps.clone();
            Box::pin(async move {
                let written = rewrite_pending_blurbs(&deps.state, REWRITE_BATCH).await?;
                // Zero is a quiet pass — every registered model already has
                // its org-voice blurb, which is the steady state.
                Ok(if written == 0 {
                    None
                } else {
                    Some(format!("{written} model blurb(s) written in the org voice"))
                })
            })
        }),
    }
}

/// Declare the sweep to the scheduler — the function the flip calls from
/// boot. When the job arms, the route's `maybe_rewrite_blurbs` kick retires
/// (the flip slice removes it): both run the same pass, and a kick AND a job
/// on the same throttle is two model calls for one batch of pending ids.
pub fn register_blurb_rewrite_job(deps: std::sync::Arc<BlurbDeps>) {
    crate::scheduler::register_job(blurb_rewrite_job_spec(deps));
}

/// The job's runtime values: the state the pass reads the catalog and writes
/// the rows through.
pub struct BlurbDeps {
    pub state: crate::state::AppState,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn blurb_is_the_first_sentence_clamped() {
        // First sentence, period kept.
        assert_eq!(
            to_blurb("  Qwen3 is a   general model.\nIt does more. Even more.  "),
            "Qwen3 is a general model."
        );
        // No sentence break: whole string, trimmed, collapsed.
        assert_eq!(to_blurb(" one two\n\tthree "), "one two three");
        assert_eq!(to_blurb(""), "");
        // Clamp at 160 UTF-16 units → 157 + ellipsis.
        let long = "a".repeat(161);
        assert_eq!(to_blurb(&long), format!("{}…", "a".repeat(157)));
        let exact = "a".repeat(160);
        assert_eq!(to_blurb(&exact), exact);
        // The clamp counts UTF-16 units, not bytes. ONE RECORDED DIVERGENCE:
        // TS `slice(0, 157)` will cut mid-surrogate-pair (158 units, one of
        // them a lone surrogate); Rust cuts at the char boundary — 78 emoji +
        // '…' = 157 units. Lone surrogates are not representable in UTF-8, and
        // the case needs a >160-unit first sentence of astral chars. Cosmetic.
        let emoji = "😀".repeat(81); // 162 units, 324 bytes
        let got = to_blurb(&emoji);
        assert_eq!(utf16_len(&got), 157); // 156 units of emoji (78 chars) + '…'
    }

    #[test]
    fn slashed_ids_match_exact_bare_ids_match_unique_suffixes_only() {
        let cat = build_catalog(&[
            json!({"id": "deepseek/deepseek-v4", "name": "DeepSeek V4", "description": "Fast. Cheap too."}),
            json!({"id": "other/deepseek-v4", "name": "Other V4", "description": "Also fast."}),
            json!({"id": "qwen/qwen3-14b", "name": "Qwen3 14B", "description": "Good at code. Cheap."}),
            json!({"id": "nodot", "name": "No Dot", "description": "One sentence"}),
            json!({"name": "no id is skipped"}),
            json!({"id": "no-name-is-skipped"}),
        ]);
        // Slashed: exact normalized id (dots ↔ dashes).
        let info = info_in(&cat, "deepseek/deepseek-v4").unwrap();
        assert_eq!(info.label, "DeepSeek V4");
        assert_eq!(info.blurb, "Fast.");
        // Bare but served by two vendors: ambiguous → no guess.
        assert!(info_in(&cat, "deepseek-v4").is_none());
        // Bare and unique.
        let info = info_in(&cat, "qwen3-14b").unwrap();
        assert_eq!(info.label, "Qwen3 14B");
        assert_eq!(info.blurb, "Good at code.");
        // A BARE catalog id is unreachable from a bare lookup — the suffix
        // index only ever registers ids with a slash, so "nodot" is indexed
        // nowhere. TS bug-for-bug: its bySuffix skips slice(1) == '' too.
        assert!(info_in(&cat, "nodot").is_none());
        assert!(info_in(&cat, "never-heard-of").is_none());
        // Normalize: dots become dashes on lookup too.
        assert!(info_in(&cat, "qwen/qwen3.14b").is_some());
    }

    // ── the rewrite pass's write policy ─────────────────────────────────────

    use crate::harness::defs::blurb_writer::BlurbCandidate;

    fn cand(id: &str) -> BlurbCandidate {
        BlurbCandidate {
            id: id.into(),
            name: format!("{id} pretty"),
            description: format!("{id} catalog line"),
        }
    }

    fn reply(v: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
        v.as_object().unwrap().clone()
    }

    #[test]
    fn written_rows_keeps_only_trimmed_nonempty_strings_in_pending_order() {
        let pending = vec![cand("a"), cand("b"), cand("c"), cand("d")];
        let rows = written_rows(
            &reply(json!({
                // good
                "a": "Runs cheap and quiet",
                // whitespace-only → a blank blurb is worse than the catalog line
                "b": "   ",
                // wrong type → skipped, not salvaged (the contract repaired
                // what was repairable; this is the tolerance that is left)
                "c": { "line": "nested" },
                // never answered — stays pending, comes back next pass
                "e": "an id the batch never asked for",
            })),
            &pending,
        );
        assert_eq!(
            rows,
            vec![("a".to_string(), "Runs cheap and quiet".to_string())]
        );
        // d has no entry at all → absent from the rows, like c and b.
    }

    #[test]
    fn written_rows_trims_and_clamps_to_200_utf16_units() {
        let pending = vec![cand("a"), cand("b")];
        let rows = written_rows(
            &reply(json!({
                // trim() happens before the clamp
                "a": "  padded  ",
                "b": "x".repeat(300),
            })),
            &pending,
        );
        assert_eq!(rows[0].1, "padded");
        assert_eq!(utf16_len(&rows[1].1), 200);
        assert_eq!(rows[1].1.len(), 200); // all-ascii: units == bytes
    }

    #[test]
    fn the_clamp_cuts_at_a_whole_astral_character() {
        // 😀 is two UTF-16 units. 199 ascii + 😀 = 201 units; the clamp stops
        // BEFORE the pair rather than splitting it, so 199.
        let s = format!("{}{}", "x".repeat(199), "😀");
        let clamped = clamp_200(&s);
        assert_eq!(utf16_len(&clamped), 199);
        assert!(clamped.ends_with('x'));
        // Exactly 200 units passes through untouched.
        let fits = format!("{}😀", "x".repeat(198));
        assert_eq!(clamp_200(&fits), fits);
    }

    // ── the registered job's declared timings ────────────────────────────────

    #[tokio::test]
    async fn the_job_spec_carries_the_declared_timings() {
        // Real but lazy state — the pool dials nothing, and the spec test
        // never runs the job (same posture as the work-session tests).
        use crate::state::AppState;
        let url = "postgres://blurb-spec-test@localhost:5432/blurb-spec-test";
        let pg = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy(url)
            .expect("a lazy pool connects to nothing");
        let cfg = crate::config::Config::from_parts(
            url.into(),
            "redis://blurb-spec-test@localhost:6379".into(),
            "test-root".into(),
            String::new(),
            String::new(),
            "0".into(),
        )
        .expect("the test config is valid on its face");
        let deps = std::sync::Arc::new(BlurbDeps {
            state: AppState::new(pg, std::sync::Arc::new(cfg)),
        });
        let spec = blurb_rewrite_job_spec(deps);
        assert_eq!(spec.name.as_str(), "blurb-rewrite");
        // The kick's throttle was always the intended cadence; the job just
        // owns it instead of the request path.
        assert_eq!(spec.every_ms, 10 * 60_000);
        assert_eq!(spec.first_run_delay_ms, Some(60_000));
        assert_eq!(spec.max_run_ms, Some(2 * 60_000));
        assert!(
            !spec.per_instance,
            "model_blurbs is a fleet-shared table; it takes the lease"
        );
    }
}
