// THE PROVIDER REGISTRY AND THE ONE CONFIG ROW — modelled field-for-field on
// `RERANK_PROVIDERS` (talaria-retrieval-rerank), because that pattern is
// already load-bearing in this repo and an operator already knows how to drive
// it: pick a provider, give it a URL and/or a key, choose a model, and the key
// is sealed in `app_settings` where plaintext never reaches a client.
//
// NO BLESSED DEFAULT AND NO RECOMMENDED MODEL, and this is a position rather
// than an omission. Talaria's job is to make the choice real and to state
// honestly what each option can and cannot do; it is not to have an opinion
// about which decision model an operator should run. So the registry carries a
// CAPABILITY SHEET per provider — which primitives it serves, whether it
// answers several questions in one request, and where its probability actually
// comes from — and `decide` refuses a question a provider cannot answer
// instead of letting it guess. Where a provider publishes a catalog we list it
// live; where it does not we carry its documented ids as a fallback, exactly
// as the rerank panel does.
//
// `custom` IS WHAT MAKES THE LIST OPEN rather than a menu. It takes a URL, an
// optional key, and a declaration of which wire shape the endpoint speaks — so
// a System One competitor, a fine-tuned classifier behind vLLM, an in-house
// service, or a model that does not exist yet needs no code from us. The four
// shipped entries cover the four SHAPES a decision model arrives in, not four
// vendors we endorse.
//
// `off` IS THE DEFAULT, and that is how "optional feature" is honoured in fact
// rather than in prose: on every existing install and every fresh one, the
// port is off and every call site runs the code it ran before this crate
// existed.

use serde::Serialize;
use serde_json::{Value, json};
use sqlx::PgPool;
use talaria_gateway::settings::{get_setting, set_setting};
use talaria_state::AppState;

// ── The wire shapes ──────────────────────────────────────────────────────────

/// Which protocol an endpoint speaks. A provider id names a SERVICE; a wire
/// names the bytes. They are separate because `custom` is one service nobody
/// has built yet speaking one of three shapes we already know.
pub const WIRE_SYSTEMONE: &str = "systemone";
pub const WIRE_PREDICT: &str = "predict";
pub const WIRE_CHAT: &str = "chat";

pub const ALL_WIRES: [&str; 3] = [WIRE_SYSTEMONE, WIRE_PREDICT, WIRE_CHAT];

// ── The provider table ───────────────────────────────────────────────────────

pub struct DecideProviderMeta {
    pub id: &'static str,
    pub label: &'static str,
    /// Where the judgment is made. `off` and the self-hosted entries answer
    /// "your hardware"; a hosted API names its jurisdiction, because state
    /// here is real ticket and message text and an operator choosing a
    /// provider is choosing where that text goes.
    pub country: &'static str,
    pub needs_url: bool,
    pub needs_key: bool,
    /// Must the operator name a model? The two wires that carry a model id
    /// (`systemone`, `chat`) need one; a classifier sidecar serves whatever it
    /// was started with, and a custom endpoint decides for itself.
    pub needs_model: bool,
    /// Must the operator declare which wire shape the endpoint speaks?
    /// Only `custom` does — every other entry knows its own.
    pub needs_wire: bool,
    /// The wire this provider speaks when it is not declared.
    pub wire: &'static str,
    /// Documented models — the fallback when there is no live catalog API.
    pub fallback_models: &'static [&'static str],
    pub live_catalog: bool,
    /// THE CAPABILITY SHEET, and the reason it is here rather than assumed.
    /// Which primitives this provider can answer at all.
    pub primitives: &'static [&'static str],
    /// Can it answer several questions over one state in ONE request? A
    /// hosted decision model can; a classifier is one forward pass per
    /// question, so asking it five is five calls — a cost the caller must
    /// have chosen, which is why `decide` refuses instead of spending it.
    pub fans_out: bool,
    /// WHERE THE PROBABILITY COMES FROM, which is not a detail:
    ///   `native`     the model is trained to emit calibrated probabilities
    ///   `classifier` a softmax over labels from a classification head
    ///   `logprobs`   derived from a text model's token logprobs, and absent
    ///                entirely on an endpoint that does not serve them
    pub calibration: &'static str,
}

