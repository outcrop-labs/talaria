import { describe, expect, it } from 'vitest'

import {
  activeLearnedParams,
  classifyParam,
  CONTRACT_PARAMS,
  isContractParam,
  LEARNED_PARAM_TTL_MS,
  readLearnedParams,
  rejectedParam,
  writeLearnedParams,
  type LearnedParamMap,
} from './gateway-params'

const KEY = 'pl-main:qwen3-14b'
const NOW = Date.parse('2026-08-06T12:00:00.000Z')
const iso = (msAgo: number): string => new Date(NOW - msAgo).toISOString()
const DAY = 24 * 60 * 60 * 1000

const map = (entries: Record<string, Record<string, number>>): LearnedParamMap =>
  new Map(Object.entries(entries).map(([k, v]) => [k, new Map(Object.entries(v))]))

// ── Classification ───────────────────────────────────────────────────────────

describe('classifyParam', () => {
  it('treats tunables as cosmetic — silently strippable, as they always were', () => {
    for (const p of ['temperature', 'top_p', 'top_k', 'frequency_penalty', 'presence_penalty', 'seed', 'stop']) {
      expect(classifyParam(p), p).toBe('cosmetic')
    }
  })

  it('treats an unrecognized parameter as cosmetic', () => {
    // The default matters: it is what preserves today's behavior for every
    // tunable no vendor has invented yet. Being wrong here costs quality; being
    // wrong in the other direction (defaulting to contract) would turn a
    // strippable 400 into a hard failure on paths that work fine today.
    expect(classifyParam('min_p')).toBe('cosmetic')
    expect(classifyParam('repetition_penalty')).toBe('cosmetic')
  })

  it('treats structured-output and tool parameters as contract-bearing', () => {
    // AUDIT 1.2, the whole point: stripping response_format turns a structured
    // request into a prose request that SUCCEEDS, and the caller hands prose to
    // a JSON parser.
    expect(classifyParam('response_format')).toBe('contract')
    expect(classifyParam('tools')).toBe('contract')
    expect(classifyParam('tool_choice')).toBe('contract')
    expect(classifyParam('parallel_tool_calls')).toBe('contract')
    // Guided decoding on a self-hosted server is the same contract by another
    // name, and it is the deployment small-model support is FOR.
    expect(classifyParam('guided_json')).toBe('contract')
  })

  it('refuses to strip the parameters that make a response readable at all', () => {
    // model/messages were already hand-refused. `stream` joins them: dropping it
    // yields a valid single JSON body that an SSE pump will wait on forever.
    expect(classifyParam('model')).toBe('protected')
    expect(classifyParam('messages')).toBe('protected')
    expect(classifyParam('stream')).toBe('protected')
  })

  it('maps every contract parameter to the capability its rejection disproves', () => {
    expect(CONTRACT_PARAMS.response_format).toBe('json')
    expect(CONTRACT_PARAMS.tools).toBe('tools')
    expect(CONTRACT_PARAMS.tool_choice).toBe('tools')
    expect(CONTRACT_PARAMS.parallel_tool_calls).toBe('tools')
    expect(isContractParam('temperature')).toBe(false)
  })
})

// ── Reading the 400 ──────────────────────────────────────────────────────────

