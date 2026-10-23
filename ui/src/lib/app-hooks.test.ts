import { QueryClient } from '@tanstack/svelte-query'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { setAppContext, type AppContext } from './app-context'
import { useAppApi, useAppInvalidate, useAppQuery } from './app-hooks'

// The hooks a surface reads its app through, run outside a component: Svelte's
// context is a plain map here, `createQuery` hands back the options it would
// run (re-evaluated on every read, as svelte-query re-runs the getter), and
// `useQueryClient` is a real QueryClient. What they pin: which of
// useAppQuery's argument forms is which, the surface's slug as the default and
// the error without one, and that every hook keeps the slug it read at init.

const h = vi.hoisted(() => ({
  contexts: new Map<unknown, unknown>(),
  client: null as unknown,
}))

vi.mock('svelte', async (importOriginal) => ({
  ...(await importOriginal<typeof import('svelte')>()),
  getContext: (key: unknown) => h.contexts.get(key),
  setContext: (key: unknown, value: unknown) => void h.contexts.set(key, value),
}))

vi.mock('@tanstack/svelte-query', async (importOriginal) => ({
  ...(await importOriginal<typeof import('@tanstack/svelte-query')>()),
  createQuery: (options: () => unknown) => ({
    get options() {
      return options()
    },
  }),
  useQueryClient: () => h.client,
}))

type Resolved = {
  queryKey: readonly unknown[]
  queryFn: (ctx: { signal: AbortSignal }) => Promise<unknown>
  enabled?: boolean
  staleTime?: number
}
const resolved = (q: unknown): Resolved => (q as { options: Resolved }).options

/** A surface of `slug`, whose slug the host can move under it. */
function mountSurface(slug: string): { move: (to: string) => void } {
  let current = slug
  setAppContext({
    get slug() {
      return current
    },
    surface: 'work',
    manifest: {} as AppContext['manifest'],
    rest: '',
    query: new URLSearchParams(),
  })
  return { move: (to) => (current = to) }
}

const urls: string[] = []

beforeEach(() => {
  h.contexts.clear()
  h.client = new QueryClient()
  vi.stubGlobal(
    'fetch',
    vi.fn(async (url: string) => {
      urls.push(url)
      return new Response('{}', { headers: { 'content-type': 'application/json' } })
    }),
  )
})

afterEach(() => {
  urls.length = 0
  vi.unstubAllGlobals()
})

async function fetched(q: unknown): Promise<string | undefined> {
  await resolved(q).queryFn({ signal: new AbortController().signal })
  return urls.at(-1)
}

