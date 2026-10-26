// The per-app gateway key — the vocabulary the app lifecycle (talaria-apps,
// which mints, seals and revokes the keys) and the chat relay (llm_chat.rs,
// which judges calls on them) share.
//
// Every model call the app host makes on an app's behalf — a harness run from
// the app's server, an MCP tool handler or a fitness replay of one of its
// harnesses — reaches the relay on ONE `tlk_` credential named `app:<slug>`.
// The app host holds it: the api mints it on the host's internal call, seals
// the secret into `app_llm_keys` (docs/ENCRYPTION.md), and revokes it when
// the app is disabled or uninstalled. App code never sees it. So the relay can
// answer, for an app's call, what it answers for an agent's:
//
//   WHO is calling     the app — its budget subject is `app:<slug>`, and
//                      every row it books lands under the run's caller stamp
//                      inside that namespace (`app:<slug>` or
//                      `app:<slug>:<…>`), with ledger source `app`
//   WHAT it may do     call any model the gateway serves, or drive a live
//                      fleet agent for one harness turn (the agent's own
//                      calls are then metered once, at the relay, as `app`
//                      spend — the #584 attribution)
//   WHAT is skipped    the relay's own guard pass: the host's harness runner
//                      guards every reply with the harness's narrowed rule
//                      set, and a second pass would file every finding twice
//
// THE RECORD IS THE AUTHORITY, NOT THE NAME. A key's name is a string anybody
// with the mint permission could type, so a key called `app:<slug>` is an
// app's key only while `app_llm_keys` records that key's id for the slug.
// The keys route refuses the prefix as well (models/keys.rs); the record is
// what holds even for a row that predates the refusal.
//
// The record is one app_settings row, read through the hot window on every
// app call:
//
//   { "<slug>": { "keyId": "<llm_api_keys.id>", "sealed": "v2:…" }, … }
//
// Its writers are `set_setting` (mint, revoke) and the secret rotation, which
// re-seals `sealed` in place and never touches `keyId` — the one field the
// relay reads.

use crate::settings::get_setting_hot;
use serde_json::{Value, json};
use sqlx::PgPool;

/// The name prefix every per-app gateway key carries.
pub const APP_KEY_PREFIX: &str = "app:";
/// The settings row holding every app key's id and sealed secret.
pub const APP_KEYS_SETTING: &str = "app_llm_keys";
/// The ledger source of every row an app key books, and of an agent turn an
/// app drives.
pub const APP_SOURCE: &str = "app";

/// The run's caller stamp, as the app host sends it. Kept inside the app's
/// namespace by [`app_caller_stamp`].
pub const CALLER_HEADER: &str = "x-talaria-caller";
/// The run's own ref (a conversation, a run, a record of the app's) — booked
/// only when it is a uuid, and never at the cost of the row.
pub const REF_HEADER: &str = "x-talaria-ref-id";
/// The ticket the run's spend belongs to — a uuid, or nothing.
pub const TASK_HEADER: &str = "x-talaria-task-id";
/// How long an agent turn may hold for a restarting agent, in ms.
pub const HOLD_HEADER: &str = "x-talaria-hold-ms";
/// Which side served an app call: `gateway` or `fleet`.
pub const TRANSPORT_HEADER: &str = "x-talaria-transport";
/// The contract parameters that did not reach the model, by capability
/// (`json`, `tools`), comma-separated. Absent when nothing was dropped.
pub const CONTRACT_DROPS_HEADER: &str = "x-talaria-contract-drops";
/// Set on an agent turn that answered with a canned sentence (`mock`,
/// `hold`) — an outage wearing a 200, never a reply.
pub const CANNED_HEADER: &str = "x-talaria-canned";

/// The longest caller stamp a row keeps. A harness id is at most 120
/// characters and a slug at most 64; this bounds what a stamp can write.
const STAMP_MAX: usize = 200;
/// The longest hold an app may ask for: the work-session ceiling.
const HOLD_MAX_MS: u64 = 600_000;

/// Apps' slug rule (talaria-app-manifest), so a name the lifecycle could
/// never have minted is never read as an app's key.
fn slug_ok(slug: &str) -> bool {
    talaria_app_manifest::slug_ok(slug)
}

/// The key name for an app: `app:<slug>`.
pub fn app_key_name(slug: &str) -> String {
    format!("{APP_KEY_PREFIX}{slug}")
}

