// The SDK reference tripwire's own edges: what the export parser sees, how a
// reference row is read, and what the diff calls a failure. A wrong answer
// here either fails every PR on a phantom or waves through a row an app
// author will copy into a broken build. Run by `bun run check`.
import { readFileSync, existsSync, statSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { diffReference, kindFits, parseReference, sdkExportNames, sdkExports, stripComments } from './sdk-reference.mjs'

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..')
let failed = 0
let ran = 0

function check(name, fn) {
  ran++
  try {
    fn()
  } catch (e) {
    failed++
    console.error(`FAIL ${name}: ${e instanceof Error ? e.stack : e}`)
  }
}

/** Key order is not part of any answer here: objects compare by sorted keys. */
const canon = (v) => JSON.stringify(v, (_, x) => (x && typeof x === 'object' && !Array.isArray(x) ? Object.fromEntries(Object.entries(x).sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))) : x))

function eq(actual, expected) {
  const a = canon(actual)
  const b = canon(expected)
  if (a !== b) throw new Error(`expected ${b}, got ${a}`)
}

const kinds = (src, resolveModule) => Object.fromEntries(sdkExports(src, resolveModule))

check('declarations: function, async function, const fn/const, class, interface, generic type', () => {
  const src = [
    'export function a() {}',
    'export async function b() {}',
    'export const c = (x: number) => x',
    'export const d =',
    '  (fn: () => void) =>',
    '  (e: KeyboardEvent): void => fn()',
    'export const E: Record<string, string> = { sm: "h-9" }',
    'export const F = { duration: 1 }',
    'export class G extends Error {}',
    'export interface H { x: number }',
    'export type I<T, U = T> = Pick<T, keyof U>',
    'export const j = async () => 1',
  ].join('\n')
  eq(kinds(src), { E: 'const', F: 'const', G: 'class', H: 'type', I: 'type', a: 'fn', b: 'fn', c: 'fn', d: 'fn', j: 'fn' })
})

check('a const initializer does not swallow the declaration on the next line', () => {
  eq(kinds('export const A = { x: 1 }\nexport const B = { y: 2 }\nexport const C = () => 3'), { A: 'const', B: 'const', C: 'fn' })
})

check('clauses: aliases, per-item and clause-level type, default-as from .svelte', () => {
  const src = [
    "export { default as Button, type ButtonProps } from '@/components/ui/Button.svelte'",
    "export { useSession as useMe, useIsAdmin, type SessionUser } from '@/lib/session'",
    "export type { Shape, Other as Renamed } from '@/lib/shapes'",
    "export { createQuery } from '@tanstack/svelte-query'",
  ].join('\n')
  const session = 'export function useSession() {}\nexport function useIsAdmin() {}\nexport interface SessionUser {}'
  const resolveModule = (spec) => (spec === '@/lib/session' ? session : null)
  eq(kinds(src, resolveModule), {
    Button: 'comp',
    ButtonProps: 'type',
    Renamed: 'type',
    SessionUser: 'type',
    Shape: 'type',
    createQuery: 'value',
    useIsAdmin: 'fn',
    useMe: 'fn',
  })
})

check('local bindings: a default .svelte import, a named import, a local declaration', () => {
  const src = [
    "import Input from '@/components/ui/Input.svelte'",
    "import { json } from '@/server/http'",
    "import type { AppStore } from '@/server/app-store'",
    'const helper = (x: number) => x',
    'export { Input, json, helper }',
    'export type { AppStore }',
  ].join('\n')
  const resolveModule = (spec) => (spec === '@/server/http' ? 'export function json() {}' : null)
  eq(kinds(src, resolveModule), { AppStore: 'type', Input: 'comp', helper: 'fn', json: 'fn' })
})

check('comments are not exports, and a path ending in /* inside a line comment does not hide code', () => {
  const src = ['// export function ghost() {}', '// API under /api/apps/<slug>/*', '/** export const alsoGhost = 1 */', 'export function real() {}'].join('\n')
  eq([...sdkExportNames(src)], ['real'])
  if (stripComments('a // b').includes(' ') === false) throw new Error('trailing comments are kept on purpose')
})

const REF = [
  '# SDK reference',
  '',
  'Prose may name `useThing`.',
  '',
  '```ts',
  "import { NotChecked } from '@talaria/sdk'",
  '```',
  '',
  '## `@talaria/sdk` — things',
  '',
  '| symbol | kind | what it is |',
  '| :--- | :--- | :--- |',
  '| `Thing` / `ThingProps` | comp / type | The thing |',
  '| `Chip` / `ChipProps` / `ChipTone` | comp / types | Three symbols, two kinds |',
  '| `useThing` | fn | Reads a thing; see `ThingProps` |',
  '',
  '## `@talaria/sdk/server` — server',
  '',
  '| symbol | kind | what it is |',
  '| :--- | :--- | :--- |',
  '| `defineThing` | fn | Server side |',
  '',
  '## Notes',
  '',
  '| symbol | kind | what it is |',
  '| :--- | :--- | :--- |',
  '| `useThing` | fn | Not under an entry point: prose, not a second row |',
].join('\n')