describe('rejectedParam', () => {
  it('still matches the three real-world 400 phrasings it shipped against', () => {
    // These are the regressions that would silently disable the learner: the
    // call would keep 400ing forever with nothing in the logs saying why.
    expect(rejectedParam('{"error":{"message":"`temperature` is deprecated for this model"}}')).toBe('temperature')
    expect(rejectedParam('{"error":{"message":"Unsupported parameter: \'top_p\'"}}')).toBe('top_p')
    expect(rejectedParam('{"error":{"message":"\'seed\' is not supported by this model"}}')).toBe('seed')
  })

  it('prefers the provider’s own `param` field over any reading of its prose', () => {
    // VERBATIM FROM THE RUN THAT FOUND IT. The message names `reasoning_effort`
    // three times and quotes it none of them, so every prose pattern in the
    // function misses it — and 28 cases across four harnesses were filed as
    // "could not reach this model" on an endpoint that was answering fine.
    const body = JSON.stringify({
      error: {
        message:
          "Function tools with reasoning_effort are not supported for gpt-5.6-terra in /v1/chat/completions. To use function tools, use /v1/responses or set reasoning_effort to 'none'.",
        type: 'invalid_request_error',
        param: 'reasoning_effort',
        code: null,
      },
    })
    expect(rejectedParam(body)).toBe('reasoning_effort')
    expect(classifyParam('reasoning_effort')).toBe('cosmetic')
  })

  it('reduces a dotted `param` path to the thing we can stop sending', () => {
    expect(rejectedParam('{"error":{"param":"response_format.json_schema.strict"}}')).toBe('response_format')
  })

  it('still refuses to strip a load-bearing parameter the provider names', () => {
    expect(classifyParam(rejectedParam('{"error":{"param":"messages"}}') ?? '')).toBe('protected')
  })

  it('names the parameter in OpenAI’s two unquoted phrasings', () => {
    // A PARAMETER THE RATCHET CANNOT NAME IS ONE IT NEVER STOPS SENDING, so the
    // endpoint 400s on every call for as long as the default is configured.
    // `reasoning` is the one that reaches us: a legitimate OpenRouter request
    // default, forwarded to an OpenAI endpoint that refuses it.
    expect(rejectedParam('Unrecognized request argument supplied: reasoning')).toBe('reasoning')
    expect(rejectedParam("Unknown parameter: 'reasoning'.")).toBe('reasoning')
    // And the quoted-dotted shape OpenAI uses for nested ones, which the
    // existing pattern already reaches — asserted so a rewrite cannot lose it.
    expect(rejectedParam("Unsupported parameter: 'reasoning.effort' is not supported with this model.")).toBe('reasoning')
  })

  it('reads a VALUE complaint as the parameter complaint it is', () => {
    // OpenAI's reasoning models phrase it "does not support", not "is not
    // supported", so every pattern written for the passive form missed it —
    // `temperature` was never learned and every harness that declares one 400'd
    // on that endpoint for ever. Dropping it is right: the model then runs at
    // its default, which is the only value it has.
    expect(rejectedParam("Unsupported value: 'temperature' does not support 0.2 with this model. Only the default (1) is supported.")).toBe('temperature')
    expect(classifyParam('temperature')).toBe('cosmetic')
  })

  it('will not let a widened pattern strip something load-bearing', () => {
    // The patterns are read by `classifyParam`, which refuses to remove `model`,
    // `messages` or `stream` however the upstream phrases its complaint. Stated
    // here because the two additions above are the loosest in the file.
    expect(classifyParam(rejectedParam('Unrecognized request argument supplied: messages') ?? '')).toBe('protected')
    expect(classifyParam(rejectedParam("Unknown parameter: 'stream'.") ?? '')).toBe('protected')
  })

  it('reads response_format out of a rejection, so it can be classified rather than stripped blind', () => {
    expect(rejectedParam('{"error":{"message":"`response_format` is not supported"}}')).toBe('response_format')
  })

  it('stays null on a 400 about something else', () => {
    expect(rejectedParam('{"error":{"message":"context length exceeded"}}')).toBeNull()
    expect(rejectedParam('')).toBeNull()
  })
})

// ── The persisted store ──────────────────────────────────────────────────────

describe('readLearnedParams', () => {
  it('reads the current shape and keeps entries inside the TTL', () => {
    const { byKey, changed } = readLearnedParams({ [KEY]: { temperature: iso(2 * DAY) } }, NOW)
    expect([...(byKey.get(KEY) ?? new Map()).keys()]).toEqual(['temperature'])
    expect(byKey.get(KEY)?.get('temperature')).toBe(NOW - 2 * DAY)
    expect(changed).toBe(false) // nothing normalized — don't churn the settings row
  })

  it('drops an entry past the 30-day TTL', () => {
    // The release valve on the ratchet: a provider that fixed support gets
    // re-tried without an admin ever touching anything.
    const { byKey, changed } = readLearnedParams({ [KEY]: { response_format: iso(LEARNED_PARAM_TTL_MS + 1000) } }, NOW)
    expect(byKey.has(KEY)).toBe(false)
    expect(changed).toBe(true)
  })

  it('keeps an entry one minute short of the TTL', () => {
    const { byKey } = readLearnedParams({ [KEY]: { top_p: iso(LEARNED_PARAM_TTL_MS - 60_000) } }, NOW)
    expect(byKey.get(KEY)?.has('top_p')).toBe(true)
  })

  it('adopts a legacy un-timestamped entry as fresh from now, and asks to be persisted', () => {
    // Every existing install stores Record<string, string[]>. Reading it as
    // expired would throw away real learnings and re-pay every 400; reading it
    // as permanent would reinstate the ratchet. Fresh-from-now is the only
    // honest reading — and `changed` is how the clock actually starts, because
    // a stamp that only lives in memory restarts with the process.
    const { byKey, changed } = readLearnedParams({ [KEY]: ['temperature', 'top_p'] }, NOW)
    expect([...(byKey.get(KEY) ?? new Map()).keys()]).toEqual(['temperature', 'top_p'])
    expect(byKey.get(KEY)?.get('temperature')).toBe(NOW)
    expect(changed).toBe(true)

    // And once persisted it is timestamped, so the NEXT read can expire it.
    const stored = writeLearnedParams(byKey, NOW)
    expect(stored[KEY]?.temperature).toBe(new Date(NOW).toISOString())
    const later = readLearnedParams(stored, NOW + LEARNED_PARAM_TTL_MS + 1000)
    expect(later.byKey.has(KEY)).toBe(false)
  })

  it('drops a corrupt timestamp rather than granting it a fresh 30 days', () => {
    // Distinct from the legacy shape, which is an array and recognizable as one.
    // This is a writer that got the format wrong, and re-stamping it would hand
    // an unknown writer a permanent strip.
    const { byKey, changed } = readLearnedParams({ [KEY]: { temperature: 'yesterday', top_p: iso(DAY) } }, NOW)
    expect([...(byKey.get(KEY) ?? new Map()).keys()]).toEqual(['top_p'])
    expect(changed).toBe(true)
  })

  it('survives anything at all in the settings row', () => {
    // app_settings is JSON that outlives the code that wrote it, and this runs
    // on the path to every upstream call.
    expect(readLearnedParams(null, NOW).byKey.size).toBe(0)
    expect(readLearnedParams('nonsense', NOW).byKey.size).toBe(0)
    expect(readLearnedParams([1, 2, 3], NOW).byKey.size).toBe(0)
    expect(readLearnedParams({ [KEY]: 42 }, NOW).byKey.size).toBe(0)
    expect(readLearnedParams({ [KEY]: {} }, NOW).byKey.size).toBe(0)
  })
})

