import { describe, expect, it, vi } from 'vitest'
import { openNotification, safeInAppHref } from './notify-open'

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
})

describe('openNotification', () => {
  const deps = () => ({
    navigate: vi.fn(async () => {}),
    markRead: vi.fn(async () => {}),
    invalidate: vi.fn(async () => {}),
    origin: ORIGIN,
  })

  it('routes in place and marks the row read', async () => {
    const d = deps()
    await expect(openNotification({ id: 'n1', href: '/boards/b/t' }, d)).resolves.toBe('/boards/b/t')
    expect(d.navigate).toHaveBeenCalledWith('/boards/b/t')
    expect(d.markRead).toHaveBeenCalledWith(['n1'])
    expect(d.invalidate).toHaveBeenCalled()
  })

  it('sends a foreign href to the home page, never off the instance', async () => {
    for (const href of ['https://evil.example', '//evil.example']) {
      const d = deps()
      await expect(openNotification({ id: 'n1', href }, d)).resolves.toBe('/')
      expect(d.navigate).toHaveBeenCalledWith('/')
      expect(d.markRead).toHaveBeenCalledWith(['n1'])
    }
  })

  it('a failing mark-read or navigation does not throw', async () => {
    const d = { ...deps(), markRead: vi.fn(async () => Promise.reject(new Error('500'))), navigate: vi.fn(() => Promise.reject(new Error('x'))) }
    await expect(openNotification({ id: 'n1', href: '/x' }, d)).resolves.toBe('/x')
  })

  it('a summary (no id) only routes', async () => {
    const d = deps()
    await openNotification({ id: '', href: '/' }, d)
    expect(d.markRead).not.toHaveBeenCalled()
  })
})
