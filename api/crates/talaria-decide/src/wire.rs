// THE THREE WIRE SHAPES — one pure builder and one pure reader each, so the
// envelope a service actually speaks is pinned by a test rather than by
// whichever dispatch branch was copied last. That discipline is the rerank
// crate's and it is here for the same reason: these are other people's
// protocols, and the only honest record of what they return is a recorded
// reply with an assertion next to it.
//
//   systemone  POST {base}/v1/systemone   state + named questions → named
//              answers. The only wire that answers several questions in one
//              round trip, and the only one whose probabilities the model
//              itself was trained to calibrate.
//   predict    POST {base}/predict        a classification head. One forward
//              pass per question; a Choice batches its options into one call.
//              Probabilities are a real softmax, over labels rather than over
//              our question.
//   chat       POST {base}/chat/completions   an ordinary text model, pinned
//              to a one-token answer and read through `logprobs`.
//
// WHY THE CHAT WIRE ASKS FOR ONE TOKEN AND NOT FOR JSON. The obvious design —
// `response_format` with a schema, parse the object — gets an answer and no
// number: the probabilities that matter are buried among the logprobs of `{`,
// `"`, `verdict` and the rest of the syntax, and picking the answer's token
// out of that stream means reimplementing the model's own tokenizer against
// every provider's slightly different streaming shape. Asking instead for
// exactly one word, letter or digit makes `content[0]` THE answer, so its
// `top_logprobs` are the distribution, directly. A model that ignores the
// instruction and writes prose simply fails to match and the judgment is
// dropped, which is the correct outcome.
//
// AND WHEN THERE ARE NO LOGPROBS the answer still comes back — read off the
// text — with `calibrated: false`. That is the whole reason `calibrated` is a
// field: an endpoint that does not serve logprobs is common, its answers are
// still useful, and the one thing we must never do is invent a confidence for
// them.

use std::collections::BTreeMap;

use serde_json::{Value, json};
use talaria_retrieval_http::HttpFetch;
use talaria_state::AppState;

use crate::config::{
    DecideConfig, DecideProviderMeta, WIRE_CHAT, WIRE_PREDICT, WIRE_SYSTEMONE, configured,
};
use crate::{Answer, Ask, Judgment, Opt, Question};

/// TypeSafe's documented base. Only the `jev` provider uses it; `custom`
/// speaking the same wire brings its own URL.
const JEV_BASE: &str = "https://api.typesafe.ai";

/// The single-token alphabet the chat wire offers a Choice. 26 options is well
/// past what any call site in this repo asks, and a letter is one token in
/// every tokenizer we are likely to meet — which is the whole requirement.
const LETTERS: [&str; 26] = [
    "A", "B", "C", "D", "E", "F", "G", "H", "I", "J", "K", "L", "M", "N", "O", "P", "Q", "R", "S",
    "T", "U", "V", "W", "X", "Y", "Z",
];

// ── Dispatch ─────────────────────────────────────────────────────────────────

/// One graded answer plus whether a real distribution stood behind it.
type Read = (Answer, bool);

pub(crate) async fn dispatch(
    state: &AppState,
    http: &HttpFetch,
    cfg: &DecideConfig,
    meta: &'static DecideProviderMeta,
    ask: &Ask,
) -> Option<BTreeMap<String, Judgment>> {
    if !configured(cfg) {
        return None;
    }
    let wire = cfg.wire_of(meta)?;
    let started = std::time::Instant::now();
    let (base, key, model) = target(state, cfg, meta).await?;
    let timeout = cfg.timeout();

    let reads: BTreeMap<String, Read> = match wire {
        WIRE_SYSTEMONE => {
            let url = format!("{}/v1/systemone", base.trim_end_matches('/'));
            let body = systemone_request(model.as_deref(), ask);
            let reply = json_fetch(http, &url, &bearer(key.as_deref()), &body, timeout).await?;
            systemone_answers(&reply, ask)
        }
        WIRE_PREDICT => {
            // `fans_out: false` holds this to one question; `decide` already
            // refused anything more.
            let (id, question) = ask.questions.iter().next()?;
            let url = format!("{}/predict", base.trim_end_matches('/'));
            let body = predict_request(&ask.state, question)?;
            let reply = json_fetch(http, &url, &bearer(key.as_deref()), &body, timeout).await?;
            predict_answer(&reply, question)
                .map(|r| BTreeMap::from([(id.clone(), r)]))
                .unwrap_or_default()
        }
        WIRE_CHAT => {
            let url = format!("{}/chat/completions", base.trim_end_matches('/'));
            let mut out = BTreeMap::new();
            // A chat model answers ONE question per call even where the
            // provider `fans_out`: the trick that makes the number real is
            // that the first content token is the whole answer, and two
            // answers cannot both be the first token. Fanning out here means
            // several calls, which is honest about what it costs.
            for (id, question) in &ask.questions {
                let body = chat_request(model.as_deref(), &ask.state, question);
                let Some(reply) =
                    json_fetch(http, &url, &bearer(key.as_deref()), &body, timeout).await
                else {
                    continue;
                };
                if let Some(r) = chat_answer(&reply, question) {
                    out.insert(id.clone(), r);
                }
            }
            out
        }
        _ => return None,
    };

    let latency_ms = started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
    Some(
        reads
            .into_iter()
            .map(|(id, (answer, calibrated))| {
                (
                    id,
                    Judgment {
                        answer,
                        calibrated,
                        provider: meta.id.to_string(),
                        model: model.clone(),
                        latency_ms,
                    },
                )
            })
            .collect(),
    )
}

