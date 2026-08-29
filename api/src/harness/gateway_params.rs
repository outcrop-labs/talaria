// The gateway's parameter learner, reduced to the parts that must be provable
// — the port of harness/gateway-params.ts.
//
// `llm-gateway` learns which parameters an upstream refuses by reading its
// 400s ("`temperature` is deprecated", "Unsupported parameter: 'top_p'"),
// stripping the named parameter, retrying, and remembering the answer per
// endpoint:model. That mechanism is right and stays: model specs rot,
// providers are always current, and no hand-maintained support table has ever
// kept up with a vendor retiring a tunable mid-quarter.
//
// AUDIT 1.2 — what was wrong was WHICH parameters it was willing to forget.
// The matcher reached any bare lowercase identifier, so `response_format` was
// as strippable as `top_p`. A model that refuses JSON mode got the constraint
// deleted, the retry SUCCEEDED, and the caller — which had asked for JSON
// precisely because it was about to run a JSON parser — received free prose
// with no signal that anything had changed. The one mode where the harness
// KNEW it was in structured-output mode silently became the mode where it
// wasn't. That is the most damaging silent failure in the harness layer, and
// it is a classification bug:
//
//   cosmetic  — removing it changes how good the answer is. Strip and forget
//               about it; this is the behavior that has always worked.
//   contract  — removing it changes the SHAPE of what comes back. Still strip,
//               because a completed call beats a 400, but never in silence: the
//               drop is recorded as a capability fact and reported to the
//               caller, which then knows to take a repair path instead of
//               feeding prose to a parser.
//   protected — never strip at all. Removing it doesn't degrade the call, it
//               makes the response unreadable by the code waiting for it.
//
// The second half of the finding was that the learnings were FOREVER:
// persisted with no timestamp and no invalidation, so a provider that later
// fixed support was never re-tried. A one-way ratchet on capability. Hence
// the TTL below, and hence this module owning the stored shape — the
// migration off the old timestamp-free format is the fiddly part and it
// deserves tests, not a try/catch in a fetch path.
//
// PURE BY CONSTRUCTION: no database, no gateway, no clock of its own (`now`
// is always an argument). The gateway supplies the I/O.

use indexmap::IndexMap;
use regex::Regex;
use serde_json::Value;
use std::sync::OnceLock;

// ── Classification ───────────────────────────────────────────────────────────

/// Parameters that decide what comes BACK, not how good it is. The enum is
/// closed so the exhaustive `capability` match below fails the build when a
/// member is added — the port of TS's `Record<ContractParam, Capability>`,
/// which exists for exactly that reason: a contract parameter that maps to no
/// capability would be classified contract-bearing and then record nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContractParam {
    ResponseFormat,
    Tools,
    ToolChoice,
    ParallelToolCalls,
    // Legacy OpenAI tool spelling. Live providers still 400 on these by name
    // ("`functions` is deprecated, use `tools`"), and that 400 says exactly as
    // much about tool support as the modern spelling does.
    Functions,
    FunctionCall,
    // vLLM / SGLang / llama.cpp structured-output extensions. These matter
    // more here than the OpenAI spelling does: they are how a SELF-HOSTED 14B
    // model is actually constrained to a schema, which is the deployment this
    // whole audit is about. A local server that rejects one of them has told
    // us it cannot do guided decoding, and that is a `json` fact.
    GuidedJson,
    GuidedRegex,
    GuidedChoice,
    ResponseSchema,
}

impl ContractParam {
    /// The wire spelling, which is what request bodies and 400 messages carry.
    pub fn as_str(self) -> &'static str {
        match self {
            ContractParam::ResponseFormat => "response_format",
            ContractParam::Tools => "tools",
            ContractParam::ToolChoice => "tool_choice",
            ContractParam::ParallelToolCalls => "parallel_tool_calls",
            ContractParam::Functions => "functions",
            ContractParam::FunctionCall => "function_call",
            ContractParam::GuidedJson => "guided_json",
            ContractParam::GuidedRegex => "guided_regex",
            ContractParam::GuidedChoice => "guided_choice",
            ContractParam::ResponseSchema => "response_schema",
        }
    }

    /// Which capability a rejection of this parameter proves absent.
    /// Exhaustive on purpose (see the enum's doc).
    pub fn capability(self) -> &'static str {
        match self {
            ContractParam::ResponseFormat
            | ContractParam::GuidedJson
            | ContractParam::GuidedRegex
            | ContractParam::GuidedChoice
            | ContractParam::ResponseSchema => "json",
            ContractParam::Tools
            | ContractParam::ToolChoice
            | ContractParam::ParallelToolCalls
            | ContractParam::Functions
            | ContractParam::FunctionCall => "tools",
        }
    }

    fn parse(param: &str) -> Option<Self> {
        Some(match param {
            "response_format" => ContractParam::ResponseFormat,
            "tools" => ContractParam::Tools,
            "tool_choice" => ContractParam::ToolChoice,
            "parallel_tool_calls" => ContractParam::ParallelToolCalls,
            "functions" => ContractParam::Functions,
            "function_call" => ContractParam::FunctionCall,
            "guided_json" => ContractParam::GuidedJson,
            "guided_regex" => ContractParam::GuidedRegex,
            "guided_choice" => ContractParam::GuidedChoice,
            "response_schema" => ContractParam::ResponseSchema,
            _ => return None,
        })
    }
}