describe('useAppQuery inside a surface', () => {
  it('reads every documented argument form against the right app and path', async () => {
    mountSurface('crm')
    let q = 'ada'
    const forms: Array<[string, unknown, string, Partial<Resolved>]> = [
      ["('items')", useAppQuery('items'), '/api/apps/crm/items', {}],
      ["('items', { enabled })", useAppQuery('items', { enabled: false }), '/api/apps/crm/items', { enabled: false }],
      ["('items', () => options)", useAppQuery('items', () => ({ staleTime: 5 })), '/api/apps/crm/items', { staleTime: 5 }],
      ['(() => path)', useAppQuery(() => `items?q=${q}`), '/api/apps/crm/items?q=ada', {}],
      ["(slug, 'meta')", useAppQuery('other-app', 'meta'), '/api/apps/other-app/meta', {}],
      ['(slug, () => path)', useAppQuery('other-app', () => `meta/${q}`), '/api/apps/other-app/meta/ada', {}],
      ["(slug, 'meta', { enabled })", useAppQuery('other-app', 'meta', { enabled: false }), '/api/apps/other-app/meta', { enabled: false }],
      ["(slug, 'meta', () => options)", useAppQuery('other-app', 'meta', () => ({ staleTime: 9 })), '/api/apps/other-app/meta', { staleTime: 9 }],
    ]
    for (const [form, query, url, options] of forms) {
      expect(await fetched(query), form).toBe(url)
      expect(resolved(query), form).toMatchObject(options)
    }
    expect(resolved(forms[3]![1]).queryKey).toEqual(['app', 'crm', 'items', { q: 'ada' }])
    expect(resolved(forms[4]![1]).queryKey).toEqual(['app', 'other-app', 'meta'])

    // A getter path re-keys the read as its state changes.
    q = 'bob'
    expect(resolved(forms[3]![1]).queryKey).toEqual(['app', 'crm', 'items', { q: 'bob' }])
    expect(await fetched(forms[5]![1])).toBe('/api/apps/other-app/meta/bob')
  })

  it('keeps the slug it read at init when the host moves the surface', async () => {
    const surface = mountSurface('crm')
    const items = useAppQuery('items')
    const api = useAppApi()
    const invalidate = useAppInvalidate()
    const qc = h.client as QueryClient
    qc.setQueryData(['app', 'crm', 'things'], {})
    qc.setQueryData(['app', 'billing', 'things'], {})

    surface.move('billing')

    expect(resolved(items).queryKey).toEqual(['app', 'crm', 'items'])
    expect(await fetched(items)).toBe('/api/apps/crm/items')
    await api.post('items', { title: 'a' })
    expect(urls.at(-1)).toBe('/api/apps/crm/items')
    expect(api.url('events')).toBe('/api/apps/crm/events')
    await invalidate('things')
    expect(qc.getQueryState(['app', 'crm', 'things'])?.isInvalidated).toBe(true)
    expect(qc.getQueryState(['app', 'billing', 'things'])?.isInvalidated).toBe(false)
  })
})

describe('useAppInvalidate', () => {
  it("defaults to the surface's app: everything, or every read under a path", async () => {
    mountSurface('crm')
    const qc = h.client as QueryClient
    const keys = [['app', 'crm', 'things'], ['app', 'crm', 'things', { q: 'ada' }], ['app', 'crm', 'notes'], ['app', 'billing', 'things']]
    for (const key of keys) qc.setQueryData(key, {})
    const stale = () => keys.filter((key) => qc.getQueryState(key)?.isInvalidated).map((key) => JSON.stringify(key))

    await useAppInvalidate()('things')
    expect(stale()).toEqual(['["app","crm","things"]', '["app","crm","things",{"q":"ada"}]'])
    await useAppInvalidate()()
    expect(stale()).toEqual(['["app","crm","things"]', '["app","crm","things",{"q":"ada"}]', '["app","crm","notes"]'])
    await useAppInvalidate('billing')()
    expect(stale()).toHaveLength(4)
  })
})

describe('outside an app surface', () => {
  it('needs the slug, and says so', () => {
    expect(() => useAppQuery('items')).toThrow('useAppQuery(path) outside an app surface needs the slug first: useAppQuery(slug, path)')
    expect(() => useAppQuery(() => 'items', { enabled: false })).toThrow(/needs the slug first/)
    expect(() => useAppInvalidate()).toThrow("useAppInvalidate() outside an app surface needs the app's slug as its first argument")
    expect(() => useAppApi()).toThrow("useAppApi() outside an app surface needs the app's slug as its first argument")
  })

  it('reads, writes and invalidates the app it is given', async () => {
    expect(await fetched(useAppQuery('crm', 'items'))).toBe('/api/apps/crm/items')
    expect(await fetched(useAppQuery('crm', () => 'items/1', { enabled: true }))).toBe('/api/apps/crm/items/1')
    await useAppApi('crm').put('items/1', { title: 'b' })
    expect(urls.at(-1)).toBe('/api/apps/crm/items/1')
    const qc = h.client as QueryClient
    qc.setQueryData(['app', 'crm', 'items'], {})
    await useAppInvalidate('crm')('items')
    expect(qc.getQueryState(['app', 'crm', 'items'])?.isInvalidated).toBe(true)
  })
})
