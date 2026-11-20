import { describe, expect, it } from 'vitest'
import { hasUnsavedEdits, holdEdits, moveWouldDropEdits } from './pane-edits'

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
  it('reads an editor whose answer throws as holding edits', () => {
    const release = holdEdits('a3', () => {
      throw new Error('torn down')
    })
    expect(hasUnsavedEdits('a3')).toBe(true)
    release()
    expect(hasUnsavedEdits('a3')).toBe(false)
  })
  it('guards a plan document tab by the plan document, and an artifact tab by its file', () => {
    let typing = true
    const releasePlan = holdEdits('plan-doc', () => typing)
    const releaseFile = holdEdits('file-1', () => true)
    const plan = { id: 'doc', kind: 'doc' }
    const file = { id: 'artifact:file-1', kind: 'artifact', ref: 'file-1' }
    // Typing in the plan: an agent open elsewhere does not move the pane.
    expect(moveWouldDropEdits(plan, 'artifact:other', 'plan-doc')).toBe(true)
    // Nor out of a file with unsaved edits.
    expect(moveWouldDropEdits(file, 'google:abc', 'plan-doc')).toBe(true)
    // The same tab, nothing shown, or nothing unsaved: free to move.
    expect(moveWouldDropEdits(file, 'artifact:file-1', 'plan-doc')).toBe(false)
    expect(moveWouldDropEdits(null, 'artifact:other', 'plan-doc')).toBe(false)
    expect(moveWouldDropEdits(plan, 'artifact:other', null)).toBe(false)
    expect(moveWouldDropEdits({ id: 'report', kind: 'report' }, 'artifact:other', 'plan-doc')).toBe(false)
    typing = false
    expect(moveWouldDropEdits(plan, 'artifact:other', 'plan-doc')).toBe(false)
    releasePlan()
    releaseFile()
  })
})
