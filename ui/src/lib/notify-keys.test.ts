import { describe, expect, it } from 'vitest'
import { KEY_HELP, keyAction, moveFocus, readToggle, refocus, targetsOf, toggleSelected } from './notify-keys'

describe('keyAction', () => {
  it('maps the keys', () => {
    expect(keyAction({ key: 'j' })).toBe('next')
    expect(keyAction({ key: 'ArrowDown' })).toBe('next')
    expect(keyAction({ key: 'k' })).toBe('prev')
    expect(keyAction({ key: 'ArrowUp' })).toBe('prev')
    expect(keyAction({ key: 'Enter' })).toBe('open')
    expect(keyAction({ key: 'o' })).toBe('open')
    expect(keyAction({ key: 'e' })).toBe('done')
    expect(keyAction({ key: 'u' })).toBe('toggleRead')
    expect(keyAction({ key: 'x' })).toBe('select')
    expect(keyAction({ key: 'Escape' })).toBe('escape')
    expect(keyAction({ key: 'z' })).toBeNull()
  })

  it('reads Shift+E however the browser spells it, and ? with its Shift', () => {
    expect(keyAction({ key: 'E', shiftKey: true })).toBe('doneAll')
    expect(keyAction({ key: 'e', shiftKey: true })).toBe('doneAll')
    expect(keyAction({ key: '?', shiftKey: true })).toBe('help')
  })

  it('leaves fields and chords alone', () => {
    expect(keyAction({ key: 'j', editable: true })).toBeNull()
    expect(keyAction({ key: 'Escape', editable: true })).toBeNull()
    expect(keyAction({ key: 'e', ctrlKey: true })).toBeNull()
    expect(keyAction({ key: 'k', metaKey: true })).toBeNull()
    expect(keyAction({ key: 'u', altKey: true })).toBeNull()
  })

  it('documents every action it can produce', () => {
    expect(KEY_HELP.length).toBeGreaterThanOrEqual(9)
  })
})

describe('focus', () => {
  it('starts at the first row and clamps', () => {
    expect(moveFocus(-1, 5, 'next')).toBe(0)
    expect(moveFocus(-1, 5, 'prev')).toBe(4)
    expect(moveFocus(0, 5, 'prev')).toBe(0)
    expect(moveFocus(4, 5, 'next')).toBe(4)
    expect(moveFocus(2, 5, 'next')).toBe(3)
    expect(moveFocus(0, 0, 'next')).toBe(-1)
  })

  it('j×3 from the first row (where the page starts) lands on the fourth', () => {
    let i = 0
    for (let n = 0; n < 3; n++) i = moveFocus(i, 10, 'next')
    expect(i).toBe(3)
  })

  it('keeps the focus on the row, or its place, when the list changes', () => {
    expect(refocus('b', 1, ['a', 'b', 'c'])).toBe(1)
    expect(refocus('b', 1, ['a', 'c'])).toBe(1)
    expect(refocus('c', 2, ['a', 'b'])).toBe(1)
    expect(refocus('x', 0, [])).toBe(-1)
    expect(refocus(null, -1, ['a'])).toBe(-1)
  })
})

describe('selection and targets', () => {
  it('toggles', () => {
    const s = toggleSelected(new Set(), 'a')
    expect([...s]).toEqual(['a'])
    expect([...toggleSelected(s, 'a')]).toEqual([])
  })

  it('applies to the visible selection, else the focused row, else (doneAll) the view', () => {
    const visible = ['a', 'b', 'c']
    expect(targetsOf('done', { focused: 'b', selection: new Set(), visible })).toEqual(['b'])
    expect(targetsOf('done', { focused: 'b', selection: new Set(['c', 'a']), visible })).toEqual(['a', 'c'])
    expect(targetsOf('doneAll', { focused: 'b', selection: new Set(), visible })).toEqual(['a', 'b', 'c'])
    expect(targetsOf('doneAll', { focused: null, selection: new Set(['b', 'gone']), visible })).toEqual(['b'])
    expect(targetsOf('toggleRead', { focused: null, selection: new Set(), visible })).toEqual([])
  })

  it('u reads when anything is unread, else makes all unread', () => {
    expect(readToggle([{ readAt: null }, { readAt: '2026-11-01T00:00:00Z' }])).toBe('read')
    expect(readToggle([{ readAt: '2026-11-01T00:00:00Z' }])).toBe('unread')
  })
})
