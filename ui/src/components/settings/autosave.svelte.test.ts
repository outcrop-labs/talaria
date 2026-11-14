// @vitest-environment runes
//
// The autosave cue's field count (autosave.svelte.ts): the cue is on while
// any field that saves on change is mounted, and off when the last one goes.
// A tabbed page mounts the next tab's fields in the same update that unmounts
// the last tab's, which is the case these tests are for.
import assert from 'node:assert/strict'
import { test } from 'vitest'
import { createAutosave } from './autosave.svelte'

/** Let the client runtime run its queued effects (`flushSync` from 'svelte'
 *  is the server build in this environment). */
const settle = () => new Promise((resolve) => setTimeout(resolve, 0))

// A page of tabs with no DOM: each tab's fields register while it is the
// current tab. `order` is the order the tabs' blocks sit on the page, so a
// later tab's fields unmount after an earlier tab's have mounted.
async function page(order: string[], fields: Record<string, number>, first: string) {
  const cue = createAutosave()
  let tab = $state(first)
  const stop = $effect.root(() => {
    for (const name of order) {
      $effect(() => {
        if (tab !== name) return
        // A root, like a component: its body runs once and tracks nothing for the block around it.
        const count = fields[name] ?? 0
        return $effect.root(() => {
          for (let i = 0; i < count; i++) cue.channel.register()
          return () => {
            for (let i = 0; i < count; i++) cue.channel.release()
          }
        })
      })
    }
  })
  await settle()
  return {
    cue,
    async go(next: string) {
      tab = next
      await settle()
    },
    stop,
  }
}

test('the cue is on while a field is mounted and off after the last one goes', async () => {
  const p = await page(['only'], { only: 2 }, 'only')
  assert.equal(p.cue.active, true)
  p.stop()
  assert.equal(p.cue.active, false)
})

test('the cue stays on when one tab of fields replaces another in the same update', async () => {
  const p = await page(['next', 'last'], { last: 2, next: 1 }, 'last')
  assert.equal(p.cue.active, true)
  await p.go('next')
  assert.equal(p.cue.active, true)
  p.stop()
})

test('the cue goes when the tab that replaces the fields has none', async () => {
  const p = await page(['empty', 'last'], { last: 3, empty: 0 }, 'last')
  await p.go('empty')
  assert.equal(p.cue.active, false)
  p.stop()
})

test('the cue survives a walk through every tab and back', async () => {
  const p = await page(['a', 'b', 'c', 'd'], { a: 4, b: 1, c: 0, d: 1 }, 'a')
  for (const next of ['b', 'c', 'd', 'b', 'a', 'd', 'c', 'b']) {
    await p.go(next)
    assert.equal(p.cue.active, next !== 'c', `on tab ${next}`)
  }
  p.stop()
  assert.equal(p.cue.active, false)
})