/// The slug a key name carries, when it is shaped like an app's — `app:crm`
/// → `crm`. Any other name answers None. Shape only: whether the key IS the
/// app's is the record's question ([`app_key_is_current`]).
pub fn app_slug_of_key(key_name: &str) -> Option<&str> {
    key_name.strip_prefix(APP_KEY_PREFIX).filter(|s| slug_ok(s))
}

/// The name a row an app call books lands under: the run's caller stamp,
/// kept inside the app's namespace. A stamp that already is `app:<slug>` or
/// starts `app:<slug>:` is kept; any other label is prefixed with the app's
/// namespace (`fitness:x` → `app:<slug>:fitness:x`) — so an app that labels
/// its runs `app:other:…` books `app:<slug>:app:other:…`, never another
/// app's spend. No stamp, or one that is empty, unprintable or too long,
/// books under the key's own name.
pub fn app_caller_stamp(slug: &str, raw: Option<&str>) -> String {
    let own = app_key_name(slug);
    let Some(stamp) = raw.map(str::trim).filter(|s| {
        !s.is_empty() && s.len() <= STAMP_MAX && s.bytes().all(|b| b.is_ascii_graphic())
    }) else {
        return own;
    };
    if stamp == own || stamp.starts_with(&format!("{own}:")) {
        return stamp.to_string();
    }
    let prefixed = format!("{own}:{stamp}");
    if prefixed.len() > STAMP_MAX {
        return own;
    }
    prefixed
}

/// The rows a budget subject's spend sums over, beyond its own name: an app's
/// subject (`app:<slug>`) is every row its runs booked under their caller
/// stamps (`app:<slug>:…`). Any other subject is its own name alone.
pub fn app_family_prefix(subject: &str) -> Option<String> {
    subject
        .strip_prefix(APP_KEY_PREFIX)
        .filter(|s| slug_ok(s))
        .map(|_| format!("{subject}:"))
}

/// A hold the app host asked for, bounded. Unset, unreadable or zero is the
/// transport's own default.
pub fn hold_ms_of(raw: Option<&str>) -> Option<u64> {
    raw?.trim()
        .parse::<u64>()
        .ok()
        .filter(|ms| *ms > 0)
        .map(|ms| ms.min(HOLD_MAX_MS))
}

/// The key id `app_llm_keys` records for `slug`. Pure over the row's JSON.
pub fn recorded_key_id<'a>(setting: &'a Value, slug: &str) -> Option<&'a str> {
    setting
        .get(slug)?
        .get("keyId")?
        .as_str()
        .filter(|s| !s.is_empty())
}

/// Is `key_id` the key the lifecycle recorded for `slug`? Through the hot
/// window: a mint or a revoke writes through `set_setting`, which drops the
/// window, so a rotation lands on the next call.
pub async fn app_key_is_current(pg: &PgPool, slug: &str, key_id: &str) -> bool {
    let setting = get_setting_hot(pg, APP_KEYS_SETTING, json!({})).await;
    recorded_key_id(&setting, slug) == Some(key_id)
}

/// The contract-drop header's value: each capability once, in first-seen
/// order. None when nothing was dropped.
pub fn contract_drops_value<'a>(caps: impl IntoIterator<Item = &'a str>) -> Option<String> {
    let mut seen: Vec<&str> = Vec::new();
    for c in caps {
        if !seen.contains(&c) {
            seen.push(c);
        }
    }
    (!seen.is_empty()).then(|| seen.join(","))
}

