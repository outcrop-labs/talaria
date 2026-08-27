import { describe, expect, it, vi } from 'vitest'

// serveUpload is the app's whole inline-content policy: WHICH types the
// browser may render same-origin, and the headers that keep everything else
// inert (audit 2026-08-26, P0-1). The two bytes routes (uploads.$id,
// artifacts.public.$slug.download) both serve through it — these tests pin
// the policy at the one place it lives.
vi.mock('./db/pg', () => ({ db: async () => Object.assign(() => Promise.resolve([]), { json: (v: unknown) => v, unsafe: () => Promise.resolve([]) }) }))
vi.mock('./audit', () => ({ getSetting: async (_k: string, fallback: unknown) => fallback, setSetting: async () => {} }))

const { serveUpload } = await import('./uploads')

const bytes = Buffer.from('x')
const serve = (mime: string, filename = 'f.txt') => serveUpload({ bytes, mime, filename }, { cache: 'private' })

describe('serveUpload — the inline allowlist', () => {
  it('renders raster images and PDFs inline', () => {
    for (const mime of ['image/jpeg', 'image/png', 'image/gif', 'image/webp', 'image/avif', 'application/pdf']) {
      expect(serve(mime).headers.get('content-disposition'), mime).toMatch(/^inline/)
    }
  })

  it('downloads everything script-capable — svg and every text/*', () => {
    // The uploader declares the MIME; text/html and image/svg+xml execute
    // same-origin when rendered, so they must never be inline.
    for (const mime of ['image/svg+xml', 'text/html', 'text/plain', 'text/csv', 'application/octet-stream', 'application/javascript']) {
      expect(serve(mime).headers.get('content-disposition'), mime).toMatch(/^attachment/)
    }
  })

  it('never lets the browser sniff a different type', () => {
    expect(serve('text/html').headers.get('x-content-type-options')).toBe('nosniff')
    expect(serve('image/png').headers.get('x-content-type-options')).toBe('nosniff')
  })

  it('sandboxes downloads so even a mislisted type stays inert when navigated to', () => {
    expect(serve('image/svg+xml').headers.get('content-security-policy')).toBe("default-src 'none'; sandbox")
    // Inline types are inert by construction (raster data / the browser's own
    // PDF viewer) — no sandbox needed there, and it would fight PDF rendering.
    expect(serve('application/pdf').headers.get('content-security-policy')).toBeNull()
  })

  it('strips header-breaking characters from the filename', () => {
    const h = serve('text/plain', 'a"\r\nX-Evil: 1.txt').headers
    const disposition = h.get('content-disposition') ?? ''
    expect(disposition).not.toContain('\n')
    expect(disposition).not.toContain('\r')
    // The value inside the quoted-string carries neither a quote (break-out)
    // nor the smuggled header name.
    const value = /filename="(.*)"/.exec(disposition)?.[1] ?? ''
    expect(value).not.toContain('"')
    expect(value).toBe('aX-Evil: 1.txt')
  })
})