pub fn is_contract_param(param: &str) -> bool {
    ContractParam::parse(param).is_some()
}

/// Parameters the learner may never remove, whatever the upstream says.
///
/// `model` and `messages` were already refused by hand in the fetch —
/// stripping either turns the request into nonsense. `stream` joins them for
/// a subtler reason: dropping it produces a PERFECTLY VALID single JSON body,
/// which the caller then hands to an SSE pump that will sit there reading a
/// stream that never arrives. A 400 relayed honestly is a far better outcome
/// than a hang, and unlike `response_format` there is no repair path to take —
/// the caller asked for a transport it cannot get.
const PROTECTED_PARAMS: [&str; 3] = ["model", "messages", "stream"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamClass {
    Cosmetic,
    Contract,
    Protected,
}

/// Everything unrecognized classifies as cosmetic, deliberately.
///
/// This preserves the behavior that has worked for every tunable since the
/// learner shipped (`temperature`, `top_p`, `top_k`, `frequency_penalty`,
/// `presence_penalty`, `seed`, `stop`, `logprobs`, `min_p`, and whatever the
/// next vendor invents), and the blast radius of being wrong is bounded: an
/// unrecognized parameter that was actually load-bearing degrades quality,
/// where the contract list above is the set whose removal changes the TYPE of
/// the response. If a new structured-output or tool-calling parameter appears,
/// add it to `ContractParam` — that is the one edit this file asks for.
pub fn classify_param(param: &str) -> ParamClass {
    if PROTECTED_PARAMS.contains(&param) {
        return ParamClass::Protected;
    }
    if is_contract_param(param) {
        return ParamClass::Contract;
    }
    ParamClass::Cosmetic
}

// ── Reading the rejection out of a 400 ───────────────────────────────────────

fn param_field() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r#"(?i)"param"\s*:\s*"([a-z_]+)(?:\.[A-Za-z0-9_]+)*""#).unwrap())
}

/// The prose patterns, in the exact order the TS file chains them — a `??`
/// chain, so the first match wins and the order IS the precedence.
fn prose_patterns() -> &'static [Regex] {
    static RS: OnceLock<Vec<Regex>> = OnceLock::new();
    RS.get_or_init(|| {
        [
            // The three phrasings the learner shipped against: Anthropic's
            // OpenAI-compat layer ("`temperature` is deprecated"), OpenAI's
            // own ("Unsupported parameter: 'top_p'"), and the loose middle
            // ground everyone else writes ("'seed' is not supported").
            r#"(?i)[`"']([a-z_]+)[`"'] is (?:deprecated|not supported|unsupported)"#,
            r#"(?i)unsupported parameter[:\s`"']+([a-z_]+)"#,
            // OPENAI'S OTHER TWO PHRASINGS, and they are not cosmetic
            // variants — a parameter the ratchet cannot NAME is one it never
            // stops sending, so the endpoint 400s on every call for as long
            // as the default is configured. `reasoning` is the one that
            // reaches us in practice: a legitimate OpenRouter request
            // default, forwarded to an OpenAI endpoint that refuses it.
            r#"(?i)unrecognized request argument supplied[:\s`"']+([a-z_]+)"#,
            r#"(?i)unknown parameter[:\s`"']+([a-z_]+)"#,
            // THE VALUE COMPLAINT, which is a PARAMETER complaint wearing
            // different words — "Unsupported value: 'temperature' does not
            // support 0.2 with this model." Note "does not support", not
            // "is not supported": the patterns above look for the passive
            // form and miss this entirely. Dropping it is right: the model
            // then runs at its default, which is the only value it has.
            r#"(?i)unsupported value[:\s]*[`"']([a-z_]+)[`"']"#,
            r#"(?i)[`"']([a-z_]+)[`"'][^.]{0,40}(?:deprecated|not supported)"#,
            // THE FIELD-PATH SHAPE, which is how a provider that validates
            // the request body reports a field it will not accept:
            //
            //   response_format.type: Input should be 'json_schema'
            //   response_format.json_schema.strict: Input should be True
            //   response_format.json_schema.schema: Empty schema ({}) ...
            //   messages.0.content: Field required
            //
            // All from a live sweep, none matched by anything above, so the
            // strip-and-retry never fired and the fitness suite scored the
            // 400 as the model failing its contract. Matching the ROOT of
            // the path is what matters: the parameter Talaria can stop
            // sending is `response_format`, not
            // `response_format.json_schema.strict`; `classify_param` still
            // refuses a protected one, so a complaint about
            // `messages.0.content` cannot turn into a request with no
            // messages.
            r#"(?i)(?:^|[\s"'{,])([a-z_]{3,})(?:\.[A-Za-z0-9_]+)*\s*:\s*(?:Input should be|Extra inputs are not permitted|Empty schema|Field required)"#,
        ]
        .iter()
        .map(|p| Regex::new(p).unwrap())
        .collect()
    })
}

