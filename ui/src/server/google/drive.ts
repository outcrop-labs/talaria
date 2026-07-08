// Google Drive service — currently: push a Talaria artifact into the user's
// Drive as a native Google Doc / Sheet (or an unconverted file). Acts strictly
// as the connected user via their stored token (per-user OAuth), so Drive's own
// sharing rules govern what lands where.

import { getUpload } from '../uploads'
import type { Artifact } from '../artifacts'
import { getAccessToken } from './connections'

const UPLOAD_ENDPOINT =
  'https://www.googleapis.com/upload/drive/v3/files?uploadType=multipart&fields=id,webViewLink,name,mimeType'

const GOOGLE_DOC = 'application/vnd.google-apps.document'
const GOOGLE_SHEET = 'application/vnd.google-apps.spreadsheet'

export interface DriveFile {
  id: string
  url: string
  name: string
  mimeType: string
}

/** A sheet's JSON grid (`string[][]`, row 0 = header) → CSV text. */
function sheetToCsv(body: string): string {
  let grid: string[][]
  try {
    const g = JSON.parse(body)
    grid = Array.isArray(g) ? g.map((r: unknown[]) => (Array.isArray(r) ? r.map((c) => String(c ?? '')) : [])) : []
  } catch {
    return body
  }
  const esc = (s: string) => (/[",\n]/.test(s) ? `"${s.replace(/"/g, '""')}"` : s)
  return grid.map((row) => row.map(esc).join(',')).join('\r\n')
}

interface MediaPart {
  contentType: string
  /** UTF-8 text body, OR base64-encoded bytes when `base64` is true. */
  data: string
  base64?: boolean
}

/** How each artifact kind maps onto a Drive upload. null ⇒ not exportable. */
async function mediaFor(a: Artifact): Promise<{ targetMime: string; media: MediaPart } | null> {
  switch (a.kind) {
    case 'doc':
      return { targetMime: GOOGLE_DOC, media: { contentType: 'text/markdown', data: a.body || '' } }
    case 'microsite':
      // Drive converts HTML → Doc; the microsite body is HTML.
      return { targetMime: GOOGLE_DOC, media: { contentType: 'text/html', data: a.body || '' } }
    case 'sheet':
      return { targetMime: GOOGLE_SHEET, media: { contentType: 'text/csv', data: sheetToCsv(a.body) } }
    case 'file': {
      // Upload the raw bytes unconverted, preserving the original type.
      if (!a.storageRef) return null
      const found = await getUpload(a.storageRef)
      if (!found) return null
      return {
        targetMime: found.mime || a.contentType || 'application/octet-stream',
        media: { contentType: found.mime || 'application/octet-stream', data: found.bytes.toString('base64'), base64: true },
      }
    }
    default:
      return null
  }
}

function buildMultipart(metadata: object, media: MediaPart): { body: Buffer; boundary: string } {
  const boundary = 'talaria-drive-' + Buffer.from(String(metadata)).toString('hex').slice(0, 16)
  const enc = media.base64 ? 'Content-Transfer-Encoding: base64\r\n' : ''
  const head =
    `--${boundary}\r\n` +
    `Content-Type: application/json; charset=UTF-8\r\n\r\n` +
    `${JSON.stringify(metadata)}\r\n` +
    `--${boundary}\r\n` +
    `Content-Type: ${media.contentType}\r\n${enc}\r\n`
  const tail = `\r\n--${boundary}--`
  return { body: Buffer.concat([Buffer.from(head), Buffer.from(media.data), Buffer.from(tail)]), boundary }
}

/** Export an artifact into the given user's Drive. Returns the created file, or
 *  throws with a caller-friendly message when the user isn't connected. */
export async function exportArtifactToDrive(userId: string, artifact: Artifact, nowMs: number): Promise<DriveFile> {
  const token = await getAccessToken(userId, nowMs)
  if (!token) {
    const err = new Error('not_connected')
    err.name = 'GoogleNotConnected'
    throw err
  }
  const mapped = await mediaFor(artifact)
  if (!mapped) {
    const err = new Error('not_exportable')
    err.name = 'NotExportable'
    throw err
  }

  const metadata = { name: artifact.title || 'Untitled', mimeType: mapped.targetMime }
  const { body, boundary } = buildMultipart(metadata, mapped.media)

  const res = await fetch(UPLOAD_ENDPOINT, {
    method: 'POST',
    headers: {
      Authorization: `Bearer ${token}`,
      'Content-Type': `multipart/related; boundary=${boundary}`,
    },
    body: new Uint8Array(body),
  })
  if (!res.ok) {
    throw new Error(`drive export failed: ${res.status} ${await res.text()}`)
  }
  const file = (await res.json()) as { id: string; webViewLink?: string; name?: string; mimeType?: string }
  return {
    id: file.id,
    url: file.webViewLink ?? `https://drive.google.com/open?id=${file.id}`,
    name: file.name ?? metadata.name,
    mimeType: file.mimeType ?? mapped.targetMime,
  }
}
