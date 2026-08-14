import { describe, expect, it } from 'vitest'
import { reachFor, supplierFor, type CapabilityProviders, type ReachDeps } from '@/server/capability-reach'
import type { McpServer } from '@/server/mcp-registry'
import type { Capability, CapabilityFact } from '@/server/harness/capability'

// THE QUESTION THIS FILE PINS DOWN is the one the fitness page used to get
// wrong: not "can the model do it" but "can the RUN do it". A model measured at
// 100% tool calling, with a web-search server registered, reaches search — and
// reporting it Not-a-fit for Research on the strength of `search: false` cost an
// admin a capable model for no reason.

const server = (over: Partial<McpServer> = {}): McpServer =>
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
    tools: [],
    toolsRefreshedAt: null,
    requiredHeaders: [],
    oauthEnabled: false,
    builtin: false,
    appSlug: null,
    createdBy: null,
    createdAt: '2026-08-07T00:00:00.000Z',
    ...over,
  }) as McpServer

const searchServer = (over: Partial<McpServer> = {}): McpServer =>
  server({ tools: [{ name: 'web_search', description: 'Search the live web for current information.' }], ...over })

const fact = (value: boolean): CapabilityFact => ({ value, source: 'probe', at: '2026-08-07T00:00:00.000Z' })

const deps = (over: Partial<ReachDeps> = {}): Partial<ReachDeps> => ({
  servers: async () => [],
  providers: async () => ({}),
  capabilities: async () => ({}),
  ...over,
})

const KEY = 'openrouter:deepseek/deepseek-v4-flash'

describe('supplierFor', () => {
  it('finds a web-search tool by name, corroborated by its description', () => {
    expect(supplierFor('search', [searchServer()])).toEqual({ server: 'exa', tool: 'web_search' })
  })

  it('does NOT mistake Talaria’s own RAG tool for live web search', () => {
    // `search_knowledge` searches the org's indexed documents. A substring match
    // on "search" would read it as the live web and let a research run answer
    // from the company wiki while claiming to have browsed.
    const rag = server({ name: 'talaria', tools: [{ name: 'search_knowledge', description: 'Search the org knowledge base.' }] })
    expect(supplierFor('search', [rag])).toBeNull()
  })

  it('ignores a disabled server', () => {
    expect(supplierFor('search', [searchServer({ enabled: false })])).toBeNull()
  })

  it('needs a corroborating word when the tool describes itself', () => {
    // A same-named tool that searches an intranet is a false positive, and the
    // description is the only thing that can tell them apart.
    const intranet = searchServer({ tools: [{ name: 'web_search', description: 'Search the internal SharePoint index.' }] })
    expect(supplierFor('search', [intranet])).toBeNull()
    // With no description at all, the name is all there is and it is enough.
    expect(supplierFor('search', [searchServer({ tools: [{ name: 'web_search' }] })])).toEqual({ server: 'exa', tool: 'web_search' })
  })

  it('lets an admin name a supplier the heuristic would never find', () => {
    const odd = server({ name: 'house', tools: [{ name: 'q' }] })
    const providers: CapabilityProviders = { search: { server: 'house', tool: 'q' } }
    expect(supplierFor('search', [odd], providers)).toEqual({ server: 'house', tool: 'q' })
  })

  it('lets an admin say NOTHING supplies it, overriding a match', () => {
    // An explicit null is a deliberate answer and not the same as silence.
    expect(supplierFor('search', [searchServer()], { search: null })).toBeNull()
  })

  it('ignores an admin pin whose server or tool has since gone away', () => {
    expect(supplierFor('search', [], { search: { server: 'gone', tool: 'web_search' } })).toBeNull()
    expect(supplierFor('search', [server({ name: 'exa', tools: [] })], { search: { server: 'exa', tool: 'web_search' } })).toBeNull()
  })

  it('supplies nothing for a capability no tool can stand in for', () => {
    // Code and long-context are deliberately absent from the table: a sandbox is
    // not a programmer, and a tool cannot widen a context window. VISION IS NO
    // LONGER ON THIS LIST — `describe_image` reads an image with the model the
    // org assigned to the vision role, which is a real answer to "can this
    // deployment do it" even when the model in the slot cannot.
    for (const cap of ['code', 'long-context', 'json'] as Capability[]) {
      expect(supplierFor(cap, [searchServer()])).toBeNull()
    }
  })

  it('finds Talaria’s own image reader for a model that cannot see', () => {
    const talaria = server({
      name: 'talaria',
      tools: [{ name: 'describe_image', description: "Read an image you cannot see: attaches it to this workspace's vision model and returns a description in text." }],
    })
    expect(supplierFor('vision', [talaria])).toEqual({ server: 'talaria', tool: 'describe_image' })
  })

  it('needs the description to corroborate, the same as search does', () => {
    // `vision` is an ordinary English word and `ocr` is a narrow one, so the name
    // alone is weaker evidence here than it is for `web_search`. A tool that
    // talks about images matches; one that happens to share a name does not.
    const reader = server({ name: 'house', tools: [{ name: 'ocr', description: 'Extract text from a scanned image or photo.' }] })
    expect(supplierFor('vision', [reader])).toEqual({ server: 'house', tool: 'ocr' })

    const unrelated = server({ name: 'house', tools: [{ name: 'vision', description: 'Company vision and mission statements.' }] })
    expect(supplierFor('vision', [unrelated])).toBeNull()
  })
})