/// The parameter an upstream 400 is complaining about, or None.
///
/// Kept narrow on purpose — a pattern that matched more would start stripping
/// parameters that were merely MENTIONED in an error about something else.
pub fn rejected_param(err_text: &str) -> Option<String> {
    // THE PROVIDER'S OWN ANSWER, BEFORE ANY PROSE. OpenAI-compatible errors
    // carry `"param": "<name>"` beside the message, and it is authoritative
    // where every pattern below is a guess at English:
    //
    //   {"error":{"message":"Function tools with reasoning_effort are not
    //   supported for gpt-5.6-terra ... or set reasoning_effort to 'none'.",
    //   "type":"invalid_request_error","param":"reasoning_effort"}}
    //
    // That message names the parameter three times and quotes it NONE of
    // them, so every prose pattern misses it — and the run it came from filed
    // 28 cases as "could not reach this model" across four harnesses, on an
    // endpoint that was answering fine.
    if let Some(caps) = param_field().captures(err_text) {
        return Some(caps[1].to_string());
    }
    prose_patterns()
        .iter()
        .find_map(|re| re.captures(err_text))
        .map(|caps| caps[1].to_string())
}

// ── The persisted store ──────────────────────────────────────────────────────

/// How long a learned strip survives without being re-confirmed by a fresh
/// 400. Deliberately the same 30 days as the capability store's TTL: the two
/// stores record the same event from two angles (what we stopped sending, and
/// what that told us about the model), and a strip that outlived its
/// capability fact would keep the contract quietly dropped while the admin UI
/// reported the model as unknown-but-fine.
pub const LEARNED_PARAM_TTL_MS: i64 = 30 * 24 * 60 * 60 * 1000;

/// endpoint:model -> parameter -> epoch ms it was learned. Insertion-ordered
/// like the JS Maps it ports, so the stored JSON's key order is stable and a
/// write never churns bytes it did not mean to touch.
pub type LearnedParamMap = IndexMap<String, IndexMap<String, i64>>;

/// The read normalized something — a legacy timestamp-free entry, an expired
/// one, or a corrupt one — and the caller should write the result back ONCE.
///
/// This matters most for the legacy shape. A timestamp-free entry is stamped
/// fresh-from-now, which is the only honest reading (we know the strip was
/// learned at some point, we cannot know when, and guessing "long ago" would
/// throw away good learnings while guessing "never expires" reinstates the
/// exact ratchet the TTL removes). But if that stamp is only ever held in
/// memory, a process that restarts weekly restamps it weekly and the entry
/// becomes permanent anyway. Persisting on first read is what actually starts
/// the clock.
pub struct ReadLearnedParams {
    pub by_key: LearnedParamMap,
    pub changed: bool,
}

