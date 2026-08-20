import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

// The marketplace's serving strategy, tested at the module boundary: the
// registry is faked (no network), and every assertion is about THIS file's
// accounting — the concurrency bound, the stale-while-revalidate shelf, the
// shared publisher cache — not about what the real registry happens to say
// today.

/** A first-party registry entry for a domain, description overridable (the
 *  stale-while-revalidate test flips it OLD → NEW). */
const entryFor = (domain: string, description = `OLD ${domain}`) => {
  const labels = domain.split('.')
  const ns = [...labels].reverse().join('.')
  return {
    server: {
      name: `${ns}/${labels[0]}-mcp`,
      title: labels[0]!,
      description,
      websiteUrl: `https://${domain}`,
      remotes: [{ type: 'streamable-http', url: `https://mcp.${domain}/mcp` }],
    },
  }
}

const mockNet = vi.hoisted(() => {
  const state = {
    calls: [] as string[],
    registryInFlight: 0,
    registryMaxInFlight: 0,
    /** Per-search registry answer; replace per test. */
    registry: ((_url: URL) => ({ servers: [] })) as (url: URL) => { servers: unknown[] },
    /** well-known answers: an HTTP status number, or { status, body }. */
    wellKnown: ((_url: URL) => 404) as (url: URL) => number | { status: number; body: unknown },
    /** When set, every fetch parks here until released — for proving a caller
     *  did NOT wait for the network. */
    gate: null as Array<() => void> | null,
  }
  // Microtask yields only: these tests run under fake timers, so the mock
  // must never depend on a setTimeout firing.
  const tick = async () => {
    for (let i = 0; i < 4; i++) await Promise.resolve()
  }
  const json = (body: unknown, status = 200) =>
    new Response(JSON.stringify(body), { status, headers: { 'content-type': 'application/json' } })
  const safeFetch = async (input: string | URL): Promise<Response> => {
    const url = new URL(String(input))
    const isRegistry = url.hostname === 'registry.modelcontextprotocol.io'
    state.calls.push(`${url.hostname}${url.pathname}?${url.searchParams.get('search') ?? ''}`)
    // The concurrency measurement has to span the whole request, gate and
    // yield included — bracketing only the response construction would count
    // work that is already done, not work in flight.
    if (isRegistry) {
      state.registryInFlight++
      state.registryMaxInFlight = Math.max(state.registryMaxInFlight, state.registryInFlight)
    }
    try {
      if (state.gate) await new Promise<void>((resolve) => state.gate!.push(resolve))
      await tick()
      if (isRegistry) return json(state.registry(url))
      const wk = state.wellKnown(url)
      return typeof wk === 'number' ? json({}, wk) : json(wk.body, wk.status)
    } finally {
      if (isRegistry) state.registryInFlight--
    }
  }
  return { state, safeFetch }
})

vi.mock('@/server/safe-fetch', () => ({ safeFetch: mockNet.safeFetch }))
vi.mock('@/server/mcp-icons', () => ({ warmIcons: () => {} }))
vi.mock('@/server/scheduler', () => ({ registerJob: () => {} }))

const load = async () => {
  vi.resetModules()
  return await import('@/server/mcp-library')
}

/** A registry where every featured domain has a first-party entry except the
 *  misses given — those exercise the well-known → documented fallbacks. */
const registryWith = (featured: readonly string[], misses: readonly string[] = [], description = 'OLD') => {
  const byTerm = new Map(featured.filter((d) => !misses.includes(d)).map((d) => [d.split('.')[0]!, d]))
  return (url: URL): { servers: unknown[] } => {
    const term = url.searchParams.get('search') ?? ''
    const domain = byTerm.get(term)
    return { servers: domain ? [entryFor(domain, `${description} ${domain}`)] : [] }
  }
}

/** Spin microtasks until cond() — under fake timers with microtask-only
 *  mocks, every background chain lands within a bounded number of turns.
 *  Async conds are awaited, so a poll can read through the module itself. */
const until = async (cond: () => boolean | Promise<boolean>) => {
  for (let i = 0; i < 100_000; i++) if (await cond()) return
  expect(await cond()).toBe(true)
}

