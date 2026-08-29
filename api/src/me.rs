// /api/me's data layer — the users.ts pref accessors (preferred model,
// preferred effort, timezone) plus model-access.ts's member allowlist, the
// two pieces a profile PUT needs beyond its own row. Each is a plain column
// read/write; the policy (who may set what) lives in the route, exactly where
// me.ts keeps it.

use crate::gateway::models::{EndpointModels, GatewayModel, catalog_of};
use crate::gateway::settings::get_setting;
use sqlx::PgPool;

const ALLOWLIST_KEY: &str = "member_model_allowlist";

/// The three columns GET answers with, in wire order. A missing row is the
/// all-null answer (TS: `rows[0]?.m ?? null` and friends).
pub type PrefRow = (Option<String>, Option<String>, Option<String>);

pub async fn get_prefs(pg: &PgPool, user_id: &str) -> Result<PrefRow, sqlx::Error> {
    let row: Option<PrefRow> = sqlx::query_as(
        "select preferred_model, preferred_effort, timezone from users where id = $1::uuid",
    )
    .bind(user_id)
    .fetch_optional(pg)
    .await?;
    Ok(row.unwrap_or((None, None, None)))
}

pub async fn set_user_name(pg: &PgPool, user_id: &str, name: &str) -> Result<(), sqlx::Error> {
    sqlx::query("update users set name = $2 where id = $1::uuid")
        .bind(user_id)
        .bind(name)
        .execute(pg)
        .await?;
    Ok(())
}

pub async fn set_preferred_model(
    pg: &PgPool,
    user_id: &str,
    model: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query("update users set preferred_model = $2 where id = $1::uuid")
        .bind(user_id)
        .bind(model)
        .execute(pg)
        .await?;
    Ok(())
}

pub async fn set_preferred_effort(
    pg: &PgPool,
    user_id: &str,
    effort: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query("update users set preferred_effort = $2 where id = $1::uuid")
        .bind(user_id)
        .bind(effort)
        .execute(pg)
        .await?;
    Ok(())
}

pub async fn set_timezone(pg: &PgPool, user_id: &str, tz: Option<&str>) -> Result<(), sqlx::Error> {
    sqlx::query("update users set timezone = $2 where id = $1::uuid")
        .bind(user_id)
        .bind(tz)
        .execute(pg)
        .await?;
    Ok(())
}

/// The endpoint rows the catalog reads, in the TS query's order (local first,
/// then name asc — first-seen order is what a pooled bare id's endpoint list
/// preserves).
async fn endpoint_models(pg: &PgPool) -> Result<Vec<EndpointModels>, sqlx::Error> {
    // `models` is a jsonb column: sqlx maps Vec<String> to text[], so the
    // decode goes through Json<Vec<String>>.
    let rows: Vec<(String, sqlx::types::Json<Vec<String>>)> = sqlx::query_as(
        "select name, models from llm_endpoints order by (class = 'local') desc, name asc",
    )
    .fetch_all(pg)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(name, models)| EndpointModels {
            name,
            models: models.0,
        })
        .collect())
}

/// gatewayModels() — the every-endpoint catalog, one fetch shared by the
/// member model gate here as it is in TS.
pub async fn gateway_models(pg: &PgPool) -> Result<Vec<GatewayModel>, sqlx::Error> {
    Ok(catalog_of(&endpoint_models(pg).await?))
}