/// Parse the stored value, tolerating both shapes and any hand-edited garbage.
///
/// `app_settings` is JSON that outlives the code that wrote it, and this runs
/// on the path to every upstream call — anything unrecognized is dropped
/// rather than thrown, which costs at most one re-learned 400. (The one
/// deliberate narrowing: a timestamp must be the `toISOString()` shape this
/// system writes. TS's `Date.parse` also accepts date-only and offset forms a
/// hand-edit might produce; both land on "corrupt" here, which is the
/// module's own stated posture toward a writer that got the format wrong.)
pub fn read_learned_params(raw: &Value, now: i64) -> ReadLearnedParams {
    let mut by_key = LearnedParamMap::new();
    let mut changed = false;
    let Some(obj) = raw.as_object() else {
        return ReadLearnedParams { by_key, changed };
    };

    for (key, entry) in obj {
        let mut params = IndexMap::new();

        if let Some(list) = entry.as_array() {
            // Legacy: a bare list of parameter names, no timestamps. Adopt
            // them all at `now` so they keep working today and become
            // expirable tomorrow.
            for p in list.iter().filter_map(|p| p.as_str()) {
                if !p.is_empty() {
                    params.insert(p.to_string(), now);
                }
            }
            changed = true;
        } else if let Some(map) = entry.as_object() {
            for (param, at) in map {
                let Some(ms) = at.as_str().and_then(crate::agent_auth::iso_to_epoch_ms) else {
                    // A corrupt timestamp is NOT the legacy shape — the
                    // legacy shape is an array, handled above and
                    // recognizable as such. This is an entry written by
                    // something that got the format wrong, and re-stamping it
                    // fresh would grant an unknown writer a permanent strip.
                    // Drop it; the next 400 re-learns it in the shape we
                    // understand.
                    changed = true;
                    continue;
                };
                if now - ms > LEARNED_PARAM_TTL_MS {
                    changed = true;
                    continue;
                }
                params.insert(param.clone(), ms);
            }
        } else {
            changed = true;
            continue;
        }

        if !params.is_empty() {
            by_key.insert(key.clone(), params);
        } else {
            changed = true;
        }
    }
    ReadLearnedParams { by_key, changed }
}

/// Serialize for `app_settings`. Expired entries are dropped on the way out,
/// so a store that is written for any reason is also a store that is pruned.
pub fn write_learned_params(by_key: &LearnedParamMap, now: i64) -> Value {
    let mut out = serde_json::Map::new();
    for (key, params) in by_key {
        let mut entry = serde_json::Map::new();
        for (param, at) in params {
            if now - at > LEARNED_PARAM_TTL_MS {
                continue;
            }
            entry.insert(
                param.clone(),
                Value::String(crate::agent_auth::epoch_ms_to_iso(*at)),
            );
        }
        if !entry.is_empty() {
            out.insert(key.clone(), Value::Object(entry));
        }
    }
    Value::Object(out)
}

/// The survivors of one key's pre-strip list. Mutates `by_key` — an expired
/// learning is deleted, not merely ignored, so the next persist stops carrying
/// it and the next 400 is allowed to re-learn it from a provider that may
/// since have fixed support.
///
/// Returns the survivors plus whether anything was forgotten, because the
/// caller owns persistence and must not write on every single call.
pub struct ActiveLearnedParams {
    pub params: Vec<String>,
    pub expired: bool,
}

pub fn active_learned_params(
    by_key: &mut LearnedParamMap,
    key: &str,
    now: i64,
) -> ActiveLearnedParams {
    let Some(entry) = by_key.get_mut(key) else {
        return ActiveLearnedParams {
            params: Vec::new(),
            expired: false,
        };
    };
    let before = entry.len();
    // `now - at` in the expired sense: a FUTURE stamp (clock skew across a
    // restart) reads as negative and stays.
    entry.retain(|_, &mut at| now.saturating_sub(at) <= LEARNED_PARAM_TTL_MS);
    let expired = before != entry.len();
    let params: Vec<String> = entry.keys().cloned().collect();
    if entry.is_empty() {
        by_key.shift_remove(key);
    }
    ActiveLearnedParams { params, expired }
}

// ── The signal a dropped contract parameter sends to the caller ──────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropSource {
    /// The upstream 400'd on this very call and the gateway retried without
    /// the parameter.
    Rejected,
    /// An earlier 400 taught us, and the request builder never sent it. The
    /// caller treats the two identically; observability doesn't.
    Remembered,
}