pub const DECIDE_PROVIDERS: &[DecideProviderMeta] = &[
    // The off switch is a provider so that "which decision model" has exactly
    // one answer slot, and the default lives in the same vocabulary as every
    // other choice instead of being a second absent-means-off rule.
    DecideProviderMeta {
        id: "off",
        label: "Off",
        country: "—",
        needs_url: false,
        needs_key: false,
        needs_model: false,
        needs_wire: false,
        wire: WIRE_SYSTEMONE,
        fallback_models: &[],
        live_catalog: false,
        primitives: &[],
        fans_out: false,
        calibration: "none",
    },
    // TypeSafe's Jev — the model this port was designed against, because it is
    // the one that exists and publishes numbers. It is not the model the
    // architecture assumes.
    DecideProviderMeta {
        id: "jev",
        label: "TypeSafe (Jev)",
        country: "US",
        needs_url: false,
        needs_key: true,
        needs_model: true,
        needs_wire: false,
        wire: WIRE_SYSTEMONE,
        fallback_models: &["jev-latest", "jev-preview", "jev-1.13.0"],
        live_catalog: false,
        primitives: &["noul", "choice", "score"],
        fans_out: true,
        calibration: "native",
    },
    // A classifier on the operator's own hardware, through the TEI sidecar
    // this repo already runs and already calls for reranking. Narrower than a
    // hosted decision model and honest about it: one question per forward
    // pass, and `score` is out because a classification head over ordered
    // levels is a different model, not a different request.
    DecideProviderMeta {
        id: "tei",
        label: "Self-hosted classifier (TEI)",
        country: "your hardware",
        needs_url: true,
        needs_key: false,
        needs_model: false,
        needs_wire: false,
        wire: WIRE_PREDICT,
        fallback_models: &[],
        live_catalog: false,
        primitives: &["noul", "choice"],
        fans_out: false,
        calibration: "classifier",
    },
    // Any model the operator already registered on /models, asked for a
    // constrained answer. The one whose confidence may simply not exist: an
    // endpoint that does not serve `logprobs` answers with no distribution
    // behind it, and the judgment comes back `calibrated: false` so no
    // threshold can read a number that is not there.
    //
    // TWO LIMITS WORTH NAMING RATHER THAN LETTING SOMEBODY FIND. This adapter
    // speaks OPENAI-COMPATIBLE `/chat/completions` — `logprobs`/`top_logprobs`
    // and the one-token contract are that dialect's, so a native Anthropic or
    // Gemini endpoint is reachable here only through a compatibility layer.
    // And it calls the endpoint DIRECTLY (resolving the row and its sealed key
    // through `talaria_gateway`), not through the metered relay: a decision is
    // not a conversation, and routing it through the relay would put guard
    // passes and persona accounting on a yes/no. The cost of that choice is
    // that these calls do not land in the token ledger, which is a gap to
    // close if this provider ever carries real volume.
    DecideProviderMeta {
        id: "gateway",
        label: "A registered model",
        country: "wherever that endpoint is",
        needs_url: false,
        needs_key: false,
        needs_model: true,
        needs_wire: false,
        wire: WIRE_CHAT,
        fallback_models: &[],
        live_catalog: true,
        primitives: &["noul", "choice", "score"],
        fans_out: true,
        calibration: "logprobs",
    },
    // The entry that makes the registry open. Capabilities are declared
    // optimistically — we cannot know what an endpoint we have never seen can
    // do, and refusing a primitive on a guess would make the escape hatch
    // narrower than the things it exists to reach.
    DecideProviderMeta {
        id: "custom",
        label: "Custom endpoint",
        country: "wherever you point it",
        needs_url: true,
        needs_key: false,
        needs_model: false,
        needs_wire: true,
        wire: WIRE_SYSTEMONE,
        fallback_models: &[],
        live_catalog: false,
        primitives: &["noul", "choice", "score"],
        fans_out: true,
        calibration: "native",
    },
];

pub fn meta_of(id: &str) -> Option<&'static DecideProviderMeta> {
    DECIDE_PROVIDERS
        .iter()
        .find(|p| p.id == id && p.id != "off")
}

/// The wire view of the provider catalog — the admin GET's `providers` array.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DecideProviderPublic {
    pub id: &'static str,
    pub label: &'static str,
    pub country: &'static str,
    pub needs_url: bool,
    pub needs_key: bool,
    pub needs_model: bool,
    pub needs_wire: bool,
    pub wire: &'static str,
    pub fallback_models: &'static [&'static str],
    pub live_catalog: bool,
    pub primitives: &'static [&'static str],
    pub fans_out: bool,
    pub calibration: &'static str,
}

pub fn providers_public() -> Vec<DecideProviderPublic> {
    DECIDE_PROVIDERS
        .iter()
        .map(|p| DecideProviderPublic {
            id: p.id,
            label: p.label,
            country: p.country,
            needs_url: p.needs_url,
            needs_key: p.needs_key,
            needs_model: p.needs_model,
            needs_wire: p.needs_wire,
            wire: p.wire,
            fallback_models: p.fallback_models,
            live_catalog: p.live_catalog,
            primitives: p.primitives,
            fans_out: p.fans_out,
            calibration: p.calibration,
        })
        .collect()
}

