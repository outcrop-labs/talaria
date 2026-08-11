// The gateway's parameter learner, reduced to the parts that must be provable.
//
// `llm-gateway.ts` learns which parameters an upstream refuses by reading its
// 400s ("`temperature` is deprecated", "Unsupported parameter: 'top_p'"),
// stripping the named parameter, retrying, and remembering the answer per
// endpoint:model. That mechanism is right and stays: model specs rot, providers
// are always current, and no hand-maintained support table has ever kept up with
// a vendor retiring a tunable mid-quarter.
//
// AUDIT 1.2 — what was wrong was WHICH parameters it was willing to forget.
// `rejectedParam` matches a bare lowercase identifier, so `response_format` was
// as strippable as `top_p`. A model that refuses JSON mode got the constraint
// deleted, the retry SUCCEEDED, and the caller — which had asked for JSON
// precisely because it was about to run a JSON parser — received free prose with
// no signal that anything had changed. The one mode where the harness KNEW it
// was in structured-output mode silently became the mode where it wasn't. That
// is the most damaging silent failure in the harness layer, and it is a
// classification bug: some parameters are tuning, and some are the caller's
// CONTRACT with the model.
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
// The second half of the finding was that the learnings were FOREVER: persisted
// with no timestamp and no invalidation, so a provider that later fixed support
// was never re-tried. A one-way ratchet on capability. Hence the TTL below, and
// hence this module owning the stored shape — the migration off the old
// timestamp-free format is the fiddly part and it deserves tests, not a
// try/catch in a fetch path.
//
// Pure by construction: no database, no gateway, no clock of its own (`now` is
// always an argument). `llm-gateway.ts` supplies the I/O.
import type { Capability } from './capability'

// ── Classification ───────────────────────────────────────────────────────────

/** Parameters that decide what comes BACK, not how good it is. */
export type ContractParam =
  | 'response_format'
  | 'tools'
  | 'tool_choice'
  | 'parallel_tool_calls'
  // Legacy OpenAI tool spelling. Live providers still 400 on these by name
  // ("`functions` is deprecated, use `tools`"), and that 400 says exactly as
  // much about tool support as the modern spelling does.
  | 'functions'
  | 'function_call'
  // vLLM / SGLang / llama.cpp structured-output extensions. These matter more
  // here than the OpenAI spelling does: they are how a SELF-HOSTED 14B model is
  // actually constrained to a schema, which is the deployment this whole audit
  // is about. A local server that rejects one of them has told us it cannot do
  // guided decoding, and that is a `json` fact.
  | 'guided_json'
  | 'guided_regex'
  | 'guided_choice'
  | 'response_schema'

/** Which capability a rejection of each contract parameter proves absent.
 *  Written as an exhaustive record so adding a member to `ContractParam` fails
 *  the build HERE rather than silently producing a parameter that classifies as
 *  contract-bearing and then maps to nothing. */
export const CONTRACT_PARAMS: Record<ContractParam, Capability> = {
  response_format: 'json',
  guided_json: 'json',
  guided_regex: 'json',
  guided_choice: 'json',
  response_schema: 'json',
  tools: 'tools',
  tool_choice: 'tools',
  parallel_tool_calls: 'tools',
  functions: 'tools',
  function_call: 'tools',
}

const CONTRACT_IDS: ReadonlySet<string> = new Set(Object.keys(CONTRACT_PARAMS))

export const isContractParam = (param: string): param is ContractParam => CONTRACT_IDS.has(param)

/** Parameters the learner may never remove, whatever the upstream says.
 *
 *  `model` and `messages` were already refused by hand in `fetchUpstreamInner` —
 *  stripping either turns the request into nonsense. `stream` joins them for a
 *  subtler reason: dropping it produces a PERFECTLY VALID single JSON body,
 *  which the caller then hands to an SSE pump that will sit there reading a
 *  stream that never arrives. A 400 relayed honestly is a far better outcome
 *  than a hang, and unlike `response_format` there is no repair path to take —
 *  the caller asked for a transport it cannot get. */