/// One contract-bearing parameter that did NOT reach the model.
///
/// Handed back on the upstream call so a caller can tell "the model answered
/// my structured request" from "the model answered a request I no longer
/// made". Both look identical in the response body, which is the whole reason
/// this type exists.
#[derive(Debug, Clone)]
pub struct ContractDrop {
    pub param: ContractParam,
    /// The capability the drop proves absent — "json" or "tools".
    pub capability: &'static str,
    pub endpoint: String,
    /// The upstream model id, as `capability_key` spells it.
    pub model: String,
    pub source: DropSource,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const KEY: &str = "pl-main:qwen3-14b";
    const NOW: i64 = 1_786_008_000_000; // 2026-08-06T12:00:00.000Z
    const DAY: i64 = 24 * 60 * 60 * 1000;

    fn iso(ms_ago: i64) -> String {
        crate::agent_auth::epoch_ms_to_iso(NOW - ms_ago)
    }

    fn map(entries: &[(&str, &[(&str, i64)])]) -> LearnedParamMap {
        entries
            .iter()
            .map(|(k, v)| {
                (
                    (*k).to_string(),
                    v.iter().map(|(p, at)| ((*p).to_string(), *at)).collect(),
                )
            })
            .collect()
    }

    // ── Classification ───────────────────────────────────────────────────────

    #[test]
    fn treats_tunables_as_cosmetic_silently_strippable_as_they_always_were() {
        for p in [
            "temperature",
            "top_p",
            "top_k",
            "frequency_penalty",
            "presence_penalty",
            "seed",
            "stop",
        ] {
            assert_eq!(classify_param(p), ParamClass::Cosmetic, "{p}");
        }
        // The default matters: it is what preserves today's behavior for
        // every tunable no vendor has invented yet. Being wrong here costs
        // quality; being wrong in the other direction would turn a
        // strippable 400 into a hard failure on paths that work fine today.
        assert_eq!(classify_param("min_p"), ParamClass::Cosmetic);
        assert_eq!(classify_param("repetition_penalty"), ParamClass::Cosmetic);
    }

    #[test]
    fn treats_structured_output_and_tool_parameters_as_contract_bearing() {
        // AUDIT 1.2, the whole point: stripping response_format turns a
        // structured request into a prose request that SUCCEEDS, and the
        // caller hands prose to a JSON parser.
        for p in [
            ContractParam::ResponseFormat,
            ContractParam::Tools,
            ContractParam::ToolChoice,
            ContractParam::ParallelToolCalls,
            // Guided decoding on a self-hosted server is the same contract by
            // another name, and it is the deployment small-model support is
            // FOR.
            ContractParam::GuidedJson,
        ] {
            assert_eq!(classify_param(p.as_str()), ParamClass::Contract);
        }
    }

    #[test]
    fn refuses_to_strip_the_parameters_that_make_a_response_readable_at_all() {
        // model/messages were already hand-refused. `stream` joins them:
        // dropping it yields a valid single JSON body that an SSE pump will
        // wait on forever.
        for p in ["model", "messages", "stream"] {
            assert_eq!(classify_param(p), ParamClass::Protected, "{p}");
        }
    }

    #[test]
    fn maps_every_contract_parameter_to_the_capability_its_rejection_disproves() {
        assert_eq!(ContractParam::ResponseFormat.capability(), "json");
        assert_eq!(ContractParam::GuidedRegex.capability(), "json");
        assert_eq!(ContractParam::ResponseSchema.capability(), "json");
        assert_eq!(ContractParam::Tools.capability(), "tools");
        assert_eq!(ContractParam::ToolChoice.capability(), "tools");
        assert_eq!(ContractParam::ParallelToolCalls.capability(), "tools");
        assert_eq!(ContractParam::Functions.capability(), "tools");
        assert_eq!(ContractParam::FunctionCall.capability(), "tools");
        assert!(!is_contract_param("temperature"));
    }

    // ── Reading the 400 ──────────────────────────────────────────────────────