/// Where to send the request, what to authenticate with, and which model to
/// name — resolved per provider, because the three answers come from three
/// different places: a documented base, an operator's URL, or an endpoint row
/// the operator already filled in on /models.
async fn target(
    state: &AppState,
    cfg: &DecideConfig,
    meta: &'static DecideProviderMeta,
) -> Option<(String, Option<String>, Option<String>)> {
    match meta.id {
        "jev" => Some((
            JEV_BASE.to_string(),
            open_key(state, cfg).await,
            cfg.model.clone(),
        )),
        "tei" | "custom" => Some((
            cfg.url.clone()?,
            open_key(state, cfg).await,
            cfg.model.clone(),
        )),
        // REUSE THE ENDPOINT, DO NOT RE-ASK FOR ITS KEY. The operator already
        // registered this model with a sealed key on /models; asking for it a
        // second time here would be a second copy of the same secret to
        // rotate.
        "gateway" => {
            let spec = cfg.model.as_deref()?;
            let (ep_hint, model) = match spec.split_once(':') {
                Some((ep, m)) => (Some(ep), m),
                None => (None, spec),
            };
            let eps = talaria_gateway::registry::list_endpoints(&state.pg)
                .await
                .ok()?;
            let ep = eps.iter().find(|e| match ep_hint {
                Some(h) => e.id == h || e.name == h || e.provider == h,
                None => e.models.iter().any(|m| m == model),
            })?;
            let base = ep.base_url.clone().or_else(|| {
                talaria_gateway::provider::native_base(&ep.provider).map(String::from)
            })?;
            let key = talaria_gateway::provider::resolve_endpoint_key(state, ep).await;
            Some((base, key, Some(model.to_string())))
        }
        _ => None,
    }
}

async fn open_key(state: &AppState, cfg: &DecideConfig) -> Option<String> {
    let sealed = cfg.key_sealed.as_deref()?;
    state.secretbox().await.ok()?.open(sealed).ok()
}

fn bearer(key: Option<&str>) -> Vec<(String, String)> {
    match key {
        Some(k) => vec![("authorization".into(), format!("Bearer {k}"))],
        None => Vec::new(),
    }
}

async fn json_fetch(
    http: &HttpFetch,
    url: &str,
    headers: &[(String, String)],
    body: &Value,
    timeout_ms: u64,
) -> Option<Value> {
    let hdrs: Vec<(&str, &str)> = headers
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    let (status, text) = (http)("POST", url, Some(body), &hdrs, timeout_ms)
        .await
        .ok()?;
    if !(200..300).contains(&status) {
        // The port answers None and the caller runs its own path, so a
        // refusal is a log line rather than an error anybody has to handle.
        tracing::warn!(
            "decide: {url} answered {status}: {}",
            talaria_body::truncate_utf16(&text, 200)
        );
        return None;
    }
    serde_json::from_str(&text).ok()
}

// ── Shared numeric helpers ───────────────────────────────────────────────────

/// Normalize non-negative weights into a distribution. None when nothing is
/// positive — an all-zero reply is not a uniform belief, it is no answer, and
/// a non-finite weight is dropped rather than poisoning the sum.
fn normalize(weights: &[f64]) -> Option<Vec<f64>> {
    let usable = |w: f64| if w.is_finite() && w > 0.0 { w } else { 0.0 };
    let total: f64 = weights.iter().copied().map(usable).sum();
    // Not finite covers the overflow case too: weights that sum past f64 would
    // divide every share to zero, which is not a distribution.
    if total <= 0.0 || !total.is_finite() {
        return None;
    }
    Some(weights.iter().copied().map(|w| usable(w) / total).collect())
}

/// The probability-weighted position over levels, 0-indexed.
///
/// COMPUTED HERE RATHER THAN READ OFF THE REPLY, deliberately: a provider may
/// index its levels from 0 or from 1 and the docs do not promise which, so
/// deriving the position from the distribution we were given is both
/// well-defined and provider-independent.
fn weighted_position(probabilities: &[f64]) -> f64 {
    probabilities
        .iter()
        .enumerate()
        .map(|(i, p)| i as f64 * p)
        .sum()
}

/// How concentrated a distribution is, as the top probability. Used only when
/// a provider gives probabilities and no confidence of its own.
fn concentration(probabilities: &[f64]) -> f64 {
    probabilities.iter().copied().fold(0.0, f64::max)
}

/// The state as one block of text, for the wires that take a single string.
fn state_text(state: &Value) -> String {
    match state {
        Value::String(s) => s.clone(),
        Value::Array(items) => items
            .iter()
            .map(|i| match i {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            })
            .collect::<Vec<_>>()
            .join("\n"),
        other => serde_json::to_string_pretty(other).unwrap_or_else(|_| other.to_string()),
    }
}

// ── systemone ────────────────────────────────────────────────────────────────

