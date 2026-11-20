import { describe, expect, it } from 'vitest'
import { hasUnsavedEdits, holdEdits } from './pane-edits'

describe('pane edits', () => {
  it('answers what the editor showing a file says, and nothing once it is gone', () => {
    let dirty = true
    const release = holdEdits('a1', () => dirty)
    expect(hasUnsavedEdits('a1')).toBe(true)
    dirty = false
    expect(hasUnsavedEdits('a1')).toBe(false)
    dirty = true
    release()
    expect(hasUnsavedEdits('a1')).toBe(false)
    expect(hasUnsavedEdits(null)).toBe(false)
  })
  it('lets a remounted editor replace the old one without the old release removing it', () => {
    const releaseOld = holdEdits('a2', () => false)
    const releaseNew = holdEdits('a2', () => true)
    releaseOld()
    expect(hasUnsavedEdits('a2')).toBe(true)
    releaseNew()
    expect(hasUnsavedEdits('a2')).toBe(false)
  })
})