    #[test]
    fn still_matches_the_three_real_world_400_phrasings_it_shipped_against() {
        // These are the regressions that would silently disable the learner:
        // the call would keep 400ing forever with nothing in the logs saying
        // why.
        assert_eq!(
            rejected_param(r#"{"error":{"message":"`temperature` is deprecated for this model"}}"#),
            Some("temperature".into())
        );
        assert_eq!(
            rejected_param(r#"{"error":{"message":"Unsupported parameter: 'top_p'"}}"#),
            Some("top_p".into())
        );
        assert_eq!(
            rejected_param(r#"{"error":{"message":"'seed' is not supported by this model"}}"#),
            Some("seed".into())
        );
    }

    #[test]
    fn prefers_the_providers_own_param_field_over_any_reading_of_its_prose() {
        // VERBATIM FROM THE RUN THAT FOUND IT. The message names
        // `reasoning_effort` three times and quotes it none of them, so every
        // prose pattern in the function misses it — and 28 cases across four
        // harnesses were filed as "could not reach this model" on an endpoint
        // that was answering fine.
        let body = json!({
            "error": {
                "message": "Function tools with reasoning_effort are not supported for gpt-5.6-terra in /v1/chat/completions. To use function tools, use /v1/responses or set reasoning_effort to 'none'.",
                "type": "invalid_request_error",
                "param": "reasoning_effort",
                "code": null,
            }
        });
        assert_eq!(
            rejected_param(&body.to_string()),
            Some("reasoning_effort".into())
        );
        assert_eq!(classify_param("reasoning_effort"), ParamClass::Cosmetic);
    }

    #[test]
    fn reduces_a_dotted_param_path_to_the_thing_we_can_stop_sending() {
        assert_eq!(
            rejected_param(r#"{"error":{"param":"response_format.json_schema.strict"}}"#),
            Some("response_format".into())
        );
    }

    #[test]
    fn still_refuses_to_strip_a_load_bearing_parameter_the_provider_names() {
        assert_eq!(
            classify_param(
                rejected_param(r#"{"error":{"param":"messages"}}"#)
                    .as_deref()
                    .unwrap_or("")
            ),
            ParamClass::Protected
        );
    }

    #[test]
    fn names_the_parameter_in_openais_two_unquoted_phrasings() {
        // A PARAMETER THE RATCHET CANNOT NAME IS ONE IT NEVER STOPS SENDING,
        // so the endpoint 400s on every call for as long as the default is
        // configured. `reasoning` is the one that reaches us: a legitimate
        // OpenRouter request default, forwarded to an OpenAI endpoint that
        // refuses it.
        assert_eq!(
            rejected_param("Unrecognized request argument supplied: reasoning"),
            Some("reasoning".into())
        );
        assert_eq!(
            rejected_param("Unknown parameter: 'reasoning'."),
            Some("reasoning".into())
        );
        // And the quoted-dotted shape OpenAI uses for nested ones, which the
        // existing pattern already reaches — asserted so a rewrite cannot
        // lose it.
        assert_eq!(
            rejected_param(
                "Unsupported parameter: 'reasoning.effort' is not supported with this model."
            ),
            Some("reasoning".into())
        );
    }

    #[test]
    fn reads_a_value_complaint_as_the_parameter_complaint_it_is() {
        // OpenAI's reasoning models phrase it "does not support", not "is not
        // supported", so every pattern written for the passive form missed
        // it — `temperature` was never learned and every harness that
        // declares one 400'd on that endpoint for ever.
        assert_eq!(
            rejected_param(
                "Unsupported value: 'temperature' does not support 0.2 with this model. Only the default (1) is supported."
            ),
            Some("temperature".into())
        );
        assert_eq!(classify_param("temperature"), ParamClass::Cosmetic);
    }

    #[test]
    fn will_not_let_a_widened_pattern_strip_something_load_bearing() {
        // The patterns are read by `classify_param`, which refuses to remove
        // `model`, `messages` or `stream` however the upstream phrases its
        // complaint. Stated here because the loosest patterns in the file are
        // the ones most likely to widen in a rewrite.
        assert_eq!(
            classify_param(
                rejected_param("Unrecognized request argument supplied: messages")
                    .as_deref()
                    .unwrap_or("")
            ),
            ParamClass::Protected
        );
        assert_eq!(
            classify_param(
                rejected_param("Unknown parameter: 'stream'.")
                    .as_deref()
                    .unwrap_or("")
            ),
            ParamClass::Protected
        );
    }

    #[test]
    fn reads_response_format_out_of_a_rejection_so_it_can_be_classified_not_stripped_blind() {
        assert_eq!(
            rejected_param(r#"{"error":{"message":"`response_format` is not supported"}}"#),
            Some("response_format".into())
        );
    }

    #[test]
    fn stays_none_on_a_400_about_something_else() {
        assert_eq!(
            rejected_param(r#"{"error":{"message":"context length exceeded"}}"#),
            None
        );
        assert_eq!(rejected_param(""), None);
    }

    // ── A provider that reports a field path instead of a quoted name ────────

    #[test]
    fn names_the_root_parameter_from_a_field_path_report() {
        // Every one of these is a real Anthropic 400 from a live sweep, and
        // not one of them matched the original patterns — so
        // `response_format` was never stripped, the call was never retried,
        // and the fitness suite scored the 400 as the MODEL failing its
        // contract on every structured harness.
        let real = [
            r#"{"error":{"code":"invalid_request_error","message":"response_format.type: Input should be 'json_schema'","type":"invalid_request_error"}}"#,
            r#"{"error":{"message":"response_format.json_schema.strict: Input should be True"}}"#,
            r#"{"error":{"message":"response_format.json_schema.schema: Empty schema ({}) that accepts any JSON value is not supported. Please specify a concrete type."}}"#,
        ];
        for text in real {
            assert_eq!(
                rejected_param(text),
                Some("response_format".into()),
                "{}",
                &text[..60.min(text.len())]
            );
        }
    }

    #[test]
    fn still_refuses_to_strip_a_protected_parameter_reported_as_a_field_path() {
        // A complaint about the message list must never become a request with
        // no messages. `classify_param` is what holds that, and this is the
        // shape that would have reached it.
        assert_eq!(
            classify_param(
                rejected_param(r#"{"error":{"message":"messages.0.content: Field required"}}"#)
                    .as_deref()
                    .unwrap_or("")
            ),
            ParamClass::Protected
        );
    }

    #[test]
    fn does_not_fire_on_ordinary_prose_that_happens_to_contain_a_colon() {
        assert_eq!(
            rejected_param("Rate limited: please retry after 20 seconds"),
            None
        );
        assert_eq!(
            rejected_param(r#"{"error":{"message":"Internal server error"}}"#),
            None
        );
    }

    // ── The persisted store ──────────────────────────────────────────────────

    #[test]
    fn reads_the_current_shape_and_keeps_entries_inside_the_ttl() {
        let read = read_learned_params(&json!({ KEY: { "temperature": iso(2 * DAY) } }), NOW);
        let entry = read.by_key.get(KEY).unwrap();
        assert_eq!(
            entry.keys().collect::<Vec<_>>(),
            [&"temperature".to_string()]
        );
        assert_eq!(entry.get("temperature"), Some(&(NOW - 2 * DAY)));
        // Nothing normalized — don't churn the settings row.
        assert!(!read.changed);
    }

    #[test]
    fn drops_an_entry_past_the_thirty_day_ttl() {
        // The release valve on the ratchet: a provider that fixed support
        // gets re-tried without an admin ever touching anything.
        let read = read_learned_params(
            &json!({ KEY: { "response_format": iso(LEARNED_PARAM_TTL_MS + 1000) } }),
            NOW,
        );
        assert!(!read.by_key.contains_key(KEY));
        assert!(read.changed);
    }

    #[test]
    fn keeps_an_entry_one_minute_short_of_the_ttl() {
        let read = read_learned_params(
            &json!({ KEY: { "top_p": iso(LEARNED_PARAM_TTL_MS - 60_000) } }),
            NOW,
        );
        assert!(read.by_key.get(KEY).unwrap().contains_key("top_p"));
    }

    #[test]
    fn adopts_a_legacy_un_timestamped_entry_as_fresh_from_now_and_asks_to_be_persisted() {
        // Every existing install stores Record<string, string[]>. Reading it
        // as expired would throw away real learnings and re-pay every 400;
        // reading it as permanent would reinstate the ratchet.
        // Fresh-from-now is the only honest reading — and `changed` is how
        // the clock actually starts, because a stamp that only lives in
        // memory restarts with the process.
        let read = read_learned_params(&json!({ KEY: ["temperature", "top_p"] }), NOW);
        let entry = read.by_key.get(KEY).unwrap();
        assert_eq!(
            entry.keys().collect::<Vec<_>>(),
            [&"temperature".to_string(), &"top_p".to_string()]
        );
        assert_eq!(entry.get("temperature"), Some(&NOW));
        assert!(read.changed);

        // And once persisted it is timestamped, so the NEXT read can expire it.
        let stored = write_learned_params(&read.by_key, NOW);
        assert_eq!(
            stored.get(KEY).unwrap().get("temperature"),
            Some(&json!(crate::agent_auth::epoch_ms_to_iso(NOW)))
        );
        let later = read_learned_params(&stored, NOW + LEARNED_PARAM_TTL_MS + 1000);
        assert!(!later.by_key.contains_key(KEY));
    }

    #[test]
    fn drops_a_corrupt_timestamp_rather_than_granting_it_a_fresh_thirty_days() {
        // Distinct from the legacy shape, which is an array and recognizable
        // as one. This is a writer that got the format wrong, and re-stamping
        // it would hand an unknown writer a permanent strip.
        let read = read_learned_params(
            &json!({ KEY: { "temperature": "yesterday", "top_p": iso(DAY) } }),
            NOW,
        );
        assert_eq!(
            read.by_key.get(KEY).unwrap().keys().collect::<Vec<_>>(),
            [&"top_p".to_string()]
        );
        assert!(read.changed);
    }

    #[test]
    fn survives_anything_at_all_in_the_settings_row() {
        // app_settings is JSON that outlives the code that wrote it, and this
        // runs on the path to every upstream call.
        for raw in [
            json!(null),
            json!("nonsense"),
            json!([1, 2, 3]),
            json!({ KEY: 42 }),
            json!({ KEY: {} }),
        ] {
            assert!(read_learned_params(&raw, NOW).by_key.is_empty(), "{raw}");
        }
    }

    #[test]
    fn write_prunes_expired_entries_so_any_write_is_also_a_cleanup() {
        let by_key = map(&[
            (
                KEY,
                &[
                    ("temperature", NOW - DAY),
                    ("response_format", NOW - LEARNED_PARAM_TTL_MS - 1),
                ][..],
            ),
            (
                "openrouter:qwen3-14b",
                &[("top_p", NOW - LEARNED_PARAM_TTL_MS - 1)][..],
            ),
        ]);
        let stored = write_learned_params(&by_key, NOW);
        assert_eq!(
            stored.as_object().unwrap().keys().collect::<Vec<_>>(),
            [&KEY.to_string()]
        );
        assert_eq!(
            stored
                .get(KEY)
                .unwrap()
                .as_object()
                .unwrap()
                .keys()
                .collect::<Vec<_>>(),
            [&"temperature".to_string()]
        );
    }

    #[test]
    fn active_returns_the_survivors_and_forgets_the_expired_in_place() {
        let mut by_key = map(&[(
            KEY,
            &[
                ("temperature", NOW - DAY),
                ("response_format", NOW - LEARNED_PARAM_TTL_MS - 1),
            ][..],
        )]);
        let active = active_learned_params(&mut by_key, KEY, NOW);
        assert_eq!(active.params, ["temperature".to_string()]);
        assert!(active.expired);
        // Deleted, not merely filtered: the next 400 is allowed to re-learn
        // it from a provider that may since have fixed support.
        assert!(!by_key.get(KEY).unwrap().contains_key("response_format"));
    }

    #[test]
    fn drops_the_key_entirely_once_its_last_learning_expires() {
        let mut by_key = map(&[(
            KEY,
            &[("response_format", NOW - LEARNED_PARAM_TTL_MS - 1)][..],
        )]);
        assert!(
            active_learned_params(&mut by_key, KEY, NOW)
                .params
                .is_empty()
        );
        assert!(!by_key.contains_key(KEY));
    }

    #[test]
    fn reports_no_expiry_for_an_untouched_or_unknown_key_so_the_caller_does_not_churn() {
        let mut by_key = map(&[(KEY, &[("temperature", NOW - DAY)][..])]);
        let active = active_learned_params(&mut by_key, KEY, NOW);
        assert_eq!(active.params, ["temperature".to_string()]);
        assert!(!active.expired);
        let other = active_learned_params(&mut by_key, "other:model", NOW);
        assert!(other.params.is_empty());
        assert!(!other.expired);
    }

    #[test]
    fn keeps_learnings_for_one_endpoint_model_out_of_another() {
        // Same model id behind a quantized local build and the vendor's own
        // API genuinely differ in what they accept; a fact from one must
        // never be credited to the other.
        let mut by_key = map(&[(KEY, &[("temperature", NOW - DAY)][..])]);
        assert!(
            active_learned_params(&mut by_key, "openrouter:qwen3-14b", NOW)
                .params
                .is_empty()
        );
    }

    #[test]
    fn a_contract_drop_carries_the_capability_its_rejection_disproves() {
        // The shape a caller reads off an upstream call to tell "the model
        // answered my structured request" from "the model answered a request
        // I no longer made" — identical in the response body, different in
        // what the caller should do next.
        let drop = ContractDrop {
            param: ContractParam::ResponseFormat,
            capability: ContractParam::ResponseFormat.capability(),
            endpoint: "pl-main".into(),
            model: "qwen3-14b".into(),
            source: DropSource::Rejected,
        };
        assert_eq!(drop.capability, "json");
        assert_eq!(drop.param.as_str(), "response_format");
    }
}
