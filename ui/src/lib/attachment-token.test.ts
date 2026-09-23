import { describe, expect, it } from 'vitest'

import {
  ATTACHMENT_HREF_RE,
  attachmentHref,
  buildAttachmentToken,
  parseAttachmentHref,
} from './attachment-token'

// The token grammar, both directions. The editor serializes INTO it
// (attachment-chip's storage.markdown.serialize), the read renderer parses
// OUT of it (markdown.ts) — a drift here is a chip that saves but renders as
// a dead link, so the round-trip IS the contract.
//
// Shape: [<filename>](attachment://<uuid>?s=<size>&m=<enc(mime)>)

const UUID = 'e3b0c442-98fc-4221-a441-4a05b3f0e4d2'
const attrs = { id: UUID, mime: 'application/pdf', size: 2048 }

describe('attachmentHref', () => {
  it('emits the exact href grammar', () => {
    expect(attachmentHref(attrs)).toBe(
      `attachment://${UUID}?s=2048&m=${encodeURIComponent('application/pdf')}`,
    )
  })

  it('is the inverse of parseAttachmentHref for every mime the app carries', () => {
    for (const mime of [
      'application/pdf',
      'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet',
      'text/plain',
      'application/zip',
      'image/svg+xml',
      'audio/ogg; codecs=opus',
    ]) {
      const round = parseAttachmentHref(attachmentHref({ ...attrs, mime }))
      expect(round).toEqual({ id: UUID, mime, size: 2048 })
    }
  })
})

describe('buildAttachmentToken', () => {
  it('wraps the href in link syntax with the filename as text', () => {
    expect(buildAttachmentToken('report.pdf', attrs)).toBe(
      `[report.pdf](attachment://${UUID}?s=2048&m=application%2Fpdf)`,
    )
  })

  it('keeps a filename with brackets and parens observable in the token text', () => {
    // state.esc() is the serializer's job; from the builder's side the name
    // rides through untouched, and brackets survive as literal text the
    // markdown link grammar still parses (they are not delimiter runs of
    // length 1 at the text's edges here — both flanks are inside the link).
    const name = 'Q3 [draft] (final).pdf'
    const token = buildAttachmentToken(name, attrs)
    expect(token).toBe(`[${name}](attachment://${UUID}?s=2048&m=application%2Fpdf)`)
    expect(token.startsWith(`[${name}](`)).toBe(true)
  })
})

describe('parseAttachmentHref', () => {
  it('round-trips a built token', () => {
    const token = buildAttachmentToken('report.pdf', attrs)
    // The href is the parenthesized half.
    const href = token.slice(token.indexOf('](') + 2, -1)
    expect(parseAttachmentHref(href)).toEqual(attrs)
  })

  it('rejects non-tokens: https, relative, schemeless, javascript', () => {
    expect(parseAttachmentHref('https://example.com/report.pdf')).toBeNull()
    expect(parseAttachmentHref('/api/uploads/e3b0c442-98fc-4221-a441-4a05b3f0e4d2')).toBeNull()
    expect(parseAttachmentHref('javascript:alert(1)')).toBeNull()
    expect(parseAttachmentHref('attachment:')).toBeNull()
  })

  it('rejects an attachment href without the query', () => {
    expect(parseAttachmentHref(`attachment://${UUID}`)).toBeNull()
  })

  it('rejects a truncated uuid', () => {
    expect(parseAttachmentHref(`attachment://e3b0c442?s=2048&m=application%2Fpdf`)).toBeNull()
  })

  it('rejects an empty size and a non-numeric size', () => {
    expect(parseAttachmentHref(`attachment://${UUID}?s=&m=application%2Fpdf`)).toBeNull()
    expect(parseAttachmentHref(`attachment://${UUID}?s=abc&m=application%2Fpdf`)).toBeNull()
  })

  it('survives a mime carrying + and & (the & cannot smuggle a second param)', () => {
    const round = parseAttachmentHref(attachmentHref({ ...attrs, mime: 'video/mp4&evil=1+x' }))
    expect(round).toEqual({ id: UUID, mime: 'video/mp4&evil=1+x', size: 2048 })
  })

  it('rejects a percent-escape sequence that does not decode', () => {
    expect(parseAttachmentHref(`attachment://${UUID}?s=1&m=%`)).toBeNull()
  })
})

describe('ATTACHMENT_HREF_RE', () => {
  it('is anchored against prefix noise and non-mime suffix characters', () => {
    // A prefixed href fails the ^ anchor. A `#`/space suffix fails the
    // charset — but a trailing LETTER is legitimately part of the mime
    // (the charset is the URI-encoded set); the parser only ever sees
    // whole attribute values, so that is not a boundary the grammar needs.
    expect(ATTACHMENT_HREF_RE.test(`x${attachmentHref(attrs)}`)).toBe(false)
    expect(ATTACHMENT_HREF_RE.test(`${attachmentHref(attrs)}#fragment`)).toBe(false)
    expect(ATTACHMENT_HREF_RE.test(`${attachmentHref(attrs)} q`)).toBe(false)
  })
})
