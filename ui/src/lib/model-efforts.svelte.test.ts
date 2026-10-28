// @vitest-environment runes
//
// `useModelEfforts` answers through GETTERS over a live query, and that is a
// promise only a caller that keeps the object can collect on. Five surfaces
// destructured it (`const { efforts } = useModelEfforts(...)`), which reads
// the query once, at mount: `[]` on a fresh page, so their effort chips never
// appeared, and the agent form's main row cleared a saved level on load.
//
// Two halves. The reactive half runs the real hook over a real query (client
// runtime, see vitest.runes-environment.ts) and shows both readings: the kept
// object follows the answer, a destructured copy stays at mount. The source
// half holds every caller to the reading that works. Rendering the five
// surfaces would need a DOM this suite does not have; the browser runs in the
// PR's verification cover them.
import { readdirSync, readFileSync } from 'node:fs'
import { join, relative } from 'node:path'
import { QueryClient } from '@tanstack/svelte-query'
import { afterEach, expect, test, vi } from 'vitest'

const h = vi.hoisted(() => ({
  client: null as unknown,
  /** The pending read for each url, resolved by the test. */
  answers: new Map<string, (body: unknown) => void>(),
}))

vi.mock('@/lib/fetch-json', () => ({
  getJson: vi.fn(
    (url: string) =>
      new Promise((resolve) => {
        h.answers.set(url, resolve)
      }),
  ),
}))

// The hook takes its client from component context; a test has none, so the
// real `createQuery` is handed this test's client instead.
vi.mock('@tanstack/svelte-query', async (importOriginal) => {
  const real = await importOriginal<typeof import('@tanstack/svelte-query')>()
  const createQuery = ((options: Parameters<typeof real.createQuery>[0], client?: () => QueryClient) =>
    real.createQuery(options, client ?? (() => h.client as QueryClient))) as typeof real.createQuery
  return { ...real, createQuery }
})

import { useModelEfforts } from './model-efforts.svelte'

/** Let the client runtime run its queued effects. (`flushSync` from 'svelte'
 *  is no use here: the test file's own `svelte` import is Node's, the server
 *  build, while the compiled runes run on the client internals.) */
const settle = () => new Promise((resolve) => setTimeout(resolve, 0))

const url = (model: string) => `/api/models/efforts?model=${encodeURIComponent(model)}`

function answer(model: string, body: { efforts: string[]; default: string | null }) {
  const resolve = h.answers.get(url(model))
  if (!resolve) throw new Error(`no read in flight for ${model}`)
  h.answers.delete(url(model))
  resolve(body)
}

afterEach(() => {
  h.answers.clear()
  ;(h.client as QueryClient | null)?.clear()
})

test('the kept object follows the answer; a destructured read keeps what mount saw', async () => {
  h.client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  let kept!: ReturnType<typeof useModelEfforts>
  let destructured: string[] = ['never read']
  const seen = { levels: ['never read'], fallback: 'never read' as string | null }
  const stop = $effect.root(() => {
    kept = useModelEfforts(() => 'ada-ops')
    ;({ efforts: destructured } = kept)
    // What the five surfaces now do: keep the object, derive from it.
    const levels = $derived(kept.efforts)
    const fallback = $derived(kept.default)
    $effect(() => {
      seen.levels = levels
      seen.fallback = fallback
    })
  })
  await settle()
  expect(seen.levels).toEqual([])
  expect(kept.answered).toBe(false)

  await vi.waitFor(() => expect(h.answers.has(url('ada-ops'))).toBe(true))
  answer('ada-ops', { efforts: ['low', 'medium', 'high'], default: 'high' })
  await vi.waitFor(() => {
    expect(seen.levels).toEqual(['low', 'medium', 'high'])
  })
  expect(seen.fallback).toBe('high')
  expect(kept.answered).toBe(true)
  // The bug, kept as a fact: the destructured copy is the mount's `[]`.
  expect(destructured).toEqual([])
  stop()
})

test('a model switch empties the levels until the new answer is in, and says so', async () => {
  h.client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  let model = $state('ep/one')
  let kept!: ReturnType<typeof useModelEfforts>
  const seen = { levels: [] as string[] }
  const stop = $effect.root(() => {
    kept = useModelEfforts(() => model)
    const levels = $derived(kept.efforts)
    $effect(() => {
      seen.levels = levels
    })
  })
  await settle()
  await vi.waitFor(() => expect(h.answers.has(url('ep/one'))).toBe(true))
  answer('ep/one', { efforts: ['low', 'high'], default: null })
  await vi.waitFor(() => {
    expect(seen.levels).toEqual(['low', 'high'])
  })

  model = 'ep/two'
  await settle()
  // In flight: no levels, and not an answer — the agent form's main row waits
  // for `answered` before it clears a saved level (TargetRow).
  expect(seen.levels).toEqual([])
  expect(kept.answered).toBe(false)
  await vi.waitFor(() => expect(h.answers.has(url('ep/two'))).toBe(true))
  answer('ep/two', { efforts: ['minimal'], default: null })
  await vi.waitFor(() => {
    expect(seen.levels).toEqual(['minimal'])
  })
  expect(kept.answered).toBe(true)
  stop()
})

const SRC = join(import.meta.dirname, '..')

function sources(dir: string, out: string[] = []): string[] {
  for (const e of readdirSync(dir, { withFileTypes: true })) {
    const full = join(dir, e.name)
    if (e.isDirectory()) sources(full, out)
    else if ((e.name.endsWith('.svelte') || e.name.endsWith('.ts')) && !e.name.endsWith('.test.ts')) out.push(full)
  }
  return out
}

test('every caller keeps the object: none destructures it', () => {
  const callers: string[] = []
  const offenders: string[] = []
  for (const file of sources(SRC)) {
    const src = readFileSync(file, 'utf8')
    if (!src.includes('useModelEfforts(')) continue
    for (const [line] of src.matchAll(/^.*\buseModelEfforts\(.*$/gm)) {
      if (/^\s*(\/\/|\*|\/\*)/.test(line) || /\bfunction useModelEfforts\(/.test(line)) continue
      const where = `${relative(SRC, file)}: ${line.trim()}`
      callers.push(where)
      if (!/^\s*const\s+[A-Za-z_$][\w$]*\s*=\s*useModelEfforts\(/.test(line)) offenders.push(where)
    }
  }
  expect(offenders).toEqual([])
  // The five surfaces the chip lives on; a scan that finds none proves nothing.
  expect(callers.length).toBeGreaterThanOrEqual(5)
})
