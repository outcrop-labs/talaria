import { describe, expect, it } from 'vitest'
import { resultsFromPayload, searchTheWeb, type WebSearchDeps } from './web-search'
import type { McpServer } from './mcp-registry'
import type { PlatformSupply } from './capability-reach'

// THE QUESTION THIS FILE PINS DOWN is the one that made Hermes agents the worst
// -served callers in the product: "how does this deployment search" had two
// answers, and the endpoint every containerized persona reaches was wired to the
// weaker one. There is one answer now, and these are its edges.

const exa = (over: Partial<McpServer> = {}): McpServer =>
  ({
    id: 'id',
    name: 'exa',
    label: 'Exa',
    description: null,
    url: 'https://example.invalid/mcp',
    headers: {},
    timeoutSecs: null,
    enabled: true,
    allAgents: true,
    authMode: 'org',
    tools: [{ name: 'web_search', description: 'Search the live web.' }],
    toolsRefreshedAt: null,
    requiredHeaders: [],
    oauthEnabled: false,
    builtin: false,
    appSlug: null,
    createdBy: null,
    createdAt: '2026-08-11T00:00:00.000Z',
    ...over,
  }) as McpServer

const OURS: PlatformSupply[] = [{ capability: 'search', server: 'talaria', tool: 'web_search' }]

const deps = (over: Partial<WebSearchDeps> = {}): Partial<WebSearchDeps> => ({
  servers: async () => [],
  providers: async () => ({}),
  platform: async () => [],
  callTool: async () => ({ text: '', structured: null }),
  own: async () => [],
  ...over,
})

describe('searchTheWeb', () => {
  it('PREFERS a registered provider over our own engine', async () => {
    // THE WHOLE POINT. An org that went and registered Exa should get Exa in the
    // agents doing real work, not only in the research pipeline. Before this
    // resolver, `/api/search` — the endpoint behind every Hermes `web_search` —
    // could not reach a registered provider at all.
    let ownCalled = 0
    const out = await searchTheWeb('postgres 17', {
      deps: deps({
        servers: async () => [exa()],
        platform: async () => OURS,
        own: async () => {
          ownCalled++
          return []
        },
        callTool: async (server, tool, args) => ({
          text: '',
          structured: [{ title: 'Release notes', url: 'https://postgresql.org/docs/release/', snippet: `${server}/${tool} for ${String(args.query)}` }],
        }),
      }),
    })
    expect(out.via).toEqual({ server: 'exa', tool: 'web_search' })
    expect(out.results[0]?.url).toBe('https://postgresql.org/docs/release/')
    expect(ownCalled).toBe(0)
  })

  it('falls back to OUR engine when nothing is registered, so a fresh install still searches', async () => {
    // SearXNG is the floor, not the ceiling. A zero-config install has working
    // web search on day one and upgrades the moment an admin registers a
    // provider — with no code change and nothing to migrate.
    const out = await searchTheWeb('postgres 17', {
      deps: deps({
        platform: async () => OURS,
        own: async () => [{ title: 'Release notes', url: 'https://postgresql.org/docs/release/', snippet: '', engine: 'mojeek' }],
      }),
    })
    expect(out.via).toBeNull()
    expect(out.results[0]?.engine).toBe('mojeek')
  })

  it('does NOT route our own engine through callMcpTool', async () => {
    // The bug that made the platform supplier a lie the first time. Talaria is
    // in nobody's MCP registry, so a platform supplier sent to `callMcpTool`
    // comes back `MCP server "talaria" is not registered` — which the tool loop
    // then hands to the model as the search RESULT.
    let viaMcp = 0
    await searchTheWeb('q', {
      deps: deps({
        platform: async () => OURS,
        callTool: async () => {
          viaMcp++
          return { text: '', structured: null }
        },
        own: async () => [{ title: 't', url: 'https://a.example', snippet: '', engine: 'mojeek' }],
      }),
    })
    expect(viaMcp).toBe(0)
  })

  it('REFUSES with a sentence when nothing can search, rather than returning empty', async () => {
    // A model handed an empty result set answers from memory in a confident
    // voice; a model handed this sentence says it could not search. The error
    // text goes straight into an agent's transcript, so it is written for one.
    await expect(searchTheWeb('q', { deps: deps() })).rejects.toThrow(/could not search rather than answering from memory/)
  })

  it('honours an admin who says nothing supplies search here', async () => {
    // An explicit null in `capability_providers` is a deliberate answer, and it
    // has to silence our own engine too — otherwise "nothing supplies this" is
    // a setting that does not do what it says.
    await expect(
      searchTheWeb('q', { deps: deps({ servers: async () => [exa()], platform: async () => OURS, providers: async () => ({ search: null }) }) }),
    ).rejects.toThrow(/not available in this workspace/)
  })
})

describe('resultsFromPayload', () => {
  it('reads a provider that spells its fields differently', async () => {
    // No provider owes us SearXNG's field names, and a normaliser that only
    // understood one shape would return nothing for the others — silently,
    // which reads as "the web had no answer".
    const rows = resultsFromPayload({ data: { hits: [{ name: 'A', link: 'https://a.example', description: 'about a' }] } }, 'exa')
    expect(rows).toEqual([{ title: 'A', url: 'https://a.example', snippet: 'about a', engine: 'exa' }])
  })

  it('drops anything that cannot be cited, and never the same URL twice', async () => {
    const rows = resultsFromPayload([{ title: 'no url' }, { url: 'https://a.example' }, { url: 'https://a.example', title: 'dupe' }], 'exa')
    expect(rows.map((r) => r.url)).toEqual(['https://a.example'])
    // A result with no title still has to be usable, so the URL stands in.
    expect(rows[0]?.title).toBe('https://a.example')
  })
})
