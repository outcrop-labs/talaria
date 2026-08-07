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
