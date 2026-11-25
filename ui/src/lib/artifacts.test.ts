import { describe, expect, it } from 'vitest'
import { canEditArtifact, initialMode, officeExport, officeExportHref } from './artifacts'

describe('officeExport', () => {
  it('names the office format a kind can leave as, or nothing', () => {
    expect(officeExport('file')).toBeNull()
    expect(officeExport(null)).toBeNull()
    expect(officeExport(undefined)).toBeNull()
    expect(officeExport('sheet')).toEqual({ format: 'xlsx', label: 'Excel (.xlsx)' })
    expect(officeExport('doc')).toEqual({ format: 'docx', label: 'Word (.docx)' })
    expect(officeExport('microsite')).toEqual({ format: 'docx', label: 'Word (.docx)' })
    expect(officeExport('run-transcript')).toEqual({ format: 'docx', label: 'Word (.docx)' })
  })
  it('builds the export route', () => {
    expect(officeExportHref('a1', 'docx')).toBe('/api/artifacts/a1/export?format=docx')
  })
})

describe('canEditArtifact', () => {
  it('reads the server\'s answer, and an absent one as editable', () => {
    expect(canEditArtifact({ canEdit: true })).toBe(true)
    expect(canEditArtifact({})).toBe(true)
    expect(canEditArtifact({ canEdit: false })).toBe(false)
  })
})

describe('initialMode', () => {
  it('opens a viewer in Read even on an empty file, and an editor in Edit only there', () => {
    expect(initialMode({ canEdit: false, body: '' })).toBe('read')
    expect(initialMode({ canEdit: false, body: '  \n' })).toBe('read')
    expect(initialMode({ canEdit: true, body: '' })).toBe('edit')
    expect(initialMode({ body: '' })).toBe('edit')
    expect(initialMode({ canEdit: true, body: '# Plan' })).toBe('read')
    expect(initialMode({ canEdit: false, body: '# Plan' })).toBe('read')
  })
})