check('rows: entry from the heading, kinds paired in order, the last kind covers the rest', () => {
  const { rows, prose, problems } = parseReference(REF)
  eq(
    rows.map((r) => `${r.entry} ${r.name} ${r.kind}`),
    [
      '@talaria/sdk Thing comp',
      '@talaria/sdk ThingProps type',
      '@talaria/sdk Chip comp',
      '@talaria/sdk ChipProps type',
      '@talaria/sdk ChipTone type',
      '@talaria/sdk useThing fn',
      '@talaria/sdk/server defineThing fn',
    ],
  )
  eq([...new Set(prose.map((p) => p.name))].sort(), ['ThingProps', 'useThing'])
  eq(problems, [])
})

check('a kind cell that does not fit its symbols is a problem, not a guess', () => {
  const { problems } = parseReference('## `@talaria/sdk`\n| symbol | kind | what |\n| :--- | :--- | :--- |\n| `A` | widget | ? |\n| `B` | fn / type | ? |')
  eq(problems.length, 2)
})

const exportsOf = (entries) => new Map(Object.entries(entries).map(([e, m]) => [e, new Map(Object.entries(m))]))

check('the diff: a clean reference passes', () => {
  const d = diffReference(
    exportsOf({
      '@talaria/sdk': { Thing: 'comp', ThingProps: 'type', Chip: 'comp', ChipProps: 'type', ChipTone: 'type', useThing: 'fn' },
      '@talaria/sdk/server': { defineThing: 'fn' },
    }),
    parseReference(REF),
  )
  eq(d, { missing: [], ghosts: [], kinds: [], dupes: [] })
})

check('the diff: a symbol documented under the wrong entry point fails both ways', () => {
  const d = diffReference(
    exportsOf({
      '@talaria/sdk': { Thing: 'comp', ThingProps: 'type', Chip: 'comp', ChipProps: 'type', ChipTone: 'type', useThing: 'fn', defineThing: 'fn' },
      '@talaria/sdk/server': {},
    }),
    parseReference(REF),
  )
  eq(d.missing, ['defineThing — exported from @talaria/sdk, but its row sits under @talaria/sdk/server'])
  eq(d.ghosts, ['defineThing — listed under @talaria/sdk/server, but only @talaria/sdk exports it (line 21)'])
})

check('the diff: a wrong kind, a missing export and a ghost', () => {
  const d = diffReference(
    exportsOf({
      '@talaria/sdk': { Thing: 'type', ThingProps: 'type', Chip: 'comp', ChipProps: 'type', ChipTone: 'type', useThing: 'fn', Extra: 'fn' },
      '@talaria/sdk/server': {},
    }),
    parseReference(REF),
  )
  eq(d.kinds, ['Thing — the row says comp, @talaria/sdk exports a type (line 13)'])
  eq(d.missing, ['Extra — exported from @talaria/sdk (fn)'])
  eq(d.ghosts, ['defineThing — listed under @talaria/sdk/server, which does not export it (line 21)'])
})

check('the diff: a ghost named in prose, and a symbol with two rows under one entry point', () => {
  const md = '## `@talaria/sdk`\n| symbol | kind | what |\n| :--- | :--- | :--- |\n| `A` | fn | see `KeyHint` |\n| `A` | fn | again |'
  const d = diffReference(exportsOf({ '@talaria/sdk': { A: 'fn' } }), parseReference(md))
  eq(d.ghosts, ['KeyHint — named in prose at line 4, exported by no entry point'])
  eq(d.dupes, ['A — listed twice under @talaria/sdk (lines 4 and 5)'])
})

check('an unresolved runtime re-export fits any value kind, never comp or type', () => {
  eq([kindFits('value', 'fn'), kindFits('value', 'const'), kindFits('value', 'class'), kindFits('value', 'comp'), kindFits('value', 'type')], [true, true, true, false, false])
  eq([kindFits('fn', 'const'), kindFits('comp', 'comp')], [false, true])
})

check('the real tree: every entry point matches docs/sdk/reference.md', () => {
  const resolveModule = (spec) => {
    if (!spec.startsWith('@/')) return null
    const base = join(ROOT, 'ui/src', spec.slice(2))
    for (const c of [`${base}.ts`, `${base}/index.ts`, base]) if (existsSync(c) && statSync(c).isFile()) return readFileSync(c, 'utf8')
    return null
  }
  const byEntry = new Map([
    ['@talaria/sdk', sdkExports(readFileSync(join(ROOT, 'ui/src/sdk/index.ts'), 'utf8'), resolveModule)],
    ['@talaria/sdk/server', sdkExports(readFileSync(join(ROOT, 'ui/src/sdk/server.ts'), 'utf8'), resolveModule)],
    ['@talaria/sdk/testing', sdkExports(readFileSync(join(ROOT, 'ui/src/sdk/testing.ts'), 'utf8'), resolveModule)],
  ])
  const d = diffReference(byEntry, parseReference(readFileSync(join(ROOT, 'docs/sdk/reference.md'), 'utf8')))
  eq(d, { missing: [], ghosts: [], kinds: [], dupes: [] })
  // Every client export resolves to a kind — none fell through to "value"
  // except re-exports from a package the check cannot read.
  const unresolved = [...byEntry.get('@talaria/sdk')].filter(([, k]) => k === 'value').map(([n]) => n).sort()
  eq(unresolved, ['createMutation', 'createQuery', 'keepPreviousData', 'useQueryClient'])
})

if (failed) {
  console.error(`check-docs: ${failed} of ${ran} failed`)
  process.exit(1)
}
console.log(`check-docs: ${ran} checks, all clean`)