// ── The stored row ───────────────────────────────────────────────────────────

/// Serialized camelCase with absent fields OMITTED — byte-stable against the
/// stored row, which the admin surface carries through verbatim.
#[derive(Debug, Clone, Default, PartialEq, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DecideConfig {
    #[serde(default)]
    pub provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// `custom` only — which wire shape the endpoint speaks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wire: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_sealed: Option<String>,
    /// The per-call budget. A decision in a request path is only worth having
    /// if it answers inside the time the surface has — the port falls back
    /// rather than holding a page open.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<i64>,
}

const KEY: &str = "decide_config";

/// The default timeout. Jev's documented envelope is 70–500ms, so 4s is not a
/// latency budget — it is the point past which something is wrong and the
/// caller's own path is better than more waiting.
pub const DEFAULT_TIMEOUT_MS: u64 = 4_000;

fn defaults_value() -> Value {
    json!({"provider": "off"})
}

async fn stored_config(pg: &PgPool) -> Value {
    get_setting(pg, KEY, defaults_value()).await
}

pub async fn get_decide_config(pg: &PgPool) -> DecideConfig {
    // A row whose shape no longer parses falls back to off rather than
    // poisoning every call site — unreachable unless the key is hand-edited.
    serde_json::from_value(stored_config(pg).await).unwrap_or(DecideConfig {
        provider: "off".into(),
        ..Default::default()
    })
}

impl DecideConfig {
    /// The wire this config speaks: the declared one where the provider asks
    /// for a declaration, the provider's own otherwise. An unknown declared
    /// wire answers None — a config naming a protocol we do not speak must
    /// not fall through to a different one.
    pub fn wire_of(&self, meta: &DecideProviderMeta) -> Option<&'static str> {
        if !meta.needs_wire {
            return Some(meta.wire);
        }
        let declared = self.wire.as_deref()?;
        ALL_WIRES.into_iter().find(|w| *w == declared)
    }

    pub fn timeout(&self) -> u64 {
        self.timeout_ms
            .filter(|t| *t > 0)
            .map(|t| (t as u64).clamp(250, 60_000))
            .unwrap_or(DEFAULT_TIMEOUT_MS)
    }
}

/// The patch the admin route sends. Absent (leave the field alone) is distinct
/// from null (clear it) — the nested `Option<Option<_>>` is that distinction.
pub struct DecidePatch {
    pub provider: Option<String>,
    pub url: Option<Option<String>>,
    pub model: Option<Option<String>>,
    pub wire: Option<Option<String>>,
    pub api_key: Option<Option<String>>,
    pub timeout_ms: Option<Option<i64>>,
}

/// A spread onto the stored row's own key order, not a merge onto a fixed
/// shape: assigning an existing key keeps its position, a NEW key appends, and
/// a clear REMOVES the key rather than nulling it. `key_sealed` arrives
/// already sealed — sealing needs the box, so the caller resolves it before
/// this pure fold runs.
fn apply_patch(
    next: &mut serde_json::Map<String, Value>,
    patch: DecidePatch,
    key_sealed: Option<Option<String>>,
) {
    fn put(next: &mut serde_json::Map<String, Value>, key: &str, v: Option<Option<Value>>) {
        match v {
            Some(Some(val)) => {
                next.insert(key.into(), val);
            }
            Some(None) => {
                next.remove(key);
            }
            None => {}
        }
    }
    if let Some(p) = patch.provider {
        next.insert("provider".into(), json!(p));
    }
    put(next, "url", patch.url.map(|u| u.map(|v| json!(v))));
    put(next, "model", patch.model.map(|m| m.map(|v| json!(v))));
    put(next, "wire", patch.wire.map(|w| w.map(|v| json!(v))));
    put(
        next,
        "timeoutMs",
        patch
            .timeout_ms
            .map(|t| t.map(|v| json!(v.clamp(250, 60_000)))),
    );
    put(next, "keySealed", key_sealed.map(|k| k.map(|v| json!(v))));
}

