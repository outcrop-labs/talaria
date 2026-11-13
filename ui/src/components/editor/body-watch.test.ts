import { describe, expect, it } from 'vitest'
import { bodyWatch } from './body-watch'

/** A body the test types into, and a count of how often the watch serialised it. */
const editor = (text: string) => {
  const doc = { text, reads: 0 }
  const watch = bodyWatch(() => {
    doc.reads += 1
    return doc.text
  })
  return { doc, watch }
}

describe('bodyWatch', () => {
  it('reports an edit while the caret is still in the body', () => {
    const { doc, watch } = editor('## What happened\n')
    watch.arm()
    expect(watch.changed()).toBe(false)
    doc.text = '## What happened\n\nAttach the BOL.\n'
    expect(watch.changed()).toBe(true)
  })

  it('does not report keys that leave the text as it was', () => {
    const { watch } = editor('Steps to reproduce')
    watch.arm()
    expect(watch.changed()).toBe(false)
    expect(watch.changed()).toBe(false)
  })

  it('stops reporting once the text is back where it started', () => {
    const { doc, watch } = editor('Done when')
    watch.arm()
    doc.text = 'Done when it ships'
    expect(watch.changed()).toBe(true)
    doc.text = 'Done when'
    expect(watch.changed()).toBe(false)
  })

  it('keeps the first remembered body across later key downs', () => {
    const { doc, watch } = editor('a')
    watch.arm()
    doc.text = 'ab'
    watch.arm()
    expect(watch.changed()).toBe(true)
  })

  it('takes the saved text as clean when a save lands with the caret in the body', () => {
    const { doc, watch } = editor('a')
    watch.arm()
    doc.text = 'ab'
    expect(watch.changed()).toBe(true)
    watch.reset()
    // The key up of ⌘S arrives with nothing armed.
    expect(watch.changed()).toBe(false)
    doc.text = 'abc'
    expect(watch.changed()).toBe(true)
  })

  it('compares against the restored text after a discard', () => {
    const { doc, watch } = editor('saved')
    watch.arm()
    doc.text = 'saved and more'
    expect(watch.changed()).toBe(true)
    doc.text = 'saved'
    watch.reset()
    watch.arm()
    expect(watch.changed()).toBe(false)
  })

  it('serialises the body once to arm, however many keys go down', () => {
    const { doc, watch } = editor('a')
    watch.arm()
    watch.arm()
    watch.arm()
    expect(doc.reads).toBe(1)
  })
})