pub(crate) fn systemone_request(model: Option<&str>, ask: &Ask) -> Value {
    let mut questions = serde_json::Map::new();
    for (id, q) in &ask.questions {
        let body = match q {
            Question::Noul {
                instructions,
                yes_no,
            } => {
                let mut m = json!({"type": "noul", "instructions": instructions});
                if let Some((yes, no)) = yes_no {
                    m["criteria"] = json!({"true": yes, "false": no});
                }
                m
            }
            Question::Choice {
                instructions,
                options,
            } => {
                let criteria: serde_json::Map<String, Value> = options
                    .iter()
                    .map(|o| (o.id.clone(), json!(o.description)))
                    .collect();
                json!({"type": "choice", "instructions": instructions, "criteria": criteria})
            }
            Question::Score {
                instructions,
                levels,
            } => json!({"type": "score", "instructions": instructions, "criteria": levels}),
        };
        questions.insert(id.clone(), body);
    }
    let mut req = json!({"state": ask.state, "questions": questions});
    if let Some(m) = model {
        req["model"] = json!(m);
    }
    req
}

pub(crate) fn systemone_answers(reply: &Value, ask: &Ask) -> BTreeMap<String, Read> {
    let Some(answers) = reply.get("answers").and_then(|a| a.as_object()) else {
        return BTreeMap::new();
    };
    let mut out = BTreeMap::new();
    for (id, question) in &ask.questions {
        let Some(a) = answers.get(id) else { continue };
        // A partial answer IS an answer: an id the provider skipped is simply
        // absent, and the caller falls back for that one id alone.
        if let Some(read) = systemone_one(a, question) {
            out.insert(id.clone(), read);
        }
    }
    out
}

fn systemone_one(a: &Value, question: &Question) -> Option<Read> {
    let conf = a.get("confidence").and_then(|c| c.as_f64());
    match question {
        Question::Noul { .. } => {
            let p = a.get("noul")?.as_f64()?;
            // The model's own calibrated probability — nothing to derive.
            Some((Answer::Noul(p.clamp(0.0, 1.0)), true))
        }
        Question::Choice { options, .. } => {
            let chosen = a.get("choice")?.as_str()?;
            // AN ANSWER OUTSIDE THE OFFERED IDS IS NOT AN ANSWER. The wire
            // guarantees the schema, not that we and the provider agree about
            // what was asked; a call site switching on an id it never offered
            // is the one failure this port must not pass through.
            let opt = options.iter().find(|o| o.id == chosen)?;
            let probabilities = probs_by_id(a.get("probabilities"), options);
            let confidence = conf.or_else(|| {
                let ps: Vec<f64> = probabilities.values().copied().collect();
                (!ps.is_empty()).then(|| concentration(&ps))
            });
            Some((
                Answer::Choice {
                    id: opt.id.clone(),
                    probabilities,
                    confidence: confidence.unwrap_or(0.0).clamp(0.0, 1.0),
                },
                confidence.is_some(),
            ))
        }
        Question::Score { levels, .. } => {
            let raw: Vec<f64> = a
                .get("probabilities")
                .and_then(|p| p.as_array())
                .map(|arr| arr.iter().map(|v| v.as_f64().unwrap_or(0.0)).collect())
                .unwrap_or_default();
            if raw.len() == levels.len()
                && let Some(probabilities) = normalize(&raw)
            {
                let confidence = conf.unwrap_or_else(|| concentration(&probabilities));
                return Some((
                    Answer::Score {
                        position: weighted_position(&probabilities),
                        probabilities,
                        confidence: confidence.clamp(0.0, 1.0),
                    },
                    true,
                ));
            }
            // No usable distribution: take the position the provider states,
            // mapped into our 0-indexed range, and say the number is not one
            // we can stand behind.
            let score = a.get("score")?.as_f64()?;
            let span = (levels.len() - 1) as f64;
            Some((
                Answer::Score {
                    position: score.clamp(0.0, span),
                    probabilities: Vec::new(),
                    confidence: conf.unwrap_or(0.0).clamp(0.0, 1.0),
                },
                false,
            ))
        }
    }
}

/// `probabilities` keyed by option id, keeping only ids we offered and
/// renormalizing over them.
fn probs_by_id(raw: Option<&Value>, options: &[Opt]) -> BTreeMap<String, f64> {
    let Some(obj) = raw.and_then(|p| p.as_object()) else {
        return BTreeMap::new();
    };
    let weights: Vec<f64> = options
        .iter()
        .map(|o| obj.get(&o.id).and_then(|v| v.as_f64()).unwrap_or(0.0))
        .collect();
    match normalize(&weights) {
        Some(ps) => options.iter().map(|o| o.id.clone()).zip(ps).collect(),
        None => BTreeMap::new(),
    }
}

// ── predict ──────────────────────────────────────────────────────────────────

/// TEI's `/predict` takes pairs. A Noul asks one: does the state entail the
/// condition? A Choice asks one per option, batched into a single call, and
/// the winner is the option the classifier scores highest.
pub(crate) fn predict_request(state: &Value, question: &Question) -> Option<Value> {
    let text = state_text(state);
    let pairs: Vec<Value> = match question {
        Question::Noul { instructions, .. } => {
            vec![json!([text, instructions])]
        }
        Question::Choice { options, .. } => options
            .iter()
            .map(|o| json!([text.clone(), o.description.clone()]))
            .collect(),
        // Declared unserved in the registry; unreachable through `decide`.
        Question::Score { .. } => return None,
    };
    Some(json!({"inputs": pairs, "truncate": true}))
}