pub async fn set_decide_config(state: &AppState, patch: DecidePatch) -> Result<Value, String> {
    // Seal BEFORE anything touches the row — a broken box must not leave a
    // half-written config behind.
    let key_sealed = match &patch.api_key {
        Some(Some(plaintext)) => {
            let sb = state.secretbox().await?;
            Some(Some(sb.seal(plaintext).map_err(|e| e.to_string())?))
        }
        Some(None) => Some(None), // null clears
        None => None,
    };
    let mut next = match stored_config(&state.pg).await {
        Value::Object(m) => m,
        _ => serde_json::Map::new(),
    };
    apply_patch(&mut next, patch, key_sealed);
    let v = Value::Object(next);
    set_setting(&state.pg, KEY, &v)
        .await
        .map_err(|e| e.to_string())?;
    Ok(v)
}

/// Redacted view for the admin UI — never carries keySealed.
pub async fn decide_config_public(pg: &PgPool) -> Value {
    // `talaria_settings` owns this fold: several provider-config rows in
    // app_settings share the shape, and a second copy is how they come to
    // disagree about what counts as "a key is set".
    talaria_gateway::settings::public_of(stored_config(pg).await)
}

/// Is this config complete enough to try? The panel's own answer, computed
/// from the registry rather than from a second list that could drift.
pub fn configured(cfg: &DecideConfig) -> bool {
    let Some(meta) = meta_of(&cfg.provider) else {
        return false;
    };
    if meta.needs_url && cfg.url.as_deref().is_none_or(|u| u.trim().is_empty()) {
        return false;
    }
    if meta.needs_key && cfg.key_sealed.is_none() {
        return false;
    }
    if meta.needs_model && cfg.model.as_deref().is_none_or(|m| m.trim().is_empty()) {
        return false;
    }
    cfg.wire_of(meta).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_provider_table_is_the_admin_face_of_the_registry() {
        let pub_ids: Vec<&str> = providers_public().iter().map(|p| p.id).collect();
        let reg_ids: Vec<&str> = DECIDE_PROVIDERS.iter().map(|p| p.id).collect();
        assert_eq!(pub_ids, reg_ids, "the panel sees every provider, in order");
    }

    #[test]
    fn off_is_first_so_the_default_is_the_panels_first_option() {
        assert_eq!(DECIDE_PROVIDERS[0].id, "off");
        assert_eq!(defaults_value(), json!({"provider": "off"}));
    }

    #[test]
    fn off_is_never_dispatchable_however_it_is_spelled() {
        // `meta_of` is the only door to a dispatch, and `off` must not open
        // it — otherwise "off" would mean "jev with no key", which fails in a
        // much more confusing way.
        assert!(meta_of("off").is_none());
        assert!(meta_of("").is_none());
        assert!(meta_of("nonsense").is_none());
        assert!(meta_of("jev").is_some());
    }

    #[test]
    fn every_dispatchable_provider_declares_at_least_one_primitive_and_a_known_wire() {
        for p in DECIDE_PROVIDERS.iter().filter(|p| p.id != "off") {
            assert!(
                !p.primitives.is_empty(),
                "{} serves nothing and could never answer",
                p.id
            );
            for prim in p.primitives {
                assert!(
                    ["noul", "choice", "score"].contains(prim),
                    "{} declares unknown primitive {prim}",
                    p.id
                );
            }
            assert!(
                ALL_WIRES.contains(&p.wire),
                "{} speaks unknown wire {}",
                p.id,
                p.wire
            );
        }
    }

    #[test]
    fn a_provider_needing_a_wire_declaration_is_incomplete_without_one() {
        let custom = meta_of("custom").unwrap();
        let mut cfg = DecideConfig {
            provider: "custom".into(),
            url: Some("https://example.invalid".into()),
            ..Default::default()
        };
        assert_eq!(cfg.wire_of(custom), None, "no wire declared");
        assert!(!configured(&cfg));
        cfg.wire = Some("predict".into());
        assert_eq!(cfg.wire_of(custom), Some(WIRE_PREDICT));
        assert!(configured(&cfg));
        // A protocol we do not speak must not fall through to one we do.
        cfg.wire = Some("grpc".into());
        assert_eq!(cfg.wire_of(custom), None);
        assert!(!configured(&cfg));
    }

    #[test]
    fn a_provider_that_knows_its_own_wire_ignores_a_declared_one() {
        let jev = meta_of("jev").unwrap();
        let cfg = DecideConfig {
            provider: "jev".into(),
            wire: Some("predict".into()),
            ..Default::default()
        };
        assert_eq!(cfg.wire_of(jev), Some(WIRE_SYSTEMONE));
    }

    #[test]
    fn configured_demands_the_url_key_and_model_each_provider_declares_it_needs() {
        // jev needs a key and a model, and no url.
        let mut jev = DecideConfig {
            provider: "jev".into(),
            ..Default::default()
        };
        assert!(!configured(&jev), "no key");
        jev.key_sealed = Some("sealed".into());
        assert!(!configured(&jev), "no model — and we ship no default one");
        jev.model = Some("jev-latest".into());
        assert!(configured(&jev));

        // tei needs a url and no key, and a blank url is not a url.
        let mut tei = DecideConfig {
            provider: "tei".into(),
            ..Default::default()
        };
        assert!(!configured(&tei));
        tei.url = Some("   ".into());
        assert!(!configured(&tei));
        tei.url = Some("http://tei.internal:80".into());
        assert!(configured(&tei));

        // gateway needs no url or key — it reuses a registered endpoint's —
        // but it still has to be told WHICH model.
        let mut gw = DecideConfig {
            provider: "gateway".into(),
            ..Default::default()
        };
        assert!(!configured(&gw));
        gw.model = Some("some-endpoint:some-model".into());
        assert!(configured(&gw));

        // The classifier sidecar serves whatever it was started with, so a
        // model id is not its question.
        let tei_no_model = DecideConfig {
            provider: "tei".into(),
            url: Some("http://tei.internal:80".into()),
            ..Default::default()
        };
        assert!(configured(&tei_no_model));
    }

    #[test]
    fn the_patch_spreads_onto_the_rows_own_key_order_and_a_clear_removes_the_key() {
        let mut row = serde_json::Map::new();
        row.insert("provider".into(), json!("jev"));
        row.insert("model".into(), json!("jev-latest"));
        row.insert("keySealed".into(), json!("sealed"));
        apply_patch(
            &mut row,
            DecidePatch {
                provider: None,
                url: None,
                model: Some(Some("jev-1.13.0".into())),
                wire: None,
                api_key: None,
                timeout_ms: Some(Some(900)),
            },
            None,
        );
        // model keeps its POSITION, timeoutMs appends, keySealed untouched.
        let keys: Vec<&str> = row.keys().map(|k| k.as_str()).collect();
        assert_eq!(keys, vec!["provider", "model", "keySealed", "timeoutMs"]);
        assert_eq!(row["model"], json!("jev-1.13.0"));
        assert_eq!(row["timeoutMs"], json!(900));

        // A clear REMOVES rather than nulls — `hasKey` is derived from
        // presence, so a null would read as "a key is set" forever.
        apply_patch(
            &mut row,
            DecidePatch {
                provider: None,
                url: None,
                model: Some(None),
                wire: None,
                api_key: None,
                timeout_ms: None,
            },
            Some(None),
        );
        assert!(!row.contains_key("model"));
        assert!(!row.contains_key("keySealed"));
    }

    #[test]
    fn the_public_view_never_carries_the_sealed_key_and_says_whether_one_is_set() {
        let stored = json!({"provider": "jev", "keySealed": "sealed", "model": "jev-latest"});
        let pubv = talaria_gateway::settings::public_of(stored);
        assert!(pubv.get("keySealed").is_none(), "the seal never ships");
        assert_eq!(pubv["hasKey"], json!(true));
        assert_eq!(pubv["model"], json!("jev-latest"));
        // hasKey is appended LAST, after the row's own keys.
        let keys: Vec<&str> = pubv
            .as_object()
            .expect("an object")
            .keys()
            .map(|k| k.as_str())
            .collect();
        assert_eq!(keys, vec!["provider", "model", "hasKey"]);

        let absent = talaria_gateway::settings::public_of(json!({"provider": "off"}));
        assert_eq!(absent["hasKey"], json!(false));
    }

    #[test]
    fn a_hand_edited_unparseable_row_reads_as_off_rather_than_as_something_worse() {
        let cfg: DecideConfig =
            serde_json::from_value(json!({"provider": 7})).unwrap_or(DecideConfig {
                provider: "off".into(),
                ..Default::default()
            });
        assert_eq!(cfg.provider, "off");
        assert!(!configured(&cfg));
    }

    #[test]
    fn the_timeout_clamps_and_an_absent_or_nonsense_value_takes_the_default() {
        let d = |t: Option<i64>| {
            DecideConfig {
                provider: "jev".into(),
                timeout_ms: t,
                ..Default::default()
            }
            .timeout()
        };
        assert_eq!(d(None), DEFAULT_TIMEOUT_MS);
        assert_eq!(d(Some(0)), DEFAULT_TIMEOUT_MS, "zero is not a budget");
        assert_eq!(d(Some(-5)), DEFAULT_TIMEOUT_MS);
        assert_eq!(d(Some(10)), 250, "clamped up to the floor");
        assert_eq!(d(Some(900)), 900);
        assert_eq!(d(Some(999_999)), 60_000, "clamped down to the ceiling");
    }
}
