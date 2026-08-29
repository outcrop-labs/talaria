import { describe, expect, it, vi } from 'vitest'

// serveUpload is the app's whole inline-content policy: WHICH types the
// browser may render same-origin, and the headers that keep everything else
// inert (audit 2026-08-26, P0-1). The two bytes routes (uploads.$id,
// artifacts.public.$slug.download) both serve through it — these tests pin
// the policy at the one place it lives.
vi.mock('./db/pg', () => ({ db: async () => Object.assign(() => Promise.resolve([]), { json: (v: unknown) => v, unsafe: () => Promise.resolve([]) }) }))
vi.mock('./audit', () => ({ getSetting: async (_k: string, fallback: unknown) => fallback, setSetting: async () => {} }))

const { readUploadForm, serveUpload } = await import('./uploads')

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

// readUploadForm is the DoS stop the 2026-08-26 audit asked for (#266): the
// old route buffered whatever the client sent and only saveUpload's
// byteLength check refused — after the memory was already spent. Two layers
// here, because two kinds of attacker exist: one declares a size (refused
// from the header alone, the body never pulled), one streams chunked with no
// declaration (the stream itself is capped mid-read). The cap is injected —
// production's is 25 MB + envelope; tests pass bytes, not megabytes.
describe('readUploadForm — refusing before buffering', () => {
  const formRequest = () => {
    const fd = new FormData()
    fd.append('file', new File([Buffer.from('tiny')], 'f.txt', { type: 'text/plain' }))
    return new Request('http://x/api/uploads', { method: 'POST', body: fd })
  }

  it('reads a small form through untouched', async () => {
    const read = await readUploadForm(formRequest(), 1024)
    expect(read.ok).toBe(true)
    if (read.ok) expect(read.form.get('file')).toBeInstanceOf(File)
  })

  it('a declared content-length over the cap is refused without waiting on the body', async () => {
    // The stream NEVER ENDS: if readUploadForm resolved too-large anyway, the
    // header alone decided it — a body that never finishes cannot have been
    // buffered. (Undici pulls eagerly at Request construction, so "was never
    // pulled" is not observable here; "did not wait" is the guarantee.)
    const body = new ReadableStream<Uint8Array>({
      pull(c) {
        c.enqueue(Buffer.from('x'))
      },
    })
    const request = new Request('http://x/api/uploads', {
      method: 'POST',
      headers: { 'content-type': 'multipart/form-data; boundary=x', 'content-length': '99999999' },
      body,
      duplex: 'half',
    } as RequestInit)
    expect(await readUploadForm(request, 1024)).toEqual({ ok: false, reason: 'too-large' })
  })

  it('a chunked stream over the cap is aborted mid-read — not buffered to the end', async () => {
    const chunk = Buffer.alloc(1024, 1) // one chunk fills the injected cap
    let sent = 0
    const body = new ReadableStream<Uint8Array>({
      pull(c) {
        sent += 1
        c.enqueue(chunk)
      },
    })
    const request = new Request('http://x/api/uploads', {
      method: 'POST',
      headers: { 'content-type': 'multipart/form-data; boundary=x' },
      body,
      duplex: 'half',
    } as RequestInit)
    const read = await readUploadForm(request, 1024)
    expect(read).toEqual({ ok: false, reason: 'too-large' })
    expect(sent).toBeLessThan(8) // the abort stopped the read, the sender was not drained
  })

  it('garbage that parses as no form at all is malformed, not too-large', async () => {
    const request = new Request('http://x/api/uploads', {
      method: 'POST',
      headers: { 'content-type': 'text/plain' },
      body: 'not a form',
    })
    expect(await readUploadForm(request, 1024)).toEqual({ ok: false, reason: 'malformed' })
  })
})