describe('mcp-library', () => {
  beforeEach(() => {
    mockNet.state.calls.length = 0
    mockNet.state.registryInFlight = 0
    mockNet.state.registryMaxInFlight = 0
    mockNet.state.gate = null
    mockNet.state.wellKnown = () => 404
    vi.useFakeTimers()
  })
  afterEach(() => {
    vi.useRealTimers()
  })

  it('resolves the featured shelf with bounded registry concurrency', async () => {
    const lib = await load()
    mockNet.state.registry = registryWith(lib.FEATURED_DOMAINS, ['github.com'])
    const { servers } = await lib.featuredMcpLibrary()
    // Every featured publisher resolves; github.com arrives via the
    // documented map (registry miss + well-known 404), the rest via registry.
    expect(servers).toHaveLength(lib.FEATURED_DOMAINS.length)
    expect(servers.every((s) => s.tier === 'first-party')).toBe(true)
    expect(servers.find((s) => s.domain === 'github.com')?.url).toBe('https://api.githubcopilot.com/mcp/')
    // The whole point of mapPool: 32 publishers resolved, never more than 6
    // registry requests in flight — and exactly 6, so the pool ran full
    // rather than collapsing to sequential.
    expect(mockNet.state.registryMaxInFlight).toBe(6)
    const searches = mockNet.state.calls.filter((c) => c.startsWith('registry.modelcontextprotocol.io'))
    expect(searches).toHaveLength(lib.FEATURED_DOMAINS.length)
    expect(mockNet.state.calls).toContain('github.com/.well-known/mcp.json?')
  })

  it('serves a fresh featured shelf with zero upstream calls', async () => {
    const lib = await load()
    mockNet.state.registry = registryWith(lib.FEATURED_DOMAINS, ['github.com'])
    await lib.featuredMcpLibrary()
    mockNet.state.calls.length = 0
    const again = await lib.featuredMcpLibrary()
    expect(again.servers).toHaveLength(lib.FEATURED_DOMAINS.length)
    expect(mockNet.state.calls).toHaveLength(0)
  })

  it('serves a stale shelf immediately and revalidates in the background, single-flighted', async () => {
    const lib = await load()
    mockNet.state.registry = registryWith(lib.FEATURED_DOMAINS, ['github.com'])
    const first = await lib.featuredMcpLibrary()
    expect(first.servers.find((s) => s.domain === 'stripe.com')?.description).toBe('OLD stripe.com')

    // Age past the 1h shelf TTL, change the registry's answer (only stripe
    // and linear still resolve), and gate every upstream fetch so NOTHING can
    // complete until we release it.
    vi.setSystemTime(Date.now() + 61 * 60 * 1000)
    mockNet.state.registry = registryWith(lib.FEATURED_DOMAINS, [
      'github.com',
      ...lib.FEATURED_DOMAINS.filter((d) => d !== 'stripe.com' && d !== 'linear.app'),
    ], 'NEW')
    mockNet.state.gate = []
    const beforeRefresh = mockNet.state.calls.length

    // Two concurrent STALE serves: both answer now, from cache, with the old
    // data — with the network gated, returning at all proves neither awaited.
    const [a, b] = await Promise.all([lib.featuredMcpLibrary(), lib.featuredMcpLibrary()])
    expect(a.servers).toEqual(b.servers)
    expect(a.servers.find((s) => s.domain === 'stripe.com')?.description).toBe('OLD stripe.com')

    // Release the gates and let the one background fan-out finish. Poll on
    // the OUTCOME (the new answer being served), not on fetch counts — the
    // last fetch STARTING is not the shelf BEING REPLACED. The count is
    // asserted after: 32 registry searches + 30 well-known probes, no more —
    // single-flight is what pins it, since two concurrent stale serves
    // without it would double the fan-out.
    mockNet.state.gate.splice(0).forEach((release) => release())
    mockNet.state.gate = null
    await until(async () =>
      (await lib.featuredMcpLibrary()).servers.find((s) => s.domain === 'stripe.com')?.description === 'NEW stripe.com'
    )
    const refreshCalls = lib.FEATURED_DOMAINS.length + lib.FEATURED_DOMAINS.filter((d) => d !== 'stripe.com' && d !== 'linear.app').length
    expect(mockNet.state.calls.length).toBe(beforeRefresh + refreshCalls)
  })

  it('pins a brand search from the warmed publisher cache, not the network', async () => {
    const lib = await load()
    mockNet.state.registry = registryWith(lib.FEATURED_DOMAINS, ['github.com'])
    await lib.featuredMcpLibrary() // warms publisherCache for every featured domain
    mockNet.state.calls.length = 0

    const results = await lib.searchMcpLibrary('stripe')
    // One registry page for the search itself; the stripe.com publisher pin
    // comes from cache (a live resolve would add a search + well-known probe).
    expect(mockNet.state.calls).toHaveLength(1)
    expect(results[0]?.domain).toBe('stripe.com')
    expect(results[0]?.tier).toBe('first-party')
  })
})