describe('reachFor', () => {
  it('reaches search natively when the model browses', async () => {
    const out = await reachFor([KEY], ['search'], deps({ capabilities: async () => ({ search: fact(true) }) }))
    expect(out['search']).toMatchObject({ reached: true, via: 'native' })
  })

  it('reaches search BY TOOL when the model does not browse but can call tools', async () => {
    // THE CASE THE WHOLE CHANGE IS FOR. deepseek-v4-flash: search false, tools
    // true, and a web-search server in the registry. It can do research.
    const out = await reachFor([KEY], ['search'], deps({ servers: async () => [searchServer()], capabilities: async () => ({ search: fact(false), tools: fact(true) }) }))
    expect(out['search']).toMatchObject({ reached: true, via: 'tool', supplier: { server: 'exa', tool: 'web_search' } })
  })

  it('prefers a REAL search tool over a native search claim, because nothing here ever asks a model to browse', async () => {
    // The bug this ordering fixes. deepseek-v4-pro was recorded `search: true`,
    // so the run took the native path — a plain completion, no
    // `web_search_options`, no plugin — and the model answered from memory. The
    // registry ended empty on a box where SearXNG was up and answering the very
    // queries the run had planned.
    const out = await reachFor(
      [KEY],
      ['search'],
      deps({ servers: async () => [searchServer()], capabilities: async () => ({ search: fact(true), tools: fact(true) }) }),
    )
    expect(out['search']).toMatchObject({ reached: true, via: 'tool', supplier: { server: 'exa', tool: 'web_search' } })
  })

  it('leaves a browsing model on the native path when the admin says nothing supplies search here', async () => {
    // The escape hatch for a sonar model, whose own index beats looping it
    // through a web-search tool: pinning `search` to null is an admin saying
    // nothing supplies it in this install, and native is all that is left.
    const out = await reachFor(
      [KEY],
      ['search'],
      deps({
        servers: async () => [searchServer()],
        providers: async (): Promise<CapabilityProviders> => ({ search: null }),
        capabilities: async () => ({ search: fact(true), tools: fact(true) }),
      }),
    )
    expect(out['search']).toMatchObject({ reached: true, via: 'native' })
  })

  it('keeps a model that can SEE on the native path even with describe_image registered', async () => {
    // Only `search` prefers its tool. Captioning is lossier than a model reading
    // the image itself, so vision must not be demoted the same way.
    const eyes = server({ name: 'talaria', tools: [{ name: 'describe_image', description: 'Describe an image.' }] })
    const out = await reachFor([KEY], ['vision'], deps({ servers: async () => [eyes], capabilities: async () => ({ vision: fact(true), tools: fact(true) }) }))
    expect(out['vision']).toMatchObject({ reached: true, via: 'native' })
  })

  it('reaches by tool on a model nothing has measured — unknown is not false', async () => {
    const out = await reachFor([KEY], ['search'], deps({ servers: async () => [searchServer()] }))
    expect(out['search']).toMatchObject({ reached: true, via: 'tool' })
  })

  it('does NOT reach by tool when the model cannot call tools', async () => {
    // A search server in front of a model that cannot hold a tool call is not
    // reach — it is a model answering from memory with a tool sitting unused
    // beside it, which is the exact failure the search floor exists to prevent.
    const out = await reachFor([KEY], ['search'], deps({ servers: async () => [searchServer()], capabilities: async () => ({ search: fact(false), tools: fact(false) }) }))
    expect(out['search']?.reached).toBe(false)
    expect(out['search']?.detail).toContain('unable to call tools')
  })

  it('does not reach search with no tool and no native support, and says what to install', async () => {
    const out = await reachFor([KEY], ['search'], deps({ capabilities: async () => ({ search: fact(false) }) }))
    expect(out['search']?.reached).toBe(false)
    // The sentence has to name the org's next move, not blame the model.
    expect(out['search']?.detail).toContain('Register one')
  })

  it('requires EVERY endpoint in the pool to support it natively', async () => {
    // A bare id can land on any member, so a native claim has to hold for the
    // worst of them; the tool path is what rescues the mixed case.
    const facts: Record<string, Partial<Record<Capability, CapabilityFact>>> = {
      a: { search: fact(true) },
      b: { search: fact(false) },
    }
    const out = await reachFor(['a', 'b'], ['search'], deps({ capabilities: async (k) => facts[k] ?? {} }))
    expect(out['search']).toMatchObject({ reached: false })
  })

  it('does not read the registry for a capability no tool can supply', async () => {
    // `code` HAS NO SUPPLIER RULE and cannot get one: writing code is what the
    // model does, not something a tool can do on its behalf. (`vision` used to
    // stand here and no longer can — `describe_image` supplies it now, which is
    // the whole point of `TOOL_REACHABLE`.)
    let asked = 0
    const out = await reachFor(
      [KEY],
      ['code'],
      deps({
        servers: async () => {
          asked++
          return []
        },
        capabilities: async () => ({ code: fact(false) }),
      }),
    )
    expect(asked).toBe(0)
    expect(out['code']).toMatchObject({ reached: false, via: null })
    expect(out['code']?.detail).toContain('nothing can supply it')
  })

  it('survives a registry that is down rather than claiming reach', async () => {
    const out = await reachFor(
      [KEY],
      ['search'],
      deps({
        servers: async () => {
          throw new Error('db down')
        },
        capabilities: async () => ({ search: fact(false) }),
      }),
    )
    expect(out['search']?.reached).toBe(false)
  })

  it('asks nothing when nothing is wanted', async () => {
    expect(await reachFor([KEY], [], deps())).toEqual({})
  })
})