describe('writeLearnedParams', () => {
  it('prunes expired entries on the way out, so any write is also a cleanup', () => {
    const byKey = map({
      [KEY]: { temperature: NOW - DAY, response_format: NOW - LEARNED_PARAM_TTL_MS - 1 },
      'openrouter:qwen3-14b': { top_p: NOW - LEARNED_PARAM_TTL_MS - 1 },
    })
    const stored = writeLearnedParams(byKey, NOW)
    expect(Object.keys(stored)).toEqual([KEY])
    expect(Object.keys(stored[KEY] ?? {})).toEqual(['temperature'])
  })
})

describe('activeLearnedParams', () => {
  it('returns the survivors and forgets the expired in place', () => {
    const byKey = map({ [KEY]: { temperature: NOW - DAY, response_format: NOW - LEARNED_PARAM_TTL_MS - 1 } })
    const { params, expired } = activeLearnedParams(byKey, KEY, NOW)
    expect(params).toEqual(['temperature'])
    expect(expired).toBe(true)
    // Deleted, not merely filtered: the next 400 is allowed to re-learn it from
    // a provider that may since have fixed support.
    expect(byKey.get(KEY)?.has('response_format')).toBe(false)
  })

  it('drops the key entirely once its last learning expires', () => {
    const byKey = map({ [KEY]: { response_format: NOW - LEARNED_PARAM_TTL_MS - 1 } })
    expect(activeLearnedParams(byKey, KEY, NOW).params).toEqual([])
    expect(byKey.has(KEY)).toBe(false)
  })

  it('reports no expiry for an untouched or unknown key, so the caller does not churn app_settings', () => {
    const byKey = map({ [KEY]: { temperature: NOW - DAY } })
    expect(activeLearnedParams(byKey, KEY, NOW)).toEqual({ params: ['temperature'], expired: false })
    expect(activeLearnedParams(byKey, 'other:model', NOW)).toEqual({ params: [], expired: false })
  })

  it('keeps learnings for one endpoint:model out of another', () => {
    // Same model id behind a quantized local build and the vendor's own API
    // genuinely differ in what they accept; a fact from one must never be
    // credited to the other.
    const byKey = map({ [KEY]: { temperature: NOW - DAY } })
    expect(activeLearnedParams(byKey, 'openrouter:qwen3-14b', NOW).params).toEqual([])
  })
})

describe('a provider that reports a field path instead of a quoted name', () => {
  // Every one of these is a real Anthropic 400 from a live sweep, and not one of
  // them matched the original patterns — so `response_format` was never stripped,
  // the call was never retried, and the fitness suite scored the 400 as the
  // MODEL failing its contract on every structured harness.
  const real = [
    `{"error":{"code":"invalid_request_error","message":"response_format.type: Input should be 'json_schema'","type":"invalid_request_error"}}`,
    `{"error":{"message":"response_format.json_schema.strict: Input should be True"}}`,
    `{"error":{"message":"response_format.json_schema.schema: Empty schema ({}) that accepts any JSON value is not supported. Please specify a concrete type."}}`,
  ]

  it('names the ROOT parameter, which is the one we can stop sending', () => {
    // Not `response_format.json_schema.strict` — that is a field inside a
    // parameter, and the thing a retry can drop is the parameter.
    for (const text of real) expect(rejectedParam(text), text.slice(0, 60)).toBe('response_format')
  })

  it('still refuses to strip a protected parameter reported the same way', () => {
    // A complaint about the message list must never become a request with no
    // messages. `classifyParam` is what holds that, and this is the shape that
    // would have reached it.
    expect(classifyParam(rejectedParam(`{"error":{"message":"messages.0.content: Field required"}}`) ?? '')).toBe('protected')
  })

  it('does not fire on ordinary prose that happens to contain a colon', () => {
    expect(rejectedParam('Rate limited: please retry after 20 seconds')).toBeNull()
    expect(rejectedParam('{"error":{"message":"Internal server error"}}')).toBeNull()
  })
})