/// p(yes) out of one classification head's label set.
///
/// THREE LABEL VOCABULARIES, because that is what is actually deployed: an NLI
/// model answers entailment/neutral/contradiction, a fine-tune answers
/// yes/no or true/false, and an untouched binary head answers LABEL_1/LABEL_0.
/// Neutral is deliberately EXCLUDED from the denominator for NLI: "the state
/// neither entails nor contradicts this" is not evidence for no, and folding
/// it in drags every answer toward 0.5.
fn predict_yes(labels: &[(String, f64)]) -> Option<f64> {
    let find = |names: &[&str]| -> Option<f64> {
        labels
            .iter()
            .find(|(l, _)| names.contains(&l.trim().to_lowercase().as_str()))
            .map(|(_, s)| *s)
    };
    let yes = find(&["entailment", "yes", "true", "label_1", "positive"]);
    let no = find(&["contradiction", "no", "false", "label_0", "negative"]);
    match (yes, no) {
        (Some(y), Some(n)) => normalize(&[y, n]).map(|ps| ps[0]),
        // A single-label head reports only the positive score; it is already a
        // probability.
        (Some(y), None) if (0.0..=1.0).contains(&y) => Some(y),
        _ => None,
    }
}

fn label_scores(entry: &Value) -> Vec<(String, f64)> {
    entry
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|e| {
                    Some((
                        e.get("label")?.as_str()?.to_string(),
                        e.get("score")?.as_f64()?,
                    ))
                })
                .collect()
        })
        .unwrap_or_default()
}

pub(crate) fn predict_answer(reply: &Value, question: &Question) -> Option<Read> {
    let rows = reply.as_array()?;
    match question {
        Question::Noul { .. } => {
            let p = predict_yes(&label_scores(rows.first()?))?;
            Some((Answer::Noul(p.clamp(0.0, 1.0)), true))
        }
        Question::Choice { options, .. } => {
            if rows.len() != options.len() {
                return None;
            }
            let weights: Vec<f64> = rows
                .iter()
                .map(|r| predict_yes(&label_scores(r)).unwrap_or(0.0))
                .collect();
            let probabilities = normalize(&weights)?;
            let (top, _) =
                probabilities
                    .iter()
                    .enumerate()
                    .fold(
                        (0usize, f64::MIN),
                        |(bi, bp), (i, p)| {
                            if *p > bp { (i, *p) } else { (bi, bp) }
                        },
                    );
            Some((
                Answer::Choice {
                    id: options[top].id.clone(),
                    probabilities: options
                        .iter()
                        .map(|o| o.id.clone())
                        .zip(probabilities.iter().copied())
                        .collect(),
                    confidence: concentration(&probabilities),
                },
                true,
            ))
        }
        Question::Score { .. } => None,
    }
}

// ── chat ─────────────────────────────────────────────────────────────────────

/// The one-token alphabet a question offers, paired with what each token
/// means. Shared by the builder and the reader so the prompt and the parse
/// cannot disagree about which letter was which option.
fn chat_alphabet(question: &Question) -> Vec<(String, String)> {
    match question {
        Question::Noul { yes_no, .. } => {
            let (yes, no) = yes_no
                .clone()
                .unwrap_or_else(|| ("the condition holds".into(), "it does not".into()));
            vec![("YES".into(), yes), ("NO".into(), no)]
        }
        Question::Choice { options, .. } => options
            .iter()
            .take(LETTERS.len())
            .enumerate()
            .map(|(i, o)| (LETTERS[i].to_string(), o.description.clone()))
            .collect(),
        Question::Score { levels, .. } => levels
            .iter()
            .enumerate()
            .map(|(i, l)| ((i + 1).to_string(), l.clone()))
            .collect(),
    }
}

pub(crate) fn chat_request(model: Option<&str>, state: &Value, question: &Question) -> Value {
    let alphabet = chat_alphabet(question);
    let instructions = match question {
        Question::Noul { instructions, .. }
        | Question::Choice { instructions, .. }
        | Question::Score { instructions, .. } => instructions,
    };
    let tokens: Vec<&str> = alphabet.iter().map(|(t, _)| t.as_str()).collect();
    let menu: Vec<String> = alphabet
        .iter()
        .map(|(t, meaning)| format!("{t} = {meaning}"))
        .collect();
    let system = [
        "You answer a single question about the material below with exactly one token and nothing else.".to_string(),
        format!("THE QUESTION: {instructions}"),
        format!("ANSWER WITH EXACTLY ONE OF: {}", tokens.join(" ")),
        menu.join("\n"),
        "Reply with that one token only. No punctuation, no explanation, no quotes.".to_string(),
    ]
    .join("\n");
    let mut req = json!({
        "messages": [
            {"role": "system", "content": system},
            {"role": "user", "content": state_text(state)},
        ],
        // A decision that answers differently on a re-read is not a decision.
        "temperature": 0,
        // One token is the answer; the allowance is for a model that emits a
        // leading space or a stray newline.
        "max_tokens": 4,
        "logprobs": true,
        // Enough of the distribution to cover every token we offered, within
        // the ceiling providers commonly impose.
        "top_logprobs": 20usize.min(tokens.len().max(2) * 2),
        "stream": false,
    });
    if let Some(m) = model {
        req["model"] = json!(m);
    }
    req
}

