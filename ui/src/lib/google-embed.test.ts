import { describe, expect, it } from 'vitest'
import { embedUrl, isEditableEmbed, openUrl, parseGoogleFileUrl } from './google-embed'

// The mapping IS the feature: the wrong embed path renders a blank frame
// rather than an error, so a silent regression here looks like "Google is
// broken" to whoever is using it.
const DOC = 'application/vnd.google-apps.document'
const SHEET = 'application/vnd.google-apps.spreadsheet'
const DECK = 'application/vnd.google-apps.presentation'

describe('embedUrl', () => {
  it('embeds a doc and a sheet in their editable editors', () => {
    expect(embedUrl('abc', DOC)).toBe('https://docs.google.com/document/d/abc/edit?embedded=true&rm=demo')
    expect(embedUrl('abc', SHEET)).toBe('https://docs.google.com/spreadsheets/d/abc/edit?embedded=true&rm=demo')
  })

  it('embeds a deck as a player, because Slides has no embedded editor', () => {
    expect(embedUrl('abc', DECK)).toBe('https://docs.google.com/presentation/d/abc/embed')
  })

  it('falls back to Drive preview for anything that is not a native type', () => {
    expect(embedUrl('abc', 'application/pdf')).toBe('https://drive.google.com/file/d/abc/preview')
    expect(embedUrl('abc', null)).toBe('https://drive.google.com/file/d/abc/preview')
    expect(embedUrl('abc', undefined)).toBe('https://drive.google.com/file/d/abc/preview')
  })

  it('escapes the id rather than pasting it into a URL', () => {
    // A Drive id is alphanumeric in practice, but this string is built into a
    // URL and the server is not the only thing that can put a value here.
    expect(embedUrl('a/b?c', DOC)).toContain('a%2Fb%3Fc')
    expect(embedUrl('a/b?c', DOC)).not.toContain('a/b?c')
  })
})

describe('openUrl', () => {
  it('always points at the real editor, never the embed', () => {
    for (const mime of [DOC, SHEET, DECK, 'application/pdf']) {
      expect(openUrl('abc', mime)).not.toContain('embedded=true')
      expect(openUrl('abc', mime)).not.toContain('/embed')
      expect(openUrl('abc', mime)).not.toContain('/preview')
    }
  })

  it('opens a deck in its editor even though the embed only plays it', () => {
    expect(openUrl('abc', DECK)).toBe('https://docs.google.com/presentation/d/abc/edit')
  })
})

describe('isEditableEmbed', () => {
  it('is true only where Google actually lets you type in the frame', () => {
    expect(isEditableEmbed(DOC)).toBe(true)
    expect(isEditableEmbed(SHEET)).toBe(true)
    // A deck embeds as a player and a PDF as a preview — claiming either is
    // editable would put an "edit here" affordance on a frame that ignores it.
    expect(isEditableEmbed(DECK)).toBe(false)
    expect(isEditableEmbed('application/pdf')).toBe(false)
    expect(isEditableEmbed(null)).toBe(false)
  })
})

describe('parseGoogleFileUrl', () => {
  const parse = (u: string) => parseGoogleFileUrl(u)

  it('reads the type from the URL shape', () => {
    expect(parse('https://docs.google.com/document/d/ABC123/edit')).toEqual({ id: 'ABC123', mime: DOC })
    expect(parse('https://docs.google.com/spreadsheets/d/ABC123/edit#gid=0')).toEqual({ id: 'ABC123', mime: SHEET })
    expect(parse('https://docs.google.com/presentation/d/ABC123/edit')).toEqual({ id: 'ABC123', mime: DECK })
  })

  it('accepts a Drive file link with no inferable type', () => {
    // A PDF or an image — the pane renders it as a preview, which is correct.
    expect(parse('https://drive.google.com/file/d/ABC123/view?usp=sharing')).toEqual({ id: 'ABC123', mime: null })
  })

  it('tolerates the query and fragment a real pasted link carries', () => {
    expect(parse('  https://docs.google.com/document/d/ABC123/edit?usp=sharing&foo=1#heading=h.x  ')?.id).toBe('ABC123')
  })

  it('refuses anything that is not a Google file link', () => {
    expect(parse('')).toBeNull()
    expect(parse('not a url')).toBeNull()
    expect(parse('https://example.com/document/d/ABC123/edit')).toBeNull()
    // Look-alike hosts must not pass: this value ends up as an iframe src.
    expect(parse('https://docs.google.com.evil.test/document/d/ABC/edit')).toBeNull()
    // http is refused; the frame is https-only.
    expect(parse('http://docs.google.com/document/d/ABC123/edit')).toBeNull()
    // No /d/ segment means no file id.
    expect(parse('https://drive.google.com/drive/my-drive')).toBeNull()
  })
})
