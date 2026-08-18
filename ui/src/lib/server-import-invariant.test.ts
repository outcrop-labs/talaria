import { execFileSync } from 'node:child_process'
import { mkdirSync, rmSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'
import { afterEach, describe, expect, it } from 'vitest'

// THE HARNESS TWO SESSIONS BUILT BY HAND, TWICE, MADE DURABLE.
//
// `server-value-import-in-a-browser-module` is the rule that keeps the database
// pool, the harness runner and the guard registry out of the client bundle. It
// has been edited four times in one evening — narrowed twice for false
// positives, widened twice for false negatives — and each round was verified by
// somebody planting imports into a real source file by hand, running the
// checker, and restoring. That is a good method and a bad thing to rebuild from
// memory: the interesting cases are the ones you did not think of, and they
// only survive if they are written down.
//
// IT PLANTS INTO A THROWAWAY FILE, never into a real one. The obvious version
// edits `lib/cn.ts` in place and restores it afterwards — which is fine until
// the process dies between the two, and this repo is routinely worked by
// several agents in one tree at once. A file that only ever gets created and
// deleted cannot corrupt anything that was already there.
//
// IT SHELLS OUT rather than importing the checker, because the checker is a
// top-level script with no exports: running it IS its API, and its exit code is
// the contract every caller (CI, a person, this test) actually depends on.

const ROOT = join(import.meta.dirname, '../../..')
const PROBE = join(ROOT, 'ui/src/lib/__invariant_probe__')

/** Run the checker over a tree containing exactly these planted files, and
 *  return which of them it named.
 *
 *  ONE SUBPROCESS FOR THE WHOLE TABLE, not one per case. The first version ran
 *  the checker 23 times and took twelve seconds, which is how a useful test
 *  becomes the test somebody excludes from the watch run. Planting every case
 *  as its OWN file keeps the per-case attribution that made it worth having:
 *  the checker reports the path it found each violation in, so a table of
 *  twenty cases is still twenty independent assertions.
 *
 *  Output is read from the thrown error on failure, because a non-zero exit is
 *  the normal outcome here and `execFileSync` signals that by throwing. */
function runChecker(files: Record<string, string>): { passed: boolean; output: string } {
  mkdirSync(PROBE, { recursive: true })
  for (const [name, body] of Object.entries(files)) writeFileSync(join(PROBE, name), body)
  try {
    const out = execFileSync('node', ['scripts/check-invariants.mjs'], { cwd: ROOT, stdio: 'pipe' })
    return { passed: true, output: out.toString() }
  } catch (e) {
    const err = e as { stdout?: Buffer; stderr?: Buffer }
    return { passed: false, output: `${err.stdout?.toString() ?? ''}${err.stderr?.toString() ?? ''}` }
  } finally {
    rmSync(PROBE, { recursive: true, force: true })
  }
}

/** `name` → the file it is planted in. Slugged so the checker's report names
 *  the case rather than `probe1.ts`. */
const fileFor = (name: string): string => `${name.replace(/[^a-z0-9]+/gi, '-').toLowerCase()}.ts`

const plant = (cases: Array<[string, string]>): Record<string, string> =>
  Object.fromEntries(cases.map(([name, line]) => [fileFor(name), `${line}\nexport const probe = 1\n`]))

afterEach(() => rmSync(PROBE, { recursive: true, force: true }))

describe('a browser module importing from @/server', () => {
  // Erased at build time. Every one of these must pass, and every one of them
  // has at some point been reported as a violation by some version of the rule.
  const SAFE: Array<[string, string]> = [
    ['prefixed type import', "import type { Setting } from '@/server/audit'"],
    ['inline type-only', "import { type Setting } from '@/server/audit'"],
    ['inline type with an alias', "import { type Setting as S } from '@/server/audit'"],
    ['inline type, trailing comma', "import { type Setting, } from '@/server/audit'"],
    ['type re-export, prefixed', "export type { Setting } from '@/server/audit'"],
    ['type re-export, inline', "export { type Setting } from '@/server/audit'"],
    // The line that made "match every `import(`" untenable: a dynamic import in
    // TYPE position is erased, and no regex can tell it from a call.
    ['dynamic import in type position', "type T = import('@/server/audit').Setting\nexport type { T }"],
    ['dynamic import inside an arrow TYPE', "type F = () => import('@/server/audit').Setting\nexport type { F }"],
    // Behind another import: the false positive that came from `[^;]*?` running
    // across statements in a codebase with no semicolons.
    ['type import behind a value import', "import { cn } from '@/lib/cn'\nimport type { Setting } from '@/server/audit'\nvoid cn"],
  ]

  it('allows every erased form, all planted at once', () => {
    const { passed, output } = runChecker(plant(SAFE))
    // The whole table in one assertion is deliberate: if ANY of these fires the
    // run is red, and the checker's own output names which file did it.
    expect(passed, `the checker rejected an erased import:\n${output}`).toBe(true)
  })

  // Each of these puts the server module graph into a browser bundle. The
  // asymmetry that decides the hard calls on this rule: a false positive costs
  // somebody a question, a false negative costs the database pool in the client.
  const VIOLATIONS: Array<[string, string]> = [
    ['plain value import', "import { getSetting } from '@/server/audit'"],
    ['default import', "import audit from '@/server/audit'"],
    ['namespace import', "import * as audit from '@/server/audit'"],
    // The one a naive "is every binding a type?" check gets wrong.
    ['mixed value and type in one clause', "import { getSetting, type Setting } from '@/server/audit'"],
    // A binding that merely STARTS with the word type.
    ['a binding named typeahead', "import { typeahead } from '@/server/audit'"],
    // No `from` at all — invisible to the original rule, and it still evaluates
    // the module for its side effects.
    ['side-effect import', "import '@/server/audit'"],
    // The nastiest: the pool arrives in a module that never named @/server.
    ['named re-export', "export { getSetting } from '@/server/audit'"],
    ['star re-export', "export * from '@/server/audit'"],
    // Runtime dynamic imports, in the contexts that are runtime BY GRAMMAR.
    ['await import', "const m = await import('@/server/audit')\nvoid m"],
    ['import().then', "void import('@/server/audit').then(() => {})"],
    ['return import', "export function f() { return import('@/server/audit') }"],
    ['void import', "void import('@/server/audit')"],
    ['const bound to import', "const p = import('@/server/audit')\nvoid p"],
  ]

  it('catches every violating form, and names each one', () => {
    const { passed, output } = runChecker(plant(VIOLATIONS))
    expect(passed).toBe(false)
    // PER CASE, not just "something failed". Asserting only the exit code would
    // let one caught violation cover for twelve missed ones — which is exactly
    // the failure this rule keeps having.
    const missed = VIOLATIONS.filter(([name]) => !output.includes(fileFor(name))).map(([name]) => name)
    expect(missed, `not reported as violations:\n  ${missed.join('\n  ')}\n\n${output}`).toEqual([])
  })

  it('passes on a probe directory with nothing incriminating in it', () => {
    // THE CONTROL. Without it, a checker that failed for an unrelated reason —
    // a different rule, a syntax error somewhere in the tree — would make every
    // violation case above pass for the wrong reason.
    const { passed, output } = runChecker({ 'clean.ts': "import { cn } from '@/lib/cn'\nexport const probe = cn('a')\n" })
    expect(passed, output).toBe(true)
  })
})