const PROTECTED_PARAMS: ReadonlySet<string> = new Set(['model', 'messages', 'stream'])

export type ParamClass = 'cosmetic' | 'contract' | 'protected'

/** Everything unrecognized classifies as cosmetic, deliberately.
 *
 *  This preserves the behavior that has worked for every tunable since the
 *  learner shipped (`temperature`, `top_p`, `top_k`, `frequency_penalty`,
 *  `presence_penalty`, `seed`, `stop`, `logprobs`, `min_p`, and whatever the
 *  next vendor invents), and the blast radius of being wrong is bounded: an
 *  unrecognized parameter that was actually load-bearing degrades quality,
 *  where the contract list above is the set whose removal changes the TYPE of
 *  the response. If a new structured-output or tool-calling parameter appears,
 *  add it to `ContractParam` — that is the one edit this file asks for. */
export function classifyParam(param: string): ParamClass {
  if (PROTECTED_PARAMS.has(param)) return 'protected'
  return isContractParam(param) ? 'contract' : 'cosmetic'
}

// ── Reading the rejection out of a 400 ───────────────────────────────────────

/** The parameter an upstream 400 is complaining about, or null.
 *
 *  Three phrasings, all seen in production: Anthropic's OpenAI-compat layer
 *  ("`temperature` is deprecated"), OpenAI's own ("Unsupported parameter:
 *  'top_p'"), and the loose middle ground everyone else writes ("'seed' is not
 *  supported by this model"). Kept narrow on purpose — a pattern that matched
 *  more would start stripping parameters that were merely MENTIONED in an error
 *  about something else. */
