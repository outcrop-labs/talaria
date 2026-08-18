import { readdirSync, readFileSync } from 'node:fs'
import { join, relative } from 'node:path'
import assert from 'node:assert/strict'
import { test } from 'vitest'

// THE DRIFT GUARD.
//
// Page surfaces had reached four different widths — 672 / 896 / 1024 / 1152 —
// with `Models` changing width between its OWN tabs and the layout skeleton
// wider than most of the content it replaced. None of that was a decision
// anyone made; it is what happens when a shape lives in a comment and forty
// call sites each spell it out. `PageSurface` and the four width tokens fixed
// it, and this is what stops it coming back, because the failure is invisible
// in review: a `max-w-4xl` on a new view looks exactly as reasonable as the
// `max-w-5xl` on the one beside it.
//
// It scans SOURCE rather than rendered output on purpose. The rule is about
// what an author types — "don't spell a page width at a call site" — and that
// is a property of the text. Rendering would need a browser and would only
// catch the views a test happened to visit.

const SRC = join(import.meta.dirname, '..')

/** Tailwind's page-scale widths. The small ones (xs–md) size fields, not pages. */
const PAGE_SCALE = /\bmax-w-(2xl|3xl|4xl|5xl|6xl|7xl)\b/

/**
 * Comments out, markup in.
 *
 * The rule is about what the MARKUP says. Both of this guard's first two
 * failures were it flagging prose ABOUT the old pattern — `PageSurface`'s own
 * doc comment quotes `mx-auto max-w-5xl` while explaining why that is banned,
 * which is exactly the sentence a reader needs and exactly what a naive text
 * scan cannot tell apart from a violation.
 */
function stripComments(src: string): string {
  return src
    .replace(/<!--[\s\S]*?-->/g, '')
    .replace(/\/\*[\s\S]*?\*\//g, '')
    .replace(/(^|[^:])\/\/[^\n]*/g, '$1')
}

function svelteFiles(dir: string, out: string[] = []): string[] {
  for (const e of readdirSync(dir, { withFileTypes: true })) {
    const full = join(dir, e.name)
    if (e.isDirectory()) svelteFiles(full, out)
    else if (e.name.endsWith('.svelte')) out.push(full)
  }
  return out
}

test('no view spells a page width at its call site', () => {
  // The rule has teeth only on CENTRED columns — `mx-auto` plus a page-scale
  // max-width is the page-frame pattern. A bare `max-w-2xl` on a prose block
  // inside a panel is a measure, not a frame, and is none of this test's
  // business.
  const offenders: string[] = []
  for (const file of svelteFiles(SRC)) {
    const src = stripComments(readFileSync(file, 'utf8'))
    for (const [i, line] of src.split('\n').entries()) {
      if (!line.includes('mx-auto')) continue
      if (!PAGE_SCALE.test(line)) continue
      offenders.push(`${relative(SRC, file)}:${i + 1}  ${line.trim().slice(0, 90)}`)
    }
  }
  assert.deepEqual(
    offenders,
    [],
    `centred columns must use a width token (--page-width / --converse-width / --read-width), ` +
      `normally by rendering <PageSurface>. Offenders:\n  ${offenders.join('\n  ')}`,
  )
})

test('PageSurface has no width escape hatch', () => {
  // The comment in PageSurface argues for this; the test is what makes it true
  // next year. A `width` prop would be taken by the first table that felt
  // cramped, and one view opting out is the whole drift returning.
  const raw = readFileSync(join(SRC, 'components/app/PageSurface.svelte'), 'utf8')
  const src = stripComments(raw)
  assert.ok(src.includes('max-w-[var(--page-width)]'), 'PageSurface stopped using the token')
  const props = src.slice(src.indexOf('let {'), src.indexOf('$props()'))
  assert.ok(!/\bwidth\s*\??:/.test(props), 'PageSurface grew a width prop')
  assert.ok(!PAGE_SCALE.test(src), 'PageSurface hardcodes a Tailwind page width')
})

test('every width token the app references is defined', () => {
  // A typo'd `var(--pagewidth)` renders as NO max-width at all — the column
  // silently spans the viewport, which reads as a layout bug rather than a
  // missing variable, and nothing else would catch it.
  const css = readFileSync(join(SRC, 'styles.css'), 'utf8')
  const used = new Set<string>()
  for (const file of svelteFiles(SRC)) {
    for (const m of stripComments(readFileSync(file, 'utf8')).matchAll(/max-w-\[var\((--[a-z-]+)\)\]/g)) {
      used.add(m[1]!)
    }
  }
  assert.ok(used.size > 0, 'no width tokens in use — did the class spelling change?')
  for (const token of used) {
    assert.ok(css.includes(`${token}:`), `${token} is used but never defined in styles.css`)
  }
})
