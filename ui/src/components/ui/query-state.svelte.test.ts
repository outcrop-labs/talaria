// @vitest-environment runes
//
// `anyLoading` / `anyPending` (query-state.ts): one surface gated on two
// reads must read BOTH flags every time, never `a.isLoading || b.isLoading`.
//
// Two halves, like model-efforts.svelte.test.ts. The reactive half runs real
// svelte-query queries on Svelte's client runtime (vitest.runes-environment.ts)
// in the agent roster's exact shape: two reads gated together, and the second
// read's `isError` read on its own, as the roster's "could not read container
// status" banner reads it. When the second read answers first, the `||` gate
// stays loading for good and `anyLoading` lets go. The source half holds every
// component to the reading that works.
import { readdirSync, readFileSync } from 'node:fs'
import { join, relative } from 'node:path'
import { createQuery, QueryClient } from '@tanstack/svelte-query'
import { expect, test, vi } from 'vitest'
import { anyLoading, anyPending } from './query-state'

/** Let the client runtime run its queued effects. */
const settle = () => new Promise((resolve) => setTimeout(resolve, 0))

function deferred<T>() {
  let resolve!: (v: T) => void
  const promise = new Promise<T>((r) => {
    resolve = r
  })
  return { promise, resolve }
}

type Flags = { readonly isLoading: boolean; readonly isPending: boolean; readonly isError: boolean }

/** The roster's shape: `defs` and `containers` gate one surface together, and
 *  the containers' error flag is read on its own. */
function roster(gate: (defs: Flags, containers: Flags) => boolean) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  const defs = deferred<string[]>()
  const containers = deferred<string[]>()
  const seen = { gated: true }
  const stop = $effect.root(() => {
    const defsQuery = createQuery(() => ({ queryKey: ['defs'], queryFn: () => defs.promise }), () => client)
    const containersQuery = createQuery(() => ({ queryKey: ['containers'], queryFn: () => containers.promise }), () => client)
    const gated = $derived(gate(defsQuery, containersQuery))
    $effect(() => {
      seen.gated = gated
    })
    // The banner: `{#if containersQuery.isError && …}`.
    $effect(() => {
      void containersQuery.isError
    })
  })
  return {
    seen,
    answer: async (which: 'defs' | 'containers') => {
      ;(which === 'defs' ? defs : containers).resolve([which])
      await settle()
      await settle()
    },
    done: () => {
      stop()
      client.clear()
    },
  }
}

const viaOr = (a: Flags, b: Flags) => a.isLoading || b.isLoading
const viaOrPending = (a: Flags, b: Flags) => a.isPending || b.isPending

test('the bug, kept as a fact: a `||` gate stays loading when the second read answers first', async () => {
  const r = roster(viaOr)
  await settle()
  expect(r.seen.gated).toBe(true)
  await r.answer('containers')
  await r.answer('defs')
  // Both reads have answered. The `||` gate read containers' flag for the
  // first time only now, and got the value from before containers answered.
  await new Promise((resolve) => setTimeout(resolve, 50))
  expect(r.seen.gated).toBe(true)
  r.done()
})

test('anyLoading lets go once both reads answer, in either order', async () => {
  for (const order of [
    ['containers', 'defs'],
    ['defs', 'containers'],
  ] as const) {
    const r = roster(anyLoading)
    await settle()
    expect(r.seen.gated).toBe(true)
    await r.answer(order[0])
    expect(r.seen.gated).toBe(true)
    await r.answer(order[1])
    await vi.waitFor(() => expect(r.seen.gated).toBe(false))
    r.done()
  }
})

test('anyPending does the same for isPending', async () => {
  const stuck = roster(viaOrPending)
  await settle()
  await stuck.answer('containers')
  await stuck.answer('defs')
  await new Promise((resolve) => setTimeout(resolve, 50))
  expect(stuck.seen.gated).toBe(true)
  stuck.done()

  for (const order of [
    ['containers', 'defs'],
    ['defs', 'containers'],
  ] as const) {
    const r = roster(anyPending)
    await settle()
    await r.answer(order[0])
    expect(r.seen.gated).toBe(true)
    await r.answer(order[1])
    await vi.waitFor(() => expect(r.seen.gated).toBe(false))
    r.done()
  }
})

test('no component gates on two queries with a `||` of their loading flags', () => {
  const src = join(import.meta.dirname, '../..')
  const files: string[] = []
  const walk = (dir: string) => {
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
      const path = join(dir, entry.name)
      if (entry.isDirectory()) walk(path)
      else if (entry.name.endsWith('.svelte') || entry.name.endsWith('.svelte.ts')) files.push(path)
    }
  }
  walk(src)
  const gate = /\b(\w+)\.(isLoading|isPending)\s*\|\|\s*(\w+)\.(isLoading|isPending)\b/g
  const found: string[] = []
  for (const file of files) {
    const text = readFileSync(file, 'utf8')
    for (const m of text.matchAll(gate)) {
      if (m[1] === m[3]) continue
      const line = text.slice(0, m.index).split('\n').length
      found.push(`${relative(src, file)}:${line}: ${m[0]} (use anyLoading/anyPending)`)
    }
  }
  expect(found).toEqual([])
})