/// The live fleet agent a model id names, for an app's agent turn: the id
/// itself when it is an agent, or the agent a tier (`<agent>-<alias>`, split
/// at the LAST hyphen) belongs to. The transport rule's fleet half
/// (talaria-harness `transport_kind`), over the agent ids `list_agents`
/// answers. The answer is the agent's model id — the name its key's calls
/// are booked under, so the turn's attribution lands on them.
pub fn persona_base<'a>(model: &str, agents: &'a [String]) -> Option<&'a str> {
    if let Some(a) = agents.iter().find(|a| *a == model) {
        return Some(a);
    }
    let cut = model.rfind('-').filter(|c| *c > 0)?;
    agents
        .iter()
        .find(|a| **a == model[..cut])
        .map(String::as_str)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_app_key_is_named_by_its_slug_and_only_a_real_slug_reads_back() {
        assert_eq!(app_key_name("crm"), "app:crm");
        assert_eq!(app_slug_of_key("app:crm"), Some("crm"));
        assert_eq!(app_slug_of_key("app:support-desk"), Some("support-desk"));
        // Not an app's shape: an operator's key, an agent's, an empty or
        // malformed slug.
        for name in [
            "crm",
            "agent:crm",
            "app:",
            "app:CRM",
            "app:a:b",
            "app:-x",
            "api:app:crm",
        ] {
            assert_eq!(app_slug_of_key(name), None, "{name}");
        }
    }

    #[test]
    fn a_caller_stamp_stays_inside_the_apps_namespace() {
        // The app's own name, and anything under it, is kept.
        assert_eq!(app_caller_stamp("crm", Some("app:crm")), "app:crm");
        assert_eq!(
            app_caller_stamp("crm", Some("app:crm:support:triage")),
            "app:crm:support:triage"
        );
        // Any other label is prefixed, never trusted.
        assert_eq!(
            app_caller_stamp("crm", Some("fitness:support:triage")),
            "app:crm:fitness:support:triage"
        );
        assert_eq!(
            app_caller_stamp("crm", Some("app:billing:x")),
            "app:crm:app:billing:x",
            "an app cannot book another app's spend"
        );
        assert_eq!(
            app_caller_stamp("crm", Some("app:crmx")),
            "app:crm:app:crmx",
            "a prefix of the slug is not the slug"
        );
        // Nothing usable: the key's own name.
        for raw in [None, Some(""), Some("   "), Some("has space"), Some("é")] {
            assert_eq!(app_caller_stamp("crm", raw), "app:crm", "{raw:?}");
        }
        let long = "x".repeat(STAMP_MAX);
        assert_eq!(app_caller_stamp("crm", Some(&long)), "app:crm");
    }

    #[test]
    fn an_apps_budget_sums_every_stamp_under_it_and_nothing_else_does() {
        assert_eq!(app_family_prefix("app:crm").as_deref(), Some("app:crm:"));
        for subject in ["api:crm", "x-eng", "app:", "app:crm:triage", "agent:crm"] {
            assert_eq!(app_family_prefix(subject), None, "{subject}");
        }
    }

    #[test]
    fn a_hold_is_bounded_and_nothing_usable_is_the_default() {
        assert_eq!(hold_ms_of(Some("60000")), Some(60_000));
        assert_eq!(hold_ms_of(Some(" 1200000 ")), Some(HOLD_MAX_MS));
        for raw in [None, Some(""), Some("0"), Some("-5"), Some("soon")] {
            assert_eq!(hold_ms_of(raw), None, "{raw:?}");
        }
    }

    #[test]
    fn the_record_names_the_key_and_a_missing_one_names_none() {
        let row = json!({ "crm": { "keyId": "k-1", "sealed": "v2:x" }, "bare": {}, "empty": { "keyId": "" } });
        assert_eq!(recorded_key_id(&row, "crm"), Some("k-1"));
        assert_eq!(recorded_key_id(&row, "bare"), None);
        assert_eq!(recorded_key_id(&row, "empty"), None);
        assert_eq!(recorded_key_id(&row, "gone"), None);
        assert_eq!(recorded_key_id(&json!(null), "crm"), None);
    }

    #[test]
    fn contract_drops_name_each_capability_once() {
        assert_eq!(contract_drops_value([]), None);
        assert_eq!(contract_drops_value(["json"]).as_deref(), Some("json"));
        assert_eq!(
            contract_drops_value(["tools", "json", "tools"]).as_deref(),
            Some("tools,json")
        );
    }

    #[test]
    fn a_persona_is_an_agent_or_a_tier_of_one() {
        let agents = vec!["penny-ops".to_string(), "engineer-engineering".to_string()];
        assert_eq!(persona_base("penny-ops", &agents), Some("penny-ops"));
        assert_eq!(
            persona_base("engineer-engineering-opus", &agents),
            Some("engineer-engineering"),
            "a tier books under its agent"
        );
        assert_eq!(persona_base("anthropic/claude-opus-5", &agents), None);
        assert_eq!(persona_base("penny", &agents), None);
        assert_eq!(persona_base("other-ops-opus", &agents), None);
        assert_eq!(
            persona_base("penny-ops", &[]),
            None,
            "an unrendered fleet is no persona"
        );
    }
}
