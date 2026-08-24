// Google Drive service — currently: push a Talaria artifact into the user's
// Drive as a native Google Doc / Sheet (or an unconverted file). Acts strictly
// as the connected user via their stored token (per-user OAuth), so Drive's own
// sharing rules govern what lands where.

import { getUpload, saveUpload } from '../uploads'
import type { Artifact, ArtifactKind } from '../artifacts'
import { getAccessToken } from './connections'

// supportsAllDrives lets us create into a Shared Drive (team-owned files).
const UPLOAD_ENDPOINT =
  'https://www.googleapis.com/upload/drive/v3/files?uploadType=multipart&supportsAllDrives=true&fields=id,webViewLink,name,mimeType'
const FILES_ENDPOINT = 'https://www.googleapis.com/drive/v3/files'

const GOOGLE_DOC = 'application/vnd.google-apps.document'
const GOOGLE_SHEET = 'application/vnd.google-apps.spreadsheet'
// Google's native (non-downloadable) types must be exported to a real format.
const GOOGLE_NATIVE_PREFIX = 'application/vnd.google-apps'

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
  return exportArtifactWithToken(await requireToken(userId, nowMs), artifact)
}

/** Export an artifact using an already-resolved access token (per-user or org).
 *  `folderId` (a Shared Drive or folder) makes the file team-owned there. */
