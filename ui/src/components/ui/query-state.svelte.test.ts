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
// stays loading for good and `anyLoading` lets go. The same holds for a
// `listQuery` result's `pending`/`failed`/`stale`/`notice`, which are its
// query's flags under other names. The source half holds every component to
// the reading that works.
import { readdirSync, readFileSync } from 'node:fs'
import { join, relative } from 'node:path'
import { createQuery, QueryClient } from '@tanstack/svelte-query'
import { expect, test, vi } from 'vitest'
import { anyLoading, anyPending, listQuery } from './query-state'

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

/** The RAG collection row's shape (CollectionRow.svelte): the agents read
 *  and two directory lists gate the binding pickers together, and the
 *  directories' notices are read on their own, which reads only `isError`
 *  while a read is fine. */
type List = { readonly pending: boolean; readonly failed: boolean; readonly notice: unknown }
function collectionRow(gate: (agents: Flags, users: List, teams: List) => boolean) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  const reads = { agents: deferred<string[]>(), users: deferred<string[]>(), teams: deferred<string[]>() }
  const seen = { gated: true }
  const stop = $effect.root(() => {
    const agentsQuery = createQuery(() => ({ queryKey: ['agents'], queryFn: () => reads.agents.promise }), () => client)
    const usersList = listQuery(createQuery(() => ({ queryKey: ['users'], queryFn: () => reads.users.promise }), () => client), { title: 'people' })
    const teamsList = listQuery(createQuery(() => ({ queryKey: ['teams'], queryFn: () => reads.teams.promise }), () => client), { title: 'teams' })
    const gated = $derived(gate(agentsQuery, usersList, teamsList))
    $effect(() => {
      seen.gated = gated
    })
    // The notices: `{#if teamsList.notice}…{#if usersList.notice}`.
    $effect(() => {
      void teamsList.notice
      void usersList.notice
    })
  })
  return {
    seen,
    answer: async (which: keyof typeof reads) => {
      reads[which].resolve([which])
      await settle()
      await settle()
    },
    done: () => {
      stop()
      client.clear()
    },
  }
}

