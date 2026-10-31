import { describe, expect, it, vi } from 'vitest'
import { hasDotSegment, isRowId, openNotification, safeInAppHref } from './notify-open'

const ORIGIN = 'https://talaria.example.com'

describe('safeInAppHref', () => {
  it('keeps a path on this origin, query and hash included', () => {
    expect(safeInAppHref('/boards/b/t', ORIGIN)).toBe('/boards/b/t')
    expect(safeInAppHref(' /comms/channel/c?x=1#m ', ORIGIN)).toBe('/comms/channel/c?x=1#m')
    expect(safeInAppHref('/', ORIGIN)).toBe('/')
  })

  it('refuses anything that could leave the instance', () => {
    for (const bad of [
      'https://evil.example',
      '//evil.example',
      '/\\evil.example',
      'javascript:alert(1)',
      '/a\nb',
      'evil.example/x',
      '',
      null,
      undefined,
      `/${'a'.repeat(2048)}`,
    ]) {
      expect(safeInAppHref(bad, ORIGIN)).toBeNull()
    }
  })

  // `/..//evil.example` resolves to the PATH `//evil.example` on this origin,
  // and that path, handed to a router or a worker's navigate(), is another
  // host. The same table as talaria-notify's
  // `openable_href_refuses_dot_segments_in_every_spelling` and sw.test.ts.
  it('refuses a dot segment in every spelling, so no resolved path can be //host', () => {
    // The hazard itself: each of these resolves to the path `//evil.example`.
    for (const bad of DOT_SEGMENT_HREFS.filter((h) => h.endsWith('//evil.example'))) {
      expect(new URL(bad, ORIGIN).pathname).toBe('//evil.example')
    }
    for (const bad of DOT_SEGMENT_HREFS) {
      expect(safeInAppHref(bad, ORIGIN)).toBeNull()
      expect(hasDotSegment(bad)).toBe(true)
    }
    for (const good of DOT_LOOKALIKES) {
      expect(safeInAppHref(good, ORIGIN)).toBe(good)
      expect(hasDotSegment(good)).toBe(false)
    }
  })
})

const DOT_SEGMENT_HREFS = [
  '/..//evil.example',
  '/.//evil.example',
  '/a/..//evil.example',
  '/%2e%2e//evil.example',
  '/%2E%2e//evil.example',
  '/.%2e//evil.example',
  '/%2e.//evil.example',
  '/%2e//evil.example',
  '/a/b/../../..//evil.example',
  '/boards/..',
  '/boards/.',
  '/./boards',
]
const DOT_LOOKALIKES = ['/.../x', '/..a//x', '/a.b/c', '/x?next=/..//evil.example', '/x#/..//evil.example', '/%2e%2ea/x']

describe('isRowId', () => {
  it('is a uuid, and nothing else', () => {
    expect(isRowId('1a5bba79-835d-495f-bd4e-fed9df6c701d')).toBe(true)
    for (const not of ['', 'talaria-summary', 'n1', null, undefined]) expect(isRowId(not)).toBe(false)
  })
})

describe('openNotification', () => {
  const ROW = '1a5bba79-835d-495f-bd4e-fed9df6c701d'
  const deps = () => ({
    navigate: vi.fn(async () => {}),
    markRead: vi.fn(async () => {}),
    invalidate: vi.fn(async () => {}),
    origin: ORIGIN,
  })

  it('routes in place and marks the row read', async () => {
    const d = deps()
    await expect(openNotification({ id: ROW, href: '/boards/b/t' }, d)).resolves.toBe('/boards/b/t')
    expect(d.navigate).toHaveBeenCalledWith('/boards/b/t')
    expect(d.markRead).toHaveBeenCalledWith([ROW])
    expect(d.invalidate).toHaveBeenCalled()
  })

  it('a foreign or dot-segment href routes nowhere — never off the instance — and still reads the row', async () => {
    for (const href of ['https://evil.example', '//evil.example', '/..//evil.example', '/%2e%2e//evil.example']) {
      const d = deps()
      await expect(openNotification({ id: ROW, href }, d)).resolves.toBeNull()
      expect(d.navigate).not.toHaveBeenCalled()
      expect(d.markRead).toHaveBeenCalledWith([ROW])
    }
  })

  it('an empty href keeps the person on the page they are on', async () => {
    const d = deps()
    await expect(openNotification({ id: ROW, href: '' }, d)).resolves.toBeNull()
    expect(d.navigate).not.toHaveBeenCalled()
    expect(d.markRead).toHaveBeenCalledWith([ROW])
  })

  it('a failing mark-read or navigation does not throw', async () => {
    const d = { ...deps(), markRead: vi.fn(async () => Promise.reject(new Error('500'))), navigate: vi.fn(() => Promise.reject(new Error('x'))) }
    await expect(openNotification({ id: ROW, href: '/x' }, d)).resolves.toBe('/x')
  })

  it('a key that is not a row (the summary, the confirmation banner) is never marked read', async () => {
    for (const id of ['', 'talaria-summary']) {
      const d = deps()
      await openNotification({ id, href: '' }, d)
      expect(d.markRead).not.toHaveBeenCalled()
      expect(d.navigate).not.toHaveBeenCalled()
    }
  })
})