export async function exportArtifactWithToken(token: string, artifact: Artifact, opts: { folderId?: string | null } = {}): Promise<DriveFile> {
  const mapped = await mediaFor(artifact)
  if (!mapped) {
    const err = new Error('not_exportable')
    err.name = 'NotExportable'
    throw err
  }

  const metadata: { name: string; mimeType: string; parents?: string[] } = { name: artifact.title || 'Untitled', mimeType: mapped.targetMime }
  if (opts.folderId) metadata.parents = [opts.folderId]
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

/** A valid access token, or throw the caller-friendly GoogleNotConnected. */
async function requireToken(userId: string, nowMs: number): Promise<string> {
  const token = await getAccessToken(userId, nowMs)
  if (!token) {
    const err = new Error('not_connected')
    err.name = 'GoogleNotConnected'
    throw err
  }
  return token
}

// ── Browse + import ──────────────────────────────────────────────────────────

export interface DriveListEntry {
  id: string
  name: string
  mimeType: string
  modifiedTime: string | null
  iconLink: string | null
  webViewLink: string | null
  sizeBytes: number | null
}

/** List/search Drive files using an already-resolved token (per-user or org) —
 *  the agent-facing twin of `listDriveFiles`. */
export async function listDriveFilesWithToken(token: string, query?: string, pageSize = 25): Promise<DriveListEntry[]> {
  // Exclude trashed + folders; optionally filter by name substring.
  const clauses = ['trashed = false', `mimeType != 'application/vnd.google-apps.folder'`]
  if (query && query.trim()) clauses.push(`name contains '${query.trim().replace(/['\\]/g, ' ')}'`)
  const params = new URLSearchParams({
    q: clauses.join(' and '),
    pageSize: String(Math.min(Math.max(pageSize, 1), 100)),
    orderBy: 'modifiedTime desc',
    fields: 'files(id,name,mimeType,modifiedTime,iconLink,webViewLink,size)',
    spaces: 'drive',
    // Without these two, files living in a Shared Drive are invisible to the
    // listing — the org's agents would browse an empty Drive while the
    // provisioned shared drive (their actual workspace) sat unread.
    supportsAllDrives: 'true',
    includeItemsFromAllDrives: 'true',
  })
  const res = await fetch(`${FILES_ENDPOINT}?${params.toString()}`, { headers: { Authorization: `Bearer ${token}` } })
  if (!res.ok) throw new Error(`drive list failed: ${res.status} ${await res.text()}`)
  const data = (await res.json()) as {
    files?: Array<{ id: string; name: string; mimeType: string; modifiedTime?: string; iconLink?: string; webViewLink?: string; size?: string }>
  }
  return (data.files ?? []).map((f) => ({
    id: f.id,
    name: f.name,
    mimeType: f.mimeType,
    modifiedTime: f.modifiedTime ?? null,
    iconLink: f.iconLink ?? null,
    webViewLink: f.webViewLink ?? null,
    sizeBytes: f.size ? Number(f.size) : null,
  }))
}

/** List/search the user's Drive files (most-recent first). `query` matches names. */
export async function listDriveFiles(userId: string, nowMs: number, query?: string, pageSize = 25): Promise<DriveListEntry[]> {
  return listDriveFilesWithToken(await requireToken(userId, nowMs), query, pageSize)
}

/** Minimal CSV → grid parser (handles quotes, escaped quotes, CRLF). */
function csvToGrid(csv: string): string[][] {
  const rows: string[][] = []
  let row: string[] = []
  let cell = ''
  let quoted = false
  for (let i = 0; i < csv.length; i++) {
    const c = csv[i]
    if (quoted) {
      if (c === '"') {
        if (csv[i + 1] === '"') { cell += '"'; i++ } else quoted = false
      } else cell += c
    } else if (c === '"') {
      quoted = true
    } else if (c === ',') {
      row.push(cell); cell = ''
    } else if (c === '\n' || c === '\r') {
      if (c === '\r' && csv[i + 1] === '\n') i++
      row.push(cell); cell = ''
      rows.push(row); row = []
    } else cell += c
  }
  if (cell !== '' || row.length) { row.push(cell); rows.push(row) }
  return rows
}

export interface ImportedContent {
  kind: ArtifactKind
  title: string
  body: string
  storageRef?: string
  contentType?: string
  sourceUrl: string | null
}

/** Pull a Drive file's content into a Talaria-artifact shape. Google Docs →
 *  markdown doc, Sheets → grid sheet, everything else → downloaded file. */
export async function importDriveFile(userId: string, fileId: string, nowMs: number): Promise<ImportedContent> {
  const token = await requireToken(userId, nowMs)
  const auth = { Authorization: `Bearer ${token}` }

  // Metadata first: name + type decide how we pull the bytes.
  const metaRes = await fetch(`${FILES_ENDPOINT}/${encodeURIComponent(fileId)}?fields=id,name,mimeType,webViewLink`, { headers: auth })
  if (!metaRes.ok) throw new Error(`drive get failed: ${metaRes.status} ${await metaRes.text()}`)
  const meta = (await metaRes.json()) as { name: string; mimeType: string; webViewLink?: string }
  const sourceUrl = meta.webViewLink ?? null

  if (meta.mimeType === GOOGLE_DOC) {
    const md = await exportGoogle(fileId, 'text/markdown', auth)
    return { kind: 'doc', title: meta.name, body: md, sourceUrl }
  }
  if (meta.mimeType === GOOGLE_SHEET) {
    const csv = await exportGoogle(fileId, 'text/csv', auth)
    return { kind: 'sheet', title: meta.name, body: JSON.stringify(csvToGrid(csv)), sourceUrl }
  }
  if (meta.mimeType.startsWith(GOOGLE_NATIVE_PREFIX)) {
    // Other native types (Slides, Drawings, ) → export a PDF and store as a file.
    const bytes = await exportGoogleBytes(fileId, 'application/pdf', auth)
    const up = await saveUpload({ filename: `${meta.name}.pdf`, mime: 'application/pdf', bytes, userId })
    return { kind: 'file', title: meta.name, body: '', storageRef: up.id, contentType: 'application/pdf', sourceUrl }
  }

  // A regular binary file → download and store.
  const dlRes = await fetch(`${FILES_ENDPOINT}/${encodeURIComponent(fileId)}?alt=media`, { headers: auth })
  if (!dlRes.ok) throw new Error(`drive download failed: ${dlRes.status} ${await dlRes.text()}`)
  const bytes = new Uint8Array(await dlRes.arrayBuffer())
  const up = await saveUpload({ filename: meta.name, mime: meta.mimeType, bytes, userId })
  return { kind: 'file', title: meta.name, body: '', storageRef: up.id, contentType: meta.mimeType, sourceUrl }
}

/** Export a Google-native file to a text format (markdown, csv, ). */
async function exportGoogle(fileId: string, mimeType: string, auth: Record<string, string>): Promise<string> {
  const res = await fetch(`${FILES_ENDPOINT}/${encodeURIComponent(fileId)}/export?mimeType=${encodeURIComponent(mimeType)}`, { headers: auth })
  if (!res.ok) throw new Error(`drive export(${mimeType}) failed: ${res.status} ${await res.text()}`)
  return res.text()
}

async function exportGoogleBytes(fileId: string, mimeType: string, auth: Record<string, string>): Promise<Uint8Array> {
  const res = await fetch(`${FILES_ENDPOINT}/${encodeURIComponent(fileId)}/export?mimeType=${encodeURIComponent(mimeType)}`, { headers: auth })
  if (!res.ok) throw new Error(`drive export(${mimeType}) failed: ${res.status} ${await res.text()}`)
  return new Uint8Array(await res.arrayBuffer())
}