test("a listQuery's `pending` is a flag too: the collection row's `||` gate stuck, the eager read lets go", async () => {
  const stuck = collectionRow((agents, users, teams) => agents.isPending || users.pending || teams.pending)
  await settle()
  await stuck.answer('users')
  await stuck.answer('teams')
  await stuck.answer('agents')
  await new Promise((resolve) => setTimeout(resolve, 50))
  expect(stuck.seen.gated).toBe(true)
  stuck.done()

  for (const order of [
    ['users', 'teams', 'agents'],
    ['agents', 'users', 'teams'],
    ['teams', 'agents', 'users'],
  ] as const) {
    const r = collectionRow((agents, users, teams) => [agents.isPending, users.pending, teams.pending].includes(true))
    await settle()
    await r.answer(order[0])
    await r.answer(order[1])
    expect(r.seen.gated).toBe(true)
    await r.answer(order[2])
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

/** The text with its comments blanked (markup, line-leading block, and `//`
 *  after whitespace), every newline kept so line numbers still match. A
 *  comment that spells out the old bug is not the bug. */
function code(text: string): string {
  const blank = (m: string) => m.replace(/[^\n]/g, ' ')
  return text
    .replace(/<!--[\s\S]*?-->/g, blank)
    .replace(/^[ \t]*\/\*[\s\S]*?\*\//gm, blank)
    .replace(/(^|\s)\/\/.*$/gm, (m, lead: string) => lead + blank(m.slice(lead.length)))
}

/** The file's lines with an expression's continuation lines joined on: a line
 *  that ends in `(`, `||`, `&&` or `??`, or a next line that starts with
 *  `||`, `&&`, `?`, `:` or `)`. Each entry is [first line number, the joined
 *  text]. */
function logicalLines(text: string): [number, string][] {
  const lines = text.split('\n')
  const out: [number, string][] = []
  for (let i = 0; i < lines.length; i++) {
    const at = i + 1
    let joined = lines[i]!
    while (i + 1 < lines.length && (/(\(|\|\||&&|\?\?)\s*$/.test(joined) || /^\s*(\|\||&&|\?|:|\))/.test(lines[i + 1]!))) {
      joined += ` ${lines[++i]!.trim()}`
    }
    out.push([at, joined])
  }
  return out
}

/** A query result's flags. */
const FLAG = 'isLoading|isPending|isError|isFetching|isSuccess'
/** A `listQuery(...)` result's flags: its query's, under other names. */
const LIST_FLAG = 'pending|failed|stale|notice'
/** The helpers that read every query's flag before they combine. */
const EAGER = 'anyLoading|anyPending|anyError|anyFailed|firstError|firstNotice'
const OPEN = '([{'
const CLOSE = ')]}'

/** Where a short-circuit (`||`, `&&`, `??`, `?:`) decides on one query's
 *  flags whether another query's flags get read: whenever the first decides
 *  the answer, the other goes unread, and the component is not told when it
 *  changes. For each operator, the deciding side is what sits before it in
 *  its own bracket or argument, and the conditional side what follows. A
 *  site is a query read on the conditional side that the deciding side does
 *  not read, behind a deciding side that reads a flag at all.
 *
 *  - One query's own flags may short-circuit: the path only changes when a
 *    flag already read changes, and that change is what tells the component.
 *  - A `listQuery(...)` result's flags count as the query it wraps.
 *  - A `$derived` whose body reads flags counts, where it is read, as the
 *    queries its body reads (`const fleetFailed = $derived(fleetQuery.isError
 *    && …)` is fleetQuery's flag).
 *  - `anyLoading`, `anyPending`, `anyError`, `anyFailed`, `firstError`,
 *    `firstNotice` and `[...].includes(true)` read every flag first, and
 *    count as one read of all of their queries.
 *  - A guard that reads no flag (data, a prop, UI state) ahead of a flag read
 *    is the ordinary `{#if}`, and so is an `{#if}`/`{:else if}` chain: the
 *    scan does not judge those. */
function crossQueryShortCircuits(text: string): { line: number; queries: string[]; expr: string }[] {
  const source = code(text)
  const lists = new Map<string, string>()
  for (const m of source.matchAll(/const (\w+) = listQuery\(\s*(\w+)?(\s*,)?/g)) lists.set(m[1]!, m[2] && m[3] ? m[2] : m[1]!)
  const query = (name: string) => lists.get(name) ?? name
  const aliases = new Map<string, Set<string>>()
  const flag = new RegExp(String.raw`\b(\w+)\??\.(?:${FLAG})\b`, 'g')
  const listFlag = lists.size ? new RegExp(String.raw`\b(${[...lists.keys()].join('|')})\??\.(?:${LIST_FLAG})\b`, 'g') : null

  /** Every read in `expr`, with where it sits and the queries it reads;
   *  eager reads masked to one placeholder of the same length. */
  function reads(expr: string): { masked: string; at: [number, Set<string>][] } {
    const at: [number, Set<string>][] = []
    const mask = (m: string, i: number, queries: Set<string>) => {
      at.push([i, queries])
      return '#'.padEnd(m.length, '_')
    }
    let masked = expr.replace(new RegExp(String.raw`\b(?:${EAGER})\(([^()]*)\)`, 'g'), (m, args: string, i: number) =>
      mask(m, i, new Set(args.split(',').map((a) => a.trim()).filter((a) => /^\w+$/.test(a)).map(query))),
    )
    masked = masked.replace(/\[([^[\]]*)\]\.includes\(true\)/g, (m, inner: string, i: number) =>
      mask(m, i, new Set(reads(inner).at.flatMap(([, queries]) => [...queries]))),
    )
    for (const m of masked.matchAll(flag)) at.push([m.index!, new Set([query(m[1]!)])])
    if (listFlag) for (const m of masked.matchAll(listFlag)) at.push([m.index!, new Set([lists.get(m[1]!)!])])
    if (aliases.size) {
      const word = new RegExp(String.raw`(?<![.\w$])(${[...aliases.keys()].join('|')})\b(?!\s*=(?!=))(?!\??\.)`, 'g')
      for (const m of masked.matchAll(word)) at.push([m.index!, aliases.get(m[1]!)!])
    }
    return { masked, at }
  }

  const found: { line: number; queries: string[]; expr: string }[] = []
  for (const [line, expr] of logicalLines(source)) {
    const { masked, at } = reads(expr)
    const hit = new Set<string>()
    for (const op of masked.matchAll(/\|\||&&|\?\?|\?(?!\.)/g)) {
      const p = op.index!
      let start = p
      for (let depth = 0; start > 0; start--) {
        const c = masked[start - 1]!
        if (CLOSE.includes(c)) depth++
        else if (OPEN.includes(c)) {
          if (depth === 0) break
          depth--
        } else if ((c === ',' || c === ';') && depth === 0) break
      }
      let end = p + op[0].length
      for (let depth = 0; end < masked.length; end++) {
        const c = masked[end]!
        if (OPEN.includes(c)) depth++
        else if (CLOSE.includes(c)) {
          if (depth === 0) break
          depth--
        } else if ((c === ',' || c === ';') && depth === 0) break
      }
      const deciding = new Set(at.filter(([i]) => i >= start && i < p).flatMap(([, queries]) => [...queries]))
      if (!deciding.size) continue
      for (const [i, queries] of at) {
        if (i <= p || i >= end) continue
        for (const q of queries) {
          if (deciding.has(q)) continue
          hit.add(q)
          for (const d of deciding) hit.add(d)
        }
      }
    }
    if (hit.size) found.push({ line, queries: [...hit].sort(), expr: expr.trim() })
    const def = /^\s*(?:const|let) (\w+)(?::[^=]+)? = \$derived\(/.exec(expr)
    if (def) {
      const body = new Set(at.filter(([i]) => i >= def[0].length).flatMap(([, queries]) => [...queries]))
      if (body.size) aliases.set(def[1]!, body)
    }
  }
  return found
}

/** Two directory lists, as the components declare them. */
const LISTS = "const usersList = listQuery(useUsers(), { title: 'people' })\nconst teamsList = listQuery(useTeams(), { title: 'teams' })\n"

test('the scan catches every shape that left a surface unread', () => {
  const shapes = [
    'const gate = $derived(defsQuery.isLoading || containersQuery.isLoading)',
    'const failed = $derived(a.isError || b.isError)',
    '{#if statuses.isError || labels?.isError}',
    'const failed = $derived(catalogQuery.isError ? catalogQuery : prefsQuery.isError ? prefsQuery : null)',
    'const failed = $derived(\n  (configQuery.isError && config === undefined) ||\n    (fleetQuery.isError && fleet === undefined),\n)',
    'const configLoading = $derived(configQuery.isLoading)\nconst fleetLoading = $derived(fleetQuery.isLoading)\n{:else if configLoading || fleetLoading}',
    'const isLoading = $derived(boardsQuery.isLoading)\nconst archivedLoading = $derived(archivedQuery.isLoading)\n{#if isLoading || archivedLoading || (board && tasksLoading)}',
    // listQuery flags. PermissionsModal's and CollectionRow's gates, as they were.
    `${LISTS}const principalsLoading = $derived(usersList.pending || agentsQuery.isLoading || teamsList.pending)`,
    `${LISTS}const pickersPending = $derived(agentsQuery.isPending || usersList.pending || teamsList.pending)`,
    `${LISTS}{#if !bindingsAll && (teamsList.notice || usersList.notice)}`,
    `${LISTS}{@const notice = usersList.notice ?? teamsList.notice}`,
    `${LISTS}{#if !board && (usersList.failed || teamsList.failed)}`,
    // A `$derived` with a compound body is its query's flag where it is read.
    'const fleetFailed = $derived(fleetQuery.isError && fleetQuery.data === undefined)\nconst threadsFailed = $derived(conversationsQuery.isError && conversationsQuery.data === undefined)\nconst showAgents = $derived(!view.searching || fleetFailed || threadsFailed)',
    // An eager read deciding whether another query's flag is read.
    'const listsLoading = $derived(anyLoading(boardsQuery, archivedQuery))\n{#if listsLoading || (board && tasksQuery.isLoading)}',
    "const boardList = listQuery(boardsQuery, { title: 'boards' })\nconst tasksPending = $derived(taskQueries.some((result) => result.isPending))\nconst loading = $derived(boardList.pending || tasksPending)",
  ]
  for (const shape of shapes) expect(crossQueryShortCircuits(shape).length, shape).toBeGreaterThan(0)
  const fine = [
    'const gate = $derived(anyLoading(defsQuery, containersQuery))',
    '{:else if anyError(defsQuery, containersQuery)}',
    'const failed = $derived(anyFailed(configQuery, fleetQuery))',
    'const failed = $derived(firstError(catalogQuery, prefsQuery))',
    `${LISTS}const loading = $derived([usersList.pending, agentsQuery.isLoading, teamsList.pending].includes(true))`,
    `${LISTS}{#if !bindingsAll && firstNotice(teamsList, usersList)}`,
    `${LISTS}{#if usersList.failed && usersList.notice}`,
    'const x = $derived(!view.searching || [fleetFailed, threadsFailed].includes(true))',
    // One query's own flags may short-circuit: the path only changes when a
    // flag already read changes, and that change is what tells the component.
    '{#if query.isError && query.data === undefined}',
    'const loading = $derived(history.data === undefined && !history.isError)',
    'title={boardsQuery.isError ? boardsQuery.error : teamsQuery.error}',
    // Two eager reads of the same queries: the second's queries were read.
    'const loading = $derived(anyPending(catalogQuery, prefsQuery))\nconst failed = $derived(firstError(catalogQuery, prefsQuery))\n{#if !failed && !loading && efforts.length > 0}',
    // A pick by UI state, not by a flag.
    "loading={place === 'drive' ? driveQuery.isLoading : anyLoading(artifactsQuery, foldersQuery)}",
    // A query object picked eagerly, then read on its own.
    'const boardListQuery = $derived(firstError(boardsQuery, archivedQuery) ?? archivedQuery)\n{:else if !board && boardListQuery.isError && boardListQuery.data === undefined}',
    // A comment that spells out the old bug.
    '// `a.isLoading || b.isLoading` stopped at a\n<!-- a.isError || b.isError -->\n/** a.isPending || b.isPending */',
  ]
  for (const shape of fine) expect(crossQueryShortCircuits(shape), shape).toEqual([])
})

test("no component short-circuits across two queries' flags", () => {
  const { src, files } = componentSources()
  const found: string[] = []
  for (const file of files) {
    for (const f of crossQueryShortCircuits(readFileSync(file, 'utf8'))) {
      found.push(`${relative(src, file)}:${f.line}: [${f.queries.join(', ')}] ${f.expr} (read every flag first: anyLoading/anyPending/anyError/anyFailed/firstError/firstNotice, or [...].includes(true))`)
    }
  }
  expect(found).toEqual([])
})
