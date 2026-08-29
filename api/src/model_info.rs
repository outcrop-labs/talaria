// Human-readable model identity, populated automatically — port of the read
// half of model-info.ts. OpenRouter's public catalog (no key) carries a
// pretty name + description for essentially every major model; we match
// registered ids against it the same way the price oracle does (full id for
// slashed ids, unambiguous suffix for bare ones) and serve a display label +
// a one-line "what it's good at" blurb. Unknown models (e.g. self-hosted)
// simply have no blurb — nothing is invented.
//
// The org-voice rewrite pass (maybeRewriteBlurbs and its harness sweep) DOES
// NOT RUN FROM RUST: it is a harness run, and harness runs port with the runs
// plane. The degradation is cosmetic and self-healing — the TS scheduler
// keeps writing model_blurbs rows while it owns the plane, and the rows are
// read here either way.

use sqlx::PgPool;
use std::collections::HashMap;
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
}