/// The reply's first content token and, when the endpoint served them, the
/// alternatives it was weighed against.
fn chat_first_token(reply: &Value) -> Option<(String, Vec<(String, f64)>)> {
    let choice = reply.get("choices")?.as_array()?.first()?;
    let text = choice
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .unwrap_or_default()
        .to_string();
    let alternatives = choice
        .get("logprobs")
        .and_then(|l| l.get("content"))
        .and_then(|c| c.as_array())
        .and_then(|c| c.first())
        .and_then(|first| first.get("top_logprobs"))
        .and_then(|t| t.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|e| {
                    Some((
                        e.get("token")?.as_str()?.to_string(),
                        e.get("logprob")?.as_f64()?,
                    ))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Some((text, alternatives))
}

/// Does this token name this answer? Tokens arrive with leading spaces and in
/// whatever case the model felt like, and `"A"` must not match the `A` inside
/// `"ANSWER"` — so the comparison is on the trimmed, case-folded whole token.
fn token_is(token: &str, want: &str) -> bool {
    token
        .trim()
        .trim_matches(['"', '\'', '.', ','])
        .eq_ignore_ascii_case(want)
}

pub(crate) fn chat_answer(reply: &Value, question: &Question) -> Option<Read> {
    let alphabet = chat_alphabet(question);
    let (text, alternatives) = chat_first_token(reply)?;

    // The distribution, when the endpoint served logprobs: one weight per
    // token we offered, summing each token's alternatives.
    let weights: Vec<f64> = alphabet
        .iter()
        .map(|(tok, _)| {
            alternatives
                .iter()
                .filter(|(t, _)| token_is(t, tok))
                .map(|(_, lp)| lp.exp())
                .sum::<f64>()
        })
        .collect();
    let distribution = normalize(&weights).filter(|_| !alternatives.is_empty());

    // The answer itself. The text is authoritative — it is what the model
    // actually emitted — and the distribution only says how sure it was.
    let said = text.trim();
    let picked = alphabet
        .iter()
        .position(|(tok, _)| {
            said.split_whitespace()
                .next()
                .is_some_and(|w| token_is(w, tok))
        })
        .or_else(|| {
            // No parseable text, but a distribution: take its argmax rather
            // than throwing away a usable call.
            distribution.as_ref().map(|ps| {
                ps.iter()
                    .enumerate()
                    .fold(
                        (0usize, f64::MIN),
                        |(bi, bp), (i, p)| {
                            if *p > bp { (i, *p) } else { (bi, bp) }
                        },
                    )
                    .0
            })
        })?;

    let calibrated = distribution.is_some();
    let ps = distribution.unwrap_or_default();
    match question {
        Question::Noul { .. } => {
            // Index 0 is YES by construction in `chat_alphabet`.
            let p = ps
                .first()
                .copied()
                .unwrap_or(if picked == 0 { 1.0 } else { 0.0 });
            Some((Answer::Noul(p.clamp(0.0, 1.0)), calibrated))
        }
        Question::Choice { options, .. } => {
            let id = options.get(picked)?.id.clone();
            let probabilities = if ps.is_empty() {
                BTreeMap::new()
            } else {
                options
                    .iter()
                    .map(|o| o.id.clone())
                    .zip(ps.iter().copied())
                    .collect()
            };
            let confidence = if ps.is_empty() {
                0.0
            } else {
                concentration(&ps)
            };
            Some((
                Answer::Choice {
                    id,
                    probabilities,
                    confidence,
                },
                calibrated,
            ))
        }
        Question::Score { levels, .. } => {
            if ps.len() == levels.len() {
                return Some((
                    Answer::Score {
                        position: weighted_position(&ps),
                        probabilities: ps.clone(),
                        confidence: concentration(&ps),
                    },
                    calibrated,
                ));
            }
            Some((
                Answer::Score {
                    position: picked as f64,
                    probabilities: Vec::new(),
                    confidence: 0.0,
                },
                false,
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts() -> Vec<Opt> {
        vec![
            Opt::new("billing", "payment or subscription issues"),
            Opt::new("technical", "bugs or integration problems"),
            Opt::new("sales", "pricing or account questions"),
        ]
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }

    // ── systemone ────────────────────────────────────────────────────────────

    #[test]
    fn the_systemone_request_is_state_plus_named_questions_in_the_documented_shape() {
        let ask = Ask::new(json!({"message": "my card was charged twice"}))
            .q(
                "refund",
                Question::noul("Does the customer request a refund?"),
            )
            .q(
                "team",
                Question::choice("Which team should handle this", opts()),
            )
            .q(
                "anger",
                Question::score(
                    "How frustrated the customer appears",
                    vec![
                        "calm".into(),
                        "frustrated but civil".into(),
                        "very angry".into(),
                    ],
                ),
            );
        let req = systemone_request(Some("jev-latest"), &ask);
        assert_eq!(req["model"], json!("jev-latest"));
        assert_eq!(req["state"]["message"], json!("my card was charged twice"));
        assert_eq!(req["questions"]["refund"]["type"], json!("noul"));
        assert!(req["questions"]["refund"].get("criteria").is_none());
        // A choice's criteria is a MAP of option id → description…
        assert_eq!(
            req["questions"]["team"]["criteria"]["billing"],
            json!("payment or subscription issues")
        );
        // …and a score's is an ORDERED ARRAY of level descriptions.
        assert_eq!(
            req["questions"]["anger"]["criteria"],
            json!(["calm", "frustrated but civil", "very angry"])
        );
    }

    #[test]
    fn a_nouls_yes_and_no_meanings_ride_as_the_documented_true_false_criteria() {
        let ask = Ask::new(json!("x")).q(
            "q",
            Question::noul_meaning("is it urgent", "someone is blocked now", "it can wait"),
        );
        let req = systemone_request(None, &ask);
        assert_eq!(
            req["questions"]["q"]["criteria"],
            json!({"true": "someone is blocked now", "false": "it can wait"})
        );
        assert!(req.get("model").is_none(), "no model, no key in the body");
    }

    #[test]
    fn a_noul_answer_is_the_probability_itself_and_is_calibrated_by_construction() {
        let ask = Ask::new(json!("x")).q("q", Question::noul("is it raining"));
        let reply = json!({"answers": {"q": {"type": "noul", "noul": 0.87}}});
        let got = systemone_answers(&reply, &ask);
        assert_eq!(got["q"], (Answer::Noul(0.87), true));
    }

    #[test]
    fn a_choice_answer_keeps_the_offered_ids_and_renormalizes_over_them() {
        let ask = Ask::new(json!("x")).q("q", Question::choice("which team", opts()));
        let reply = json!({"answers": {"q": {
            "type": "choice",
            "choice": "technical",
            "probabilities": {"billing": 0.1, "technical": 0.8, "sales": 0.1},
            "confidence": 0.8,
        }}});
        let (answer, calibrated) = systemone_answers(&reply, &ask)
            .remove("q")
            .expect("answered");
        assert!(calibrated);
        match answer {
            Answer::Choice {
                id,
                probabilities,
                confidence,
            } => {
                assert_eq!(id, "technical");
                assert!(close(confidence, 0.8));
                assert!(close(probabilities["technical"], 0.8));
                assert_eq!(probabilities.len(), 3);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_choice_outside_the_offered_ids_is_dropped_rather_than_handed_to_a_call_site() {
        // THE ONE FAILURE THIS PORT MUST NOT PASS THROUGH: a call site
        // switching on an id it never offered.
        let ask = Ask::new(json!("x")).q("q", Question::choice("which team", opts()));
        let reply = json!({"answers": {"q": {
            "type": "choice", "choice": "legal", "confidence": 0.99,
        }}});
        assert!(systemone_answers(&reply, &ask).is_empty());
    }

    #[test]
    fn a_score_position_is_derived_from_the_distribution_not_read_off_the_reply() {
        // The provider says 2.0; the distribution says 1.0. We take the
        // distribution, because nothing promises whether `score` is 0- or
        // 1-indexed.
        let ask = Ask::new(json!("x")).q(
            "q",
            Question::score(
                "how angry",
                vec!["calm".into(), "cross".into(), "furious".into()],
            ),
        );
        let reply = json!({"answers": {"q": {
            "type": "score", "score": 2.0,
            "probabilities": [0.0, 1.0, 0.0], "confidence": 0.9,
        }}});
        let (answer, calibrated) = systemone_answers(&reply, &ask)
            .remove("q")
            .expect("answered");
        assert!(calibrated);
        match answer {
            Answer::Score {
                position,
                confidence,
                ..
            } => {
                assert!(close(position, 1.0), "weighted over 0-indexed levels");
                assert!(close(confidence, 0.9));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_score_with_no_usable_distribution_still_answers_but_says_it_is_uncalibrated() {
        let ask = Ask::new(json!("x")).q(
            "q",
            Question::score(
                "how angry",
                vec!["calm".into(), "cross".into(), "furious".into()],
            ),
        );
        let reply = json!({"answers": {"q": {"type": "score", "score": 1.5}}});
        let (answer, calibrated) = systemone_answers(&reply, &ask)
            .remove("q")
            .expect("answered");
        assert!(
            !calibrated,
            "no distribution means no number to stand behind"
        );
        assert_eq!(answer.position(), Some(1.5));
    }

    #[test]
    fn a_partial_answer_set_returns_what_came_back_and_omits_what_did_not() {
        let ask = Ask::new(json!("x"))
            .q("a", Question::noul("one"))
            .q("b", Question::noul("two"));
        let reply = json!({"answers": {"a": {"type": "noul", "noul": 0.6}}});
        let got = systemone_answers(&reply, &ask);
        assert_eq!(
            got.len(),
            1,
            "four good answers are never binned over one bad"
        );
        assert!(got.contains_key("a"));
        assert!(!got.contains_key("b"));
    }

    #[test]
    fn a_reply_with_no_answers_object_yields_nothing_rather_than_a_default() {
        let ask = Ask::new(json!("x")).q("q", Question::noul("one"));
        assert!(systemone_answers(&json!({"error": "nope"}), &ask).is_empty());
        assert!(systemone_answers(&json!({"answers": 7}), &ask).is_empty());
    }

    // ── predict ──────────────────────────────────────────────────────────────

    #[test]
    fn a_noul_predict_request_is_one_premise_hypothesis_pair() {
        let req = predict_request(
            &json!("the deploy is failing"),
            &Question::noul("is this urgent"),
        )
        .expect("built");
        assert_eq!(
            req["inputs"],
            json!([["the deploy is failing", "is this urgent"]])
        );
    }

    #[test]
    fn a_choice_predict_request_batches_one_pair_per_option_in_offer_order() {
        let req = predict_request(&json!("help"), &Question::choice("which team", opts()))
            .expect("built");
        let rows = req["inputs"].as_array().expect("an array");
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[1][1], json!("bugs or integration problems"));
    }

    #[test]
    fn a_score_is_refused_by_the_predict_wire_because_the_registry_never_offers_it() {
        assert!(
            predict_request(
                &json!("x"),
                &Question::score("how much", vec!["a".into(), "b".into()])
            )
            .is_none()
        );
    }

    #[test]
    fn nli_labels_read_entailment_against_contradiction_and_ignore_neutral() {
        // NEUTRAL IS EXCLUDED ON PURPOSE: "neither entails nor contradicts" is
        // not evidence for no, and including it drags every answer to 0.5.
        let reply = json!([[
            {"label": "entailment", "score": 0.6},
            {"label": "neutral", "score": 0.3},
            {"label": "contradiction", "score": 0.2},
        ]]);
        let (answer, calibrated) =
            predict_answer(&reply, &Question::noul("is it urgent")).expect("answered");
        assert!(calibrated);
        assert!(close(answer.probability().expect("a noul"), 0.75)); // 0.6/(0.6+0.2)
    }

    #[test]
    fn the_three_deployed_label_vocabularies_all_read() {
        for (yes, no) in [
            ("entailment", "contradiction"),
            ("yes", "no"),
            ("LABEL_1", "LABEL_0"),
        ] {
            let reply = json!([[{"label": yes, "score": 0.9}, {"label": no, "score": 0.1}]]);
            let (answer, _) = predict_answer(&reply, &Question::noul("q")).expect("answered");
            assert!(
                close(answer.probability().expect("a noul"), 0.9),
                "{yes}/{no}"
            );
        }
    }

    #[test]
    fn an_unrecognized_label_set_answers_nothing_rather_than_guessing() {
        let reply =
            json!([[{"label": "sports", "score": 0.9}, {"label": "politics", "score": 0.1}]]);
        assert!(predict_answer(&reply, &Question::noul("q")).is_none());
    }

    #[test]
    fn a_choice_predict_reply_picks_the_top_option_and_normalizes_across_them() {
        let row = |s: f64| json!([{"label": "entailment", "score": s}, {"label": "contradiction", "score": 1.0 - s}]);
        let reply = json!([row(0.2), row(0.8), row(0.2)]);
        let (answer, calibrated) =
            predict_answer(&reply, &Question::choice("which team", opts())).expect("answered");
        assert!(calibrated);
        assert_eq!(answer.chosen(), Some("technical"));
    }

    #[test]
    fn a_choice_predict_reply_of_the_wrong_length_is_refused_not_aligned_by_guesswork() {
        let reply = json!([[{"label": "entailment", "score": 0.9}]]);
        assert!(predict_answer(&reply, &Question::choice("which team", opts())).is_none());
    }

    // ── chat ─────────────────────────────────────────────────────────────────

    #[test]
    fn the_chat_request_pins_a_one_token_answer_and_asks_for_logprobs() {
        let req = chat_request(
            Some("gpt-x"),
            &json!("the deploy is failing"),
            &Question::noul("is this urgent"),
        );
        assert_eq!(req["model"], json!("gpt-x"));
        assert_eq!(req["temperature"], json!(0));
        assert_eq!(req["logprobs"], json!(true));
        assert!(req["top_logprobs"].as_u64().expect("a number") >= 2);
        let system = req["messages"][0]["content"].as_str().expect("a string");
        assert!(system.contains("EXACTLY ONE OF: YES NO"), "{system}");
        assert!(system.contains("is this urgent"), "{system}");
        // The state is the USER turn, kept apart from the rubric.
        assert_eq!(
            req["messages"][1]["content"],
            json!("the deploy is failing")
        );
    }

    #[test]
    fn a_choice_chat_request_offers_letters_and_the_reader_maps_them_back_to_ids() {
        let q = Question::choice("which team", opts());
        let req = chat_request(None, &json!("help"), &q);
        let system = req["messages"][0]["content"].as_str().expect("a string");
        assert!(system.contains("EXACTLY ONE OF: A B C"), "{system}");
        assert!(
            system.contains("B = bugs or integration problems"),
            "{system}"
        );

        let reply = json!({"choices": [{
            "message": {"content": "B"},
            "logprobs": {"content": [{"top_logprobs": [
                {"token": "B", "logprob": -0.1},
                {"token": "A", "logprob": -2.5},
                {"token": "C", "logprob": -3.0},
            ]}]},
        }]});
        let (answer, calibrated) = chat_answer(&reply, &q).expect("answered");
        assert!(calibrated);
        assert_eq!(answer.chosen(), Some("technical"));
        assert!(answer.certainty() > 0.8);
    }

    #[test]
    fn a_noul_chat_answer_reads_its_probability_out_of_the_first_tokens_alternatives() {
        let q = Question::noul("is this urgent");
        let reply = json!({"choices": [{
            "message": {"content": "YES"},
            "logprobs": {"content": [{"top_logprobs": [
                // exp(0) = 1 vs exp(ln 1/3) — a 75/25 split.
                {"token": " YES", "logprob": 0.0},
                {"token": "NO", "logprob": -1.0986122886681098},
            ]}]},
        }]});
        let (answer, calibrated) = chat_answer(&reply, &q).expect("answered");
        assert!(calibrated);
        assert!(
            close(answer.probability().expect("a noul"), 0.75),
            "{answer:?}"
        );
    }

    #[test]
    fn a_chat_reply_with_no_logprobs_still_answers_but_is_never_calibrated() {
        // THE WHOLE REASON `calibrated` IS A FIELD. The answer is usable; the
        // confidence does not exist, and inventing one would make a guess look
        // like evidence.
        let q = Question::noul("is this urgent");
        let reply = json!({"choices": [{"message": {"content": "NO"}}]});
        let (answer, calibrated) = chat_answer(&reply, &q).expect("answered");
        assert!(!calibrated);
        assert_eq!(answer.probability(), Some(0.0));
        // And the gate refuses it, however extreme the number looks.
        assert!(
            crate::gated(
                Some(Judgment {
                    answer,
                    calibrated,
                    provider: "gateway".into(),
                    model: None,
                    latency_ms: 1,
                }),
                0.1,
            )
            .is_none()
        );
    }

    #[test]
    fn a_token_matches_only_as_a_whole_word_so_a_prose_reply_does_not_read_as_an_answer() {
        assert!(token_is(" YES", "YES"));
        assert!(token_is("\"B\"", "B"));
        assert!(token_is("yes", "YES"));
        assert!(!token_is("YESTERDAY", "YES"));
        // A model that ignored the instruction and wrote a sentence: the first
        // word is not one of the tokens, and with no logprobs there is nothing
        // to fall back to, so the judgment is dropped.
        let reply = json!({"choices": [{"message": {"content": "Answering your question: it is urgent."}}]});
        assert!(chat_answer(&reply, &Question::noul("is this urgent")).is_none());
    }

    #[test]
    fn an_empty_reply_with_a_distribution_falls_back_to_the_argmax_rather_than_wasting_the_call() {
        let q = Question::noul("is this urgent");
        let reply = json!({"choices": [{
            "message": {"content": ""},
            "logprobs": {"content": [{"top_logprobs": [
                {"token": "NO", "logprob": -0.05},
                {"token": "YES", "logprob": -3.0},
            ]}]},
        }]});
        let (answer, calibrated) = chat_answer(&reply, &q).expect("answered");
        assert!(calibrated);
        assert!(answer.probability().expect("a noul") < 0.2);
    }

    #[test]
    fn a_score_chat_request_offers_digits_from_one_and_reads_a_zero_indexed_position() {
        let q = Question::score(
            "how angry",
            vec!["calm".into(), "cross".into(), "furious".into()],
        );
        let req = chat_request(None, &json!("x"), &q);
        let system = req["messages"][0]["content"].as_str().expect("a string");
        assert!(system.contains("EXACTLY ONE OF: 1 2 3"), "{system}");
        assert!(system.contains("1 = calm"), "{system}");

        let reply = json!({"choices": [{
            "message": {"content": "2"},
            "logprobs": {"content": [{"top_logprobs": [
                {"token": "2", "logprob": 0.0},
                {"token": "1", "logprob": -50.0},
                {"token": "3", "logprob": -50.0},
            ]}]},
        }]});
        let (answer, calibrated) = chat_answer(&reply, &q).expect("answered");
        assert!(calibrated);
        // Offered as "2", returned as position 1.0 — 0-indexed, as the port's
        // own contract promises.
        assert!(
            close(answer.position().expect("a score"), 1.0),
            "{answer:?}"
        );
    }

    // ── the numeric helpers ──────────────────────────────────────────────────

    #[test]
    fn normalize_refuses_an_all_zero_reply_because_that_is_no_answer_not_a_uniform_one() {
        assert!(normalize(&[0.0, 0.0]).is_none());
        assert!(normalize(&[]).is_none());
        assert!(normalize(&[-1.0, -2.0]).is_none());
        assert!(normalize(&[f64::NAN]).is_none());
        let ps = normalize(&[1.0, 3.0]).expect("normalized");
        assert!(close(ps[0], 0.25) && close(ps[1], 0.75));
    }

    #[test]
    fn a_weighted_position_is_zero_indexed_and_may_land_between_levels() {
        assert!(close(weighted_position(&[1.0, 0.0, 0.0]), 0.0));
        assert!(close(weighted_position(&[0.0, 0.0, 1.0]), 2.0));
        assert!(close(weighted_position(&[0.0, 0.5, 0.5]), 1.5));
    }

    #[test]
    fn the_state_flattens_to_text_for_the_wires_that_take_one_string() {
        assert_eq!(state_text(&json!("plain")), "plain");
        assert_eq!(state_text(&json!(["a", "b"])), "a\nb");
        assert!(state_text(&json!({"k": "v"})).contains("\"k\""));
    }
}