/// The stored member allowlist (memberModelAllowlist). Non-string entries in
/// a hand-edited row can never equal a model id, so they are dropped here
/// rather than carried.
pub async fn member_model_allowlist(pg: &PgPool) -> Vec<String> {
    let stored = get_setting(pg, ALLOWLIST_KEY, serde_json::json!([])).await;
    stored
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

/// May this role use this model id (modelAllowedFor)? Admins and an empty
/// allowlist are open; a bare id must be listed; an endpoint-qualified id is
/// judged by the bare model it pins.
pub fn model_allowed_for(
    role: &str,
    model: &str,
    allow: &[String],
    catalog: &[GatewayModel],
) -> bool {
    if role == "admin" || allow.is_empty() {
        return true;
    }
    if allow.iter().any(|a| a == model) {
        return true;
    }
    if let Some(entry) = catalog.iter().find(|m| m.id == model)
        && entry.qualified
    {
        // "ep/rest": allowed iff the pinned bare model is allowed.
        let rest = &model[model.find('/').expect("qualified ids contain '/'") + 1..];
        return allow.iter().any(|a| a == rest);
    }
    false
}

/// Can this runtime resolve `tz` as a zone (isValidTimeZone)? TS asks
/// Intl.DateTimeFormat to RESOLVE the name — never a regex — and the answer
/// is what ECMA-402 accepts. node's V8 and bun's JSC ICUs were probed on the
/// full table pinned in the test below and agreed on every spelling:
///
/// - an IANA name, CASE-INSENSITIVE — "utc", "AMERICA/NEW_YORK", and
///   "Etc/gmt+5" all resolve, because the runtime canonicalizes while it
///   looks up. chrono-tz's TZ_VARIANTS is the same tzdb (zones and backward
///   links both: Zulu, US/Pacific, ROC, W-SU, EST5EDT all there), minus the
///   three names ICU refuses outright (Factory, localtime, posixrules) —
///   none of which chrono-tz carries either, so there is no denylist.
/// - an offset form: `±HH`, `±HH:MM`, or `±HHMM` — two-digit hour ≤ 23 and,
///   when present, two-digit minute ≤ 59.
///
/// The stored value drives scheduled work (brief opens, digest sends), so a
/// typo must die here — this is the whole server-side contract.
pub fn is_valid_time_zone(tz: &str) -> bool {
    if chrono_tz::TZ_VARIANTS
        .iter()
        .any(|v| v.name().eq_ignore_ascii_case(tz))
    {
        return true;
    }
    is_offset_zone(tz)
}

fn two_digits(a: u8, b: u8) -> Option<u8> {
    if a.is_ascii_digit() && b.is_ascii_digit() {
        Some((a - b'0') * 10 + (b - b'0'))
    } else {
        None
    }
}

/// The offset grammar: sign, exactly two digits, then optionally a colon and
/// exactly two more. The sign is required ("05:30", "0000" fail) and carries
/// no bound of its own ("-00:00" resolves); "+5:00", "+05:0", and "+05:000"
/// fail on shape alone.
fn is_offset_zone(tz: &str) -> bool {
    let b = tz.as_bytes();
    if b.first() != Some(&b'+') && b.first() != Some(&b'-') {
        return false;
    }
    let rest = &b[1..];
    let (hour, minute) = match rest {
        [h0, h1] => (two_digits(*h0, *h1), Some(0)),
        [h0, h1, m0, m1] => (two_digits(*h0, *h1), two_digits(*m0, *m1)),
        [h0, h1, b':', m0, m1] => (two_digits(*h0, *h1), two_digits(*m0, *m1)),
        _ => (None, None),
    };
    matches!((hour, minute), (Some(h), Some(m)) if h <= 23 && m <= 59)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog() -> Vec<GatewayModel> {
        catalog_of(&[
            EndpointModels {
                name: "claude-a".into(),
                models: vec!["m".into(), "shared".into()],
            },
            EndpointModels {
                name: "claude-b".into(),
                models: vec!["m".into()],
            },
            EndpointModels {
                name: "openrouter".into(),
                models: vec!["o-big".into(), "shared".into()],
            },
        ])
    }

    #[test]
    fn model_gate_admin_and_open_allowlist_bypass() {
        let cat = catalog();
        let one = vec!["m".to_string()];
        // Admins and an empty allowlist are open to everything — even an id
        // neither endpoint serves.
        assert!(model_allowed_for("admin", "anything", &one, &cat));
        assert!(model_allowed_for("member", "anything", &[], &cat));
    }

    #[test]
    fn model_gate_bare_ids_must_be_listed() {
        let cat = catalog();
        let one = vec!["m".to_string()];
        assert!(model_allowed_for("member", "m", &one, &cat));
        // "shared" is pooled bare — it must be listed by its bare spelling.
        assert!(!model_allowed_for("member", "shared", &one, &cat));
        assert!(model_allowed_for(
            "member",
            "shared",
            &["shared".to_string()],
            &cat
        ));
        // An id no endpoint serves is judged as a bare id: listed or not.
        assert!(!model_allowed_for("member", "nope", &one, &cat));
    }

    #[test]
    fn model_gate_qualified_ids_pin_the_bare_model() {
        let cat = catalog();
        let one = vec!["m".to_string()];
        // "claude-a/m" is qualified; allowing "m" allows it…
        assert!(model_allowed_for("member", "claude-a/m", &one, &cat));
        // …on ANY endpoint that serves the model — the allowlist names
        // models, never endpoints.
        assert!(model_allowed_for("member", "claude-b/m", &one, &cat));
        // A qualified id no endpoint serves pins nothing the catalog knows —
        // TS's `entry?.qualified` on undefined is the same refusal.
        assert!(!model_allowed_for("member", "openrouter/m", &one, &cat));
        // Allowing the qualified spelling does NOT allow the bare one.
        let q = vec!["claude-a/m".to_string()];
        assert!(!model_allowed_for("member", "m", &q, &cat));
        // A bare id may itself contain '/' (OpenRouter names) — only the
        // catalog's qualified flag decides how it is judged.
        assert!(!model_allowed_for("member", "openrouter/o-big", &one, &cat));
    }

    /// Every row probed against node 26's Intl AND bun's, where both
    /// runtimes agreed — this table IS the contract. The lowercase and
    /// offset rows are the ones a tzdb-exact port would get wrong.
    #[test]
    fn time_zone_table_matches_the_intl_probe() {
        let accepted = [
            "America/New_York",
            "UTC",
            "Etc/UTC",
            "Etc/GMT+5",
            "Etc/GMT-14",
            "US/Pacific",
            "Canada/Eastern",
            "GMT",
            "EST",
            "EST5EDT",
            "MST7MDT",
            "America/Argentina/Buenos_Aires",
            "Pacific/Kiritimati",
            "Asia/Kolkata",
            "utc",
            "America/new_york",
            "AMERICA/NEW_YORK",
            "aMeRiCa/New_York",
            "Etc/gmt+5",
            "Antarctica/McMurdo",
            "Chile/Continental",
            "Mexico/BajaNorte",
            "Iran",
            "Japan",
            "Israel",
            "Turkey",
            "ROC",
            "ROK",
            "PRC",
            "W-SU",
            "Zulu",
            "CET",
            "EET",
            "WET",
            "MET",
            "PST8PDT",
            "Navajo",
            "Universal",
            "Etc/Universal",
            "Etc/Greenwich",
            "Greenwich",
            "Iceland",
            "Jamaica",
            "Kwajalein",
            "Poland",
            "Portugal",
            "Singapore",
            "Hongkong",
            "Uct",
            "UCT",
            "Etc/Zulu",
            "Etc/GMT-0",
            "Etc/GMT+0",
            "Etc/GMT0",
            "GMT+0",
            "GMT-0",
            "gmt",
            "uTc",
            "-00:00",
            "+00:00",
            "+23:59",
            "-23:59",
            "+05",
            "+05:00",
            "-08:00",
            "+0530",
            "-0500",
            "+2300",
            "+2359",
            "-2359",
        ];
        for tz in accepted {
            assert!(is_valid_time_zone(tz), "should accept {tz:?}");
        }
        let refused = [
            "Factory",
            "localtime",
            "posixrules",
            " America/New_York",
            "America/New_York ",
            "America/New_York/",
            "America//New_York",
            "GMT+5",
            "UTC+3",
            "Etc/GMT+24",
            "Etc/GMT+25",
            "GMT-14",
            "GMT+14",
            "+5:00",
            "+24:00",
            "+25:00",
            "+25",
            "+23:60",
            "05:30",
            "0000",
            "1200",
            "+0",
            "+05:0",
            "+05:000",
            "+0:00",
            "+005",
            "+05:30:00",
            "99:00",
            "+:00",
            "Z",
            "Zulu/East",
            "local",
            "bogus",
            "America/Gotham",
            "",
            "x",
        ];
        for tz in refused {
            assert!(!is_valid_time_zone(tz), "should refuse {tz:?}");
        }
    }
}
