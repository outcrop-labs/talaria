// @vitest-environment runes
//
// The autosave cue (autosave.svelte.ts): the header cue is on while any field
// that saves on change is mounted, and off when the last one goes. A group
// with its own Save turns it off while that group is mounted, and a save that
// lands then flashes its row instead. A tabbed page mounts the next tab's
// fields in the same update that unmounts the last tab's, which is the case
// the tab tests are for.
import assert from 'node:assert/strict'
import { test } from 'vitest'
import { createAutosave, createRowSaved } from './autosave.svelte'

/** Let the client runtime run its queued effects (`flushSync` from 'svelte'
 *  is the server build in this environment). */
const settle = () => new Promise((resolve) => setTimeout(resolve, 0))

// A page of tabs with no DOM: each tab's fields register while it is the
// current tab. `order` is the order the tabs' blocks sit on the page, so a
// later tab's fields unmount after an earlier tab's have mounted.
async function page(order: string[], fields: Record<string, number>, first: string, saves: Record<string, number> = {}) {
  const cue = createAutosave()
  let tab = $state(first)
  const stop = $effect.root(() => {
    for (const name of order) {
      $effect(() => {
        if (tab !== name) return
        // A root, like a component: its body runs once and tracks nothing for the block around it.
        const count = fields[name] ?? 0
        const bars = saves[name] ?? 0
        return $effect.root(() => {
          for (let i = 0; i < count; i++) cue.channel.register()
          for (let i = 0; i < bars; i++) cue.channel.hold()
          return () => {
            for (let i = 0; i < count; i++) cue.channel.release()
            for (let i = 0; i < bars; i++) cue.channel.drop()
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

test('a group with its own Save turns the cue off, whatever the page says', () => {
  const cue = createAutosave(1800, () => true)
  assert.equal(cue.active, true)
  cue.channel.hold()
  assert.equal(cue.active, false)
  cue.channel.hold()
  cue.channel.drop()
  assert.equal(cue.active, false, 'one of two save groups is still mounted')
  cue.channel.drop()
  assert.equal(cue.active, true)
})

test('fields that save on change cannot bring the cue back beside a Save', () => {
  const cue = createAutosave()
  cue.channel.hold()
  cue.channel.register()
  cue.channel.register()
  assert.equal(cue.active, false)
  cue.channel.drop()
  assert.equal(cue.active, true)
})

test('a save flashes the header only while the header shows the cue', () => {
  const cue = createAutosave()
  assert.equal(cue.channel.flash(), false, 'no cue: the row flashes instead')
  assert.equal(cue.saved, false)
  cue.channel.register()
  assert.equal(cue.channel.flash(), true)
  assert.equal(cue.saved, true)
  cue.dispose()

  const mixed = createAutosave(1800, () => true)
  mixed.channel.hold()
  assert.equal(mixed.channel.flash(), false, 'beside a Save: the row flashes instead')
  assert.equal(mixed.saved, false)
  mixed.dispose()
})

test('the cue stays off when a tab with a Save replaces a tab of fields, and returns on the way back', async () => {
  const p = await page(['mixed', 'auto'], { auto: 2, mixed: 3 }, 'auto', { mixed: 1 })
  assert.equal(p.cue.active, true)
  await p.go('mixed')
  assert.equal(p.cue.active, false)
  await p.go('auto')
  assert.equal(p.cue.active, true)
  p.stop()
  assert.equal(p.cue.active, false)
})

test('a row flashes by its own key, and stops after its time', async () => {
  const rows = createRowSaved(20)
  rows.flash('name')
  assert.equal(rows.saved('name'), true)
  assert.equal(rows.saved('title'), false)
  assert.equal(rows.saved(), false)
  rows.flash()
  assert.equal(rows.saved(), true)
  await new Promise((resolve) => setTimeout(resolve, 60))
  assert.equal(rows.saved('name'), false)
  assert.equal(rows.saved(), false)
  rows.dispose()
})
