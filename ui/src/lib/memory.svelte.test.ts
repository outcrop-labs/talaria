// @vitest-environment runes
//
// `useMemory` in MemoryEditor's shape: the editor reads `isError` on mount and
// `data` only once Whole file is on screen. svelte-query passes on a change
// only when it touches a prop already read, so the answer that landed while
// Entries showed never reached a `data` read made after it, and Whole file sat
// on its skeleton. The hook notifies on every change (`notifyOnChangeProps`),
// and this runs it over a real query on Svelte's client runtime
// (vitest.runes-environment.ts) to show the late read gets the answer.
import { QueryClient } from '@tanstack/svelte-query'
import { afterEach, expect, test, vi } from 'vitest'

const h = vi.hoisted(() => ({
  client: null as unknown,
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

// The hook takes its client from component context; a test has none.
vi.mock('@tanstack/svelte-query', async (importOriginal) => {
  const real = await importOriginal<typeof import('@tanstack/svelte-query')>()
  const createQuery = ((options: Parameters<typeof real.createQuery>[0], client?: () => QueryClient) =>
    real.createQuery(options, client ?? (() => h.client as QueryClient))) as typeof real.createQuery
  return { ...real, createQuery }
})

import { useMemory } from './memory'

const settle = () => new Promise((resolve) => setTimeout(resolve, 0))

afterEach(() => {
  h.answers.clear()
  ;(h.client as QueryClient | null)?.clear()
})

test('a `data` read made after the answer landed gets the answer', async () => {
  h.client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  let query!: ReturnType<typeof useMemory>
  const stop = $effect.root(() => {
    query = useMemory(() => 'ada-ops')
    // MemoryEditor's `{#if query.isError && offline}`, on mount.
    $effect(() => {
      void query.isError
    })
  })
  await settle()
  const url = '/api/memory/ada-ops?file=memory'
  await vi.waitFor(() => expect(h.answers.has(url)).toBe(true))
  h.answers.get(url)!({ content: '- Prefers short status notes.\n', container: 'agent-ada-ops' })
  await settle()
  await settle()
  // Whole file: the first read of `data`, after the answer.
  expect(query.data).toEqual({ content: '- Prefers short status notes.\n', container: 'agent-ada-ops' })
  stop()
})