export function rejectedParam(errText: string): string | null {
  // THE PROVIDER'S OWN ANSWER, BEFORE ANY PROSE. OpenAI-compatible errors carry
  // `"param": "<name>"` beside the message, and it is authoritative where every
  // pattern below is a guess at English:
  //
  //   {"error":{"message":"Function tools with reasoning_effort are not supported
  //     for gpt-5.6-terra in /v1/chat/completions. ... or set reasoning_effort to
  //     'none'.", "type":"invalid_request_error", "param":"reasoning_effort"}}
  //
  // That message names the parameter three times and quotes it NONE of them, so
  // every prose pattern in this function misses it — and the run it came from
  // filed 28 cases as "could not reach this model" across four harnesses, on an
  // endpoint that was answering fine.
  //
  // A dotted path is reduced to its root for the same reason the field-path
  // pattern below does it: what Talaria can stop sending is `response_format`,
  // not `response_format.json_schema.strict`.
  const field = /"param"\s*:\s*"([a-z_]+)(?:\.[A-Za-z0-9_]+)*"/i.exec(errText)
  if (field?.[1]) return field[1]

  const m =
    /[`"']([a-z_]+)[`"'] is (?:deprecated|not supported|unsupported)/i.exec(errText) ??
    /unsupported parameter[:\s`"']+([a-z_]+)/i.exec(errText) ??
    // OPENAI'S OTHER TWO PHRASINGS, and they are not cosmetic variants — a
    // parameter the ratchet cannot NAME is one it never stops sending, so the
    // endpoint 400s on every call for as long as the default is configured.
    //
    //   Unrecognized request argument supplied: reasoning
    //   Unknown parameter: 'reasoning'.
    //
    // Neither quotes the name next to "not supported", so none of the patterns
    // above sees them. `reasoning` is the one that reaches us in practice: it is
    // a legitimate OpenRouter request default, and an admin who sets it on one
    // endpoint and points a second at OpenAI gets it forwarded to a provider
    // that refuses it.
    /unrecognized request argument supplied[:\s`"']+([a-z_]+)/i.exec(errText) ??
    /unknown parameter[:\s`"']+([a-z_]+)/i.exec(errText) ??
    // THE VALUE COMPLAINT, which is a PARAMETER complaint wearing different
    // words — and the one that bites OpenAI's reasoning models:
    //
    //   Unsupported value: 'temperature' does not support 0.2 with this model.
    //   Only the default (1) is supported.
    //
    // Note "does not support", not "is not supported": the patterns above look
    // for the passive form and miss this entirely, so `temperature` was never
    // learned and every harness that declares one 400'd on that endpoint for
    // ever. Dropping it is the right answer — the model then runs at its
    // default, which is the only value it has.
    /unsupported value[:\s]*[`"']([a-z_]+)[`"']/i.exec(errText) ??
    /[`"']([a-z_]+)[`"'][^.]{0,40}(?:deprecated|not supported)/i.exec(errText) ??
    // THE FIELD-PATH SHAPE, which is how a provider that validates the request
    // body reports a field it will not accept:
    //
    //   response_format.type: Input should be 'json_schema'
    //   response_format.json_schema.strict: Input should be True
    //   response_format.json_schema.schema: Empty schema ({}) ... is not supported
    //
    // All three are Anthropic, all three are about `response_format`, and none
    // of the patterns above matches one — they look for a QUOTED parameter name
    // next to "not supported", and this shape quotes nothing and names the field
    // by path. So the strip-and-retry never fired, the 400 went back to the
    // caller, and the fitness suite recorded it as the model failing its
    // contract on every structured call.
    //
    // Matching the ROOT of the path is what matters: the parameter Talaria can
    // stop sending is `response_format`, not `response_format.json_schema.strict`.
    // `classifyParam` still refuses to strip a protected one, so a complaint
    // about `messages.0.content` cannot turn into a request with no messages.
    /(?:^|[\s"'{,])([a-z_]{3,})(?:\.[A-Za-z0-9_]+)*\s*:\s*(?:Input should be|Extra inputs are not permitted|Empty schema|Field required)/i.exec(errText)
  return m?.[1] ?? null
}

// ── The persisted store ──────────────────────────────────────────────────────

/** How long a learned strip survives without being re-confirmed by a fresh 400.
 *
 *  Deliberately the same 30 days as `LEARNED_TTL_MS` in `capability.ts`: the two
 *  stores record the same event from two angles (what we stopped sending, and
 *  what that told us about the model), and a strip that outlived its capability
 *  fact would keep the contract quietly dropped while the admin UI reported the
 *  model as unknown-but-fine. */
export const LEARNED_PARAM_TTL_MS = 30 * 24 * 60 * 60 * 1000

/** Current on-disk shape: endpoint:model -> parameter -> ISO time it was
 *  learned. The predecessor stored `Record<string, string[]>` and is still out
 *  there in every existing install's `app_settings`. */
export type StoredLearnedParams = Record<string, Record<string, string>>

/** endpoint:model -> parameter -> epoch ms it was learned. */
export type LearnedParamMap = Map<string, Map<string, number>>

export interface ReadLearnedParams {
  byKey: LearnedParamMap
  /** The read normalized something — a legacy timestamp-free entry, an expired
   *  one, or a corrupt one. The caller should write the result back ONCE.
   *
   *  This matters most for the legacy shape. A timestamp-free entry is stamped
   *  fresh-from-now, which is the only honest reading (we know the strip was
   *  learned at some point, we cannot know when, and guessing "long ago" would
   *  throw away good learnings while guessing "never expires" reinstates the
   *  exact ratchet the TTL removes). But if that stamp is only ever held in
   *  memory, a process that restarts weekly restamps it weekly and the entry
   *  becomes permanent anyway. Persisting on first read is what actually starts
   *  the clock. */
  changed: boolean
}

/** Parse the stored value, tolerating both shapes and any hand-edited garbage.
 *
 *  `app_settings` is JSON that outlives the code that wrote it, and this runs on
 *  the path to every upstream call — anything unrecognized is dropped rather
 *  than thrown, which costs at most one re-learned 400. */
export function readLearnedParams(raw: unknown, now: number): ReadLearnedParams {
  const byKey: LearnedParamMap = new Map()
  let changed = false
  if (!raw || typeof raw !== 'object' || Array.isArray(raw)) return { byKey, changed }

  for (const [key, entry] of Object.entries(raw as Record<string, unknown>)) {
    const params = new Map<string, number>()

    if (Array.isArray(entry)) {
      // Legacy: a bare list of parameter names, no timestamps. Adopt them all
      // at `now` so they keep working today and become expirable tomorrow.
      for (const p of entry) if (typeof p === 'string' && p) params.set(p, now)
      changed = true
    } else if (entry && typeof entry === 'object') {
      for (const [param, at] of Object.entries(entry as Record<string, unknown>)) {
        const ms = typeof at === 'string' ? Date.parse(at) : NaN
        if (!Number.isFinite(ms)) {
          // A corrupt timestamp is NOT the legacy shape — the legacy shape is an
          // array, handled above and recognizable as such. This is an entry
          // written by something that got the format wrong, and re-stamping it
          // fresh would grant an unknown writer a permanent strip. Drop it; the
          // next 400 re-learns it in the shape we understand.
          changed = true
          continue
        }
        if (now - ms > LEARNED_PARAM_TTL_MS) {
          changed = true
          continue
        }
        params.set(param, ms)
      }
    } else {
      changed = true
      continue
    }

    if (params.size > 0) byKey.set(key, params)
    else changed = true
  }
  return { byKey, changed }
}

/** Serialize for `app_settings`. Expired entries are dropped on the way out, so
 *  a store that is written for any reason is also a store that is pruned. */
export function writeLearnedParams(byKey: LearnedParamMap, now: number): StoredLearnedParams {
  const out: StoredLearnedParams = {}
  for (const [key, params] of byKey) {
    const entry: Record<string, string> = {}
    for (const [param, at] of params) {
      if (now - at > LEARNED_PARAM_TTL_MS) continue
      entry[param] = new Date(at).toISOString()
    }
    if (Object.keys(entry).length > 0) out[key] = entry
  }
  return out
}

/** The parameters still worth pre-stripping for this key, dropping any that have
 *  aged out. Mutates `byKey` — an expired learning is deleted, not merely
 *  ignored, so the next persist stops carrying it and the next 400 is allowed to
 *  re-learn it from a provider that may since have fixed support.
 *
 *  Returns the survivors plus whether anything was forgotten, because the caller
 *  owns persistence and must not write on every single call. */
export function activeLearnedParams(
  byKey: LearnedParamMap,
  key: string,
  now: number,
): { params: string[]; expired: boolean } {
  const entry = byKey.get(key)
  if (!entry) return { params: [], expired: false }
  let expired = false
  for (const [param, at] of entry) {
    if (now - at > LEARNED_PARAM_TTL_MS) {
      entry.delete(param)
      expired = true
    }
  }
  if (entry.size === 0) byKey.delete(key)
  return { params: [...entry.keys()], expired }
}

// ── The signal a dropped contract parameter sends to the caller ──────────────

/** One contract-bearing parameter that did NOT reach the model.
 *
 *  Handed back on the `UpstreamCall` so a caller can tell "the model answered my
 *  structured request" from "the model answered a request I no longer made".
 *  Both look identical in the response body, which is the whole reason this type
 *  exists. */
export interface ContractDrop {
  param: ContractParam
  /** The capability the drop proves absent — 'json' or 'tools'. */
  capability: Capability
  endpoint: string
  /** The upstream model id, as `capabilityKey` spells it. */
  model: string
  /** `rejected`: the upstream 400'd on this very call and we retried without the
   *  parameter. `remembered`: an earlier 400 taught us, and `buildUpstream`
   *  never sent it. The caller treats them identically; observability doesn't. */
  source: 'rejected' | 'remembered'
}
