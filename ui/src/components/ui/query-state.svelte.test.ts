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

/** Every `.svelte` / `.svelte.ts` file under ui/src, tests excluded. */
function componentSources(): { src: string; files: string[] } {
  const src = join(import.meta.dirname, '../..')
  const files: string[] = []
  const walk = (dir: string) => {
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
      const path = join(dir, entry.name)
      if (entry.isDirectory()) walk(path)
      else if ((entry.name.endsWith('.svelte') || entry.name.endsWith('.svelte.ts')) && !entry.name.includes('.test.')) files.push(path)
    }
  }
  walk(src)
  return { src, files }
}

/** The file's lines with an expression's continuation lines joined on: a line
 *  that ends in `(`, `||` or `&&`, or a next line that starts with `||`, `&&`,
 *  `?`, `:` or `)`. Each entry is [first line number, the joined text]. */
function logicalLines(text: string): [number, string][] {
  const lines = text.split('\n')
  const out: [number, string][] = []
  for (let i = 0; i < lines.length; i++) {
    const at = i + 1
    let joined = lines[i]!
    while (i + 1 < lines.length && (/(\(|\|\||&&)\s*$/.test(joined) || /^\s*(\|\||&&|\?|:|\))/.test(lines[i + 1]!))) {
      joined += ` ${lines[++i]!.trim()}`
    }
    out.push([at, joined])
  }
  return out
}

const FLAG = 'isLoading|isPending|isError|isFetching|isSuccess'

/** Where a short-circuit (`||`, `&&`, `?:`) reads the flags of two different
 *  queries in one expression: the second query's flag goes unread whenever
 *  the first decides the answer. A `$derived` that is only a query's flag
 *  (`const fleetLoading = $derived(fleetQuery.isLoading)`) counts as that
 *  query's flag where it is read. `anyLoading`, `anyPending`, `anyError`,
 *  `anyFailed` and `firstError` read every flag before they combine, and so
 *  does `[...].includes(true)`. */
function crossQueryShortCircuits(text: string): { line: number; queries: string[]; expr: string }[] {
  const alias = new RegExp(String.raw`const (\w+) = \$derived\(\s*!?\s*(\w+)\??\.(?:${FLAG})\s*\)`, 'g')
  const aliases = new Map<string, string>()
  for (const m of text.matchAll(alias)) aliases.set(m[1]!, m[2]!)
  const flag = new RegExp(String.raw`\b(\w+)\??\.(?:${FLAG})\b`, 'g')
  const word = aliases.size ? new RegExp(String.raw`\b(${[...aliases.keys()].join('|')})\b`, 'g') : null
  const found: { line: number; queries: string[]; expr: string }[] = []
  for (const [line, expr] of logicalLines(text)) {
    if (new RegExp(alias.source).test(expr)) continue
    const eager = expr
      .replace(/\b(?:anyLoading|anyPending|anyError|anyFailed|firstError)\([^()]*\)/g, '_')
      .replace(/\[[^[\]]*\]\.includes\(true\)/g, '_')
    if (!/\|\||&&|\?(?![.?])/.test(eager)) continue
    const queries = new Set<string>()
    for (const m of eager.matchAll(flag)) queries.add(m[1]!)
    if (word) for (const m of eager.matchAll(word)) queries.add(aliases.get(m[1]!)!)
    if (queries.size > 1) found.push({ line, queries: [...queries].sort(), expr: expr.trim() })
  }
  return found
}

test('the scan catches every shape that left a surface unread', () => {
  const shapes = [
    'const gate = $derived(defsQuery.isLoading || containersQuery.isLoading)',
    'const failed = $derived(a.isError || b.isError)',
    '{#if statuses.isError || labels?.isError}',
    'const failed = $derived(catalogQuery.isError ? catalogQuery : prefsQuery.isError ? prefsQuery : null)',
    'const failed = $derived(\n  (configQuery.isError && config === undefined) ||\n    (fleetQuery.isError && fleet === undefined),\n)',
    'const configLoading = $derived(configQuery.isLoading)\nconst fleetLoading = $derived(fleetQuery.isLoading)\n{:else if configLoading || fleetLoading}',
    'const isLoading = $derived(boardsQuery.isLoading)\nconst archivedLoading = $derived(archivedQuery.isLoading)\n{#if isLoading || archivedLoading || (board && tasksLoading)}',
  ]
  for (const shape of shapes) expect(crossQueryShortCircuits(shape).length, shape).toBeGreaterThan(0)
  const fine = [
    'const gate = $derived(anyLoading(defsQuery, containersQuery))',
    '{:else if anyError(defsQuery, containersQuery)}',
    'const failed = $derived(anyFailed(configQuery, fleetQuery))',
    'const failed = $derived(firstError(catalogQuery, prefsQuery))',
    'const loading = $derived([usersList.pending, agentsQuery.isLoading, teamsList.pending].includes(true))',
    // One query's own flags may short-circuit: the path only changes when a
    // flag already read changes, and that change is what tells the component.
    '{#if query.isError && query.data === undefined}',
    'const loading = $derived(history.data === undefined && !history.isError)',
    'title={boardsQuery.isError ? boardsQuery.error : teamsQuery.error}',
  ]
  for (const shape of fine) expect(crossQueryShortCircuits(shape), shape).toEqual([])
})

test("no component short-circuits across two queries' flags", () => {
  const { src, files } = componentSources()
  const found: string[] = []
  for (const file of files) {
    for (const f of crossQueryShortCircuits(readFileSync(file, 'utf8'))) {
      found.push(`${relative(src, file)}:${f.line}: [${f.queries.join(', ')}] ${f.expr} (read every flag first: anyLoading/anyPending/anyError/anyFailed/firstError)`)
    }
  }
  expect(found).toEqual([])
})
