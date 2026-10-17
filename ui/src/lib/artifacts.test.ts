import { describe, expect, it } from 'vitest'
import { officeExport, officeExportHref } from './artifacts'

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
