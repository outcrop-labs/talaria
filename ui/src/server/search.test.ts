import { describe, expect, it } from 'vitest'
import { resultsFrom, searchReachable, searchWeb } from './search'

// The client for Talaria's own search engine. What matters here is not the happy
// path — it is that a payload from a dozen aggregated engines cannot break the
// search, and that a failure comes back as a sentence a MODEL can act on: this
// error text goes straight into an agent's transcript.

const payload = (results: unknown[]) => ({ results })
const reply = (body: unknown, status = 200): Response =>
  new Response(JSON.stringify(body), { status, headers: { 'content-type': 'application/json' } })

const deps = (res: Response | (() => Promise<Response>)) => ({
  url: async () => 'http://search.test',
  fetch: (typeof res === 'function' ? res : async () => res) as unknown as typeof globalThis.fetch,
})

describe('resultsFrom', () => {
  it('takes what SearXNG returns and drops what cannot be cited', () => {
    // A result with no URL cannot be cited, and an uncitable result in a
    // research pipeline is worse than one fewer result — `ungrounded_ref`
    // grounds every claim against exactly these.
    const out = resultsFrom(
      payload([
        { title: 'A', url: 'https://a.example', content: 'first', engine: 'duckduckgo' },
        { title: 'No url', content: 'unciteable' },
        { title: 'B', url: 'https://b.example', content: 'second', engine: 'brave' },
      ]),
    )
    expect(out).toEqual([
      { title: 'A', url: 'https://a.example', snippet: 'first', engine: 'duckduckgo' },
      { title: 'B', url: 'https://b.example', snippet: 'second', engine: 'brave' },
    ])
  })

  it('deduplicates by URL, because a metasearch returns the same page from several engines', () => {
    const out = resultsFrom(
      payload([
        { title: 'A', url: 'https://a.example', content: 'from google', engine: 'google' },
        { title: 'A', url: 'https://a.example', content: 'from bing', engine: 'bing' },
      ]),
    )
    expect(out).toHaveLength(1)
  })

  it('survives a single engine returning something odd', () => {
    // The payload is an aggregate of a dozen engines. One of them answering with
    // nonsense must cost that result, never the search.
    expect(resultsFrom(payload([null, 'a string', 42, { url: 'https://ok.example' }]))).toEqual([
      { title: 'https://ok.example', url: 'https://ok.example', snippet: '', engine: 'unknown' },
    ])
    expect(resultsFrom(null)).toEqual([])
    expect(resultsFrom({ results: 'not an array' })).toEqual([])
  })

  it('never emits a null snippet', () => {
    // Every consumer concatenates it, and a null in that position becomes the
    // literal string "null" in a prompt.
    expect(resultsFrom(payload([{ url: 'https://a.example' }]))[0]?.snippet).toBe('')
  })
})

describe('searchReachable', () => {
  it('reports the engine\'s own sentence, not a generic failure', async () => {
    // What an admin reads here has to be what the AGENT got — otherwise the
    // admin surface and the model's excuse describe two different problems.
    const dead = async () => {
      throw new Error('ECONNREFUSED')
    }
    const out = await searchReachable(deps(dead))
    expect(out).toMatchObject({ ok: false, url: 'http://search.test' })
    expect(out.error).toContain('did not answer')
  })

  it('is ok when the engine actually finds something', async () => {
    const out = await searchReachable(deps(reply(payload([{ url: 'https://a.example' }]))))
    expect(out).toMatchObject({ ok: true, error: null })
  })

  it('is NOT ok when the engine answers cheerfully and finds nothing', async () => {
    // THE STATE A FRESH SEARXNG SHIPS IN. Every default general engine CAPTCHA-
    // walls a self-hosted instance, so the request is a clean HTTP 200 with
    // valid JSON and `results: []` — and the old check, which only asked whether
    // `searchWeb` threw, called that healthy. `platformSupply` then advertised a
    // `web_search` tool that could not search, and models handed empty result
    // sets answered from memory in a confident voice with no sources.
    const out = await searchReachable(deps(reply(payload([]))))
    expect(out.ok).toBe(false)
    // The sentence has to name the real cause and hand over the command that
    // proves it, because "search returned nothing" reads as a query problem.
    expect(out.error).toContain('unresponsive_engines')
  })
})

describe('searchWeb', () => {
  it('honours the limit and caps it', async () => {
    const many = Array.from({ length: 40 }, (_, i) => ({ title: `t${i}`, url: `https://e${i}.example`, content: '', engine: 'x' }))
    expect(await searchWeb('anything', { limit: 3, deps: deps(reply(payload(many))) })).toHaveLength(3)
    expect(await searchWeb('anything', { limit: 999, deps: deps(reply(payload(many))) })).toHaveLength(25)
  })

  it('names the JSON-format misconfiguration exactly, because a bare 403 misleads', async () => {
    // SearXNG ships with JSON off and answers a request for it with 403 — not a
    // 404, not an auth error. Left generic, that sends an operator looking for a
    // credentials problem that does not exist.
    await expect(searchWeb('x', { deps: deps(reply({}, 403)) })).rejects.toThrow(/search\.formats/)
  })

  it('turns an unreachable engine into an instruction rather than a stack trace', async () => {
    // This sentence lands in an agent's transcript. "fetch failed" is a dead
    // end; telling the model to say search is unavailable instead of answering
    // from memory is the behaviour we actually want out of it.
    const dead = async () => {
      throw new Error('ECONNREFUSED')
    }
    await expect(searchWeb('x', { deps: deps(dead) })).rejects.toThrow(/did not answer.*rather than answering from memory/s)
  })

  it('refuses an empty query before it costs an upstream request', async () => {
    await expect(searchWeb('   ', { deps: deps(reply(payload([]))) })).rejects.toThrow(/needs a query/)
  })
})
