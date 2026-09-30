// THE EMBED URL PER FILE TYPE. Google serves a different embed path for each
// editor, and the wrong one renders a blank frame rather than an error — so
// this mapping is the whole reliability story and it lives in one place,
// testable, rather than inline in the markup.
//
// Docs and Sheets take `?embedded=true` on their normal /edit URL and stay
// EDITABLE inside the frame. Slides has no embedded editor: /embed is a
// player, so a deck is presented rather than edited — which matches the
// agent's own tools, since nothing in Talaria edits a deck either. Anything
// that is not a native Google type (a PDF, an image, a CSV) has no editor at
// all and gets Drive's /preview.
export function embedUrl(fileId: string, mime: string | null | undefined): string {
  const id = encodeURIComponent(fileId)
  switch (mime) {
    case 'application/vnd.google-apps.document':
      return `https://docs.google.com/document/d/${id}/edit?embedded=true&rm=demo`
    case 'application/vnd.google-apps.spreadsheet':
      return `https://docs.google.com/spreadsheets/d/${id}/edit?embedded=true&rm=demo`
    case 'application/vnd.google-apps.presentation':
      return `https://docs.google.com/presentation/d/${id}/embed`
    default:
      return `https://drive.google.com/file/d/${id}/preview`
  }
}

/** Where "Open in Google" goes — the real editor, in a real tab, with the
 *  browser's own Google session. Always offered: see the note on the frame. */
export function openUrl(fileId: string, mime: string | null | undefined): string {
  const id = encodeURIComponent(fileId)
  switch (mime) {
    case 'application/vnd.google-apps.document':
      return `https://docs.google.com/document/d/${id}/edit`
    case 'application/vnd.google-apps.spreadsheet':
      return `https://docs.google.com/spreadsheets/d/${id}/edit`
    case 'application/vnd.google-apps.presentation':
      return `https://docs.google.com/presentation/d/${id}/edit`
    default:
      return `https://drive.google.com/file/d/${id}/view`
  }
}

export function isEditableEmbed(mime: string | null | undefined): boolean {
  return (
    mime === 'application/vnd.google-apps.document' ||
    mime === 'application/vnd.google-apps.spreadsheet'
  )
}

/** What a pasted Google URL names. `mime` is inferred from the URL SHAPE, not
 *  fetched: /document/, /spreadsheets/ and /presentation/ each map to exactly
 *  one native type, and a Drive /file/ link is anything else, which the pane
 *  renders as a preview. Returns null for a URL that is not a Google file, so
 *  the caller can say so rather than pinning a broken frame. */
export function parseGoogleFileUrl(raw: string): { id: string; mime: string | null } | null {
  let url: URL
  try {
    url = new URL(raw.trim())
  } catch {
    return null
  }
  if (url.protocol !== 'https:') return null
  const host = url.hostname.toLowerCase()
  if (host !== 'docs.google.com' && host !== 'drive.google.com') return null
  // /<kind>/d/<id>/… — the id is the segment after "d".
  const parts = url.pathname.split('/').filter(Boolean)
  const d = parts.indexOf('d')
  if (d === -1) return null
  const id = parts[d + 1]
  if (!id || !/^[A-Za-z0-9_-]{1,200}$/.test(id)) return null
  const kind = parts[0]
  const mime =
    kind === 'document'
      ? 'application/vnd.google-apps.document'
      : kind === 'spreadsheets'
        ? 'application/vnd.google-apps.spreadsheet'
        : kind === 'presentation'
          ? 'application/vnd.google-apps.presentation'
          : null
  return { id, mime }
}
