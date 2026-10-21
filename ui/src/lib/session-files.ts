// THE SESSION'S FILES — what a work session's pane shows, as the server lists it.
//
// A work session is about the files it touches: every Talaria document, sheet
// or page and every Google file the agent created or opened in it, plus what a
// teammate opened there by hand. `GET /api/conversations/{id}/files` derives
// that list from the session's own messages (their link chips) and its pinned
// files, oldest first and deduplicated by `key`. Nothing is created just
// because a session was opened, so an empty list is a real answer.
//
// This module is the read and the rules around it, kept pure where it can be so
// a test can reach them: which file a session opens on, which files a refetch
// says are NEW, and the per-viewer memory of the tabs somebody closed.

import { createQuery } from '@tanstack/svelte-query'
import type { PinnedFile } from '@/lib/conversations.svelte'
import { getJsonOr404 } from '@/lib/fetch-json'
import { openUrl } from '@/lib/google-embed'
import { readStored, writeStored } from '@/lib/persist'
import { resolve, type MaybeGetter } from '@/lib/reactive-arg'

export type GoogleFileType = 'document' | 'spreadsheet' | 'presentation' | 'drive'

/** One file of a session, exactly as the files route answers it. */
export interface SessionFile {
  /** Stable dedupe key: `artifact:<uuid>` or `google:<fileId>`. It is also the
   *  pane's tab id, so the same file reached two ways is one tab. */
  key: string
  kind: 'artifact' | 'google'
  /** The artifact's uuid, or the Google file id. */
  id: string
  title: string | null
  /** Artifact rows only: `artifacts.kind` (doc | sheet | microsite | file | …). */
  artifactKind?: string
  /** Google rows only. */
  googleType?: GoogleFileType
  /** Google rows only: the canonical web URL. */
  url?: string
  source: 'touched' | 'pinned' | 'living'
  /** When a message last named this file (ISO); null for pins and the legacy
   *  living document, which nobody "touched" in a turn. */
  lastTouchedAt: string | null
}

export interface SessionFilesAnswer {
  /** The conversation's own kind — `work`, `plan`, `research`, `chat`, `ticket`.
   *  It is what decides which pane a session gets. */
  kind: string
  files: SessionFile[]
}

const GOOGLE_TYPES: readonly GoogleFileType[] = ['document', 'spreadsheet', 'presentation', 'drive']

/** Keep only rows this client can render. The route is ours, but a row this
 *  build does not understand must drop out rather than mount a broken tab. */
export function parseSessionFiles(raw: unknown): SessionFilesAnswer {
  const obj = raw && typeof raw === 'object' ? (raw as Record<string, unknown>) : {}
  const kind = typeof obj.kind === 'string' ? obj.kind : 'work'
  const rows = Array.isArray(obj.files) ? obj.files : []
  const files: SessionFile[] = []
  for (const row of rows) {
    if (!row || typeof row !== 'object') continue
    const r = row as Record<string, unknown>
    if (typeof r.key !== 'string' || typeof r.id !== 'string' || r.id === '') continue
    if (r.kind !== 'artifact' && r.kind !== 'google') continue
    files.push({
      key: r.key,
      kind: r.kind,
      id: r.id,
      title: typeof r.title === 'string' ? r.title : null,
      ...(typeof r.artifactKind === 'string' ? { artifactKind: r.artifactKind } : {}),
      ...(GOOGLE_TYPES.includes(r.googleType as GoogleFileType) ? { googleType: r.googleType as GoogleFileType } : {}),
      ...(typeof r.url === 'string' ? { url: r.url } : {}),
      source: r.source === 'pinned' || r.source === 'living' ? r.source : 'touched',
      lastTouchedAt: typeof r.lastTouchedAt === 'string' ? r.lastTouchedAt : null,
    })
  }
  return { kind, files }
}

/** One read of the files route, stamped with the session it answers. The
 *  stamp is what lets a reader refuse a result that belongs to the session it
 *  was showing a moment ago — a strip seeded with the previous session's files
 *  would be wrong in a way nobody could see the cause of. `answer` is null on
 *  the route's 404: a session this viewer cannot read has nothing to list. */
export interface SessionFilesRead {
  sessionId: string
  answer: SessionFilesAnswer | null
}

export const useSessionFiles = (sessionId: MaybeGetter<string | null>) =>
  createQuery(() => {
    const id = resolve(sessionId)
    return {
      queryKey: ['session-files', id],
      enabled: !!id,
      queryFn: async (): Promise<SessionFilesRead> => {
        const raw = await getJsonOr404<unknown>(`/api/conversations/${id}/files`)
        return { sessionId: id ?? '', answer: raw === null ? null : parseSessionFiles(raw) }
      },
    }
  })

// ── WHAT A FILE IS CALLED ───────────────────────────────────────────────────

export type FileGlyph = 'doc' | 'sheet' | 'page' | 'file' | 'google-doc' | 'google-sheet' | 'google-slides' | 'google-drive'

export function fileGlyph(f: Pick<SessionFile, 'kind' | 'artifactKind' | 'googleType'>): FileGlyph {
  if (f.kind === 'google') {
    switch (f.googleType) {
      case 'document':
        return 'google-doc'
      case 'spreadsheet':
        return 'google-sheet'
      case 'presentation':
        return 'google-slides'
      default:
        return 'google-drive'
    }
  }
  switch (f.artifactKind) {
    case 'doc':
      return 'doc'
    case 'sheet':
      return 'sheet'
    // `create_page` writes a `microsite` row; people call it a page.
    case 'microsite':
      return 'page'
    default:
      return 'file'
  }
}

const TYPE_LABEL: Record<FileGlyph, string> = {
  doc: 'Document',
  sheet: 'Sheet',
  page: 'Page',
  file: 'File',
  'google-doc': 'Google Doc',
  'google-sheet': 'Google Sheet',
  'google-slides': 'Google Slides',
  'google-drive': 'Drive file',
}

const UNTITLED: Record<FileGlyph, string> = {
  doc: 'Untitled document',
  sheet: 'Untitled sheet',
  page: 'Untitled page',
  file: 'Untitled file',
  // A Google file the session only knows by URL has no title of its own on
  // the wire; its type is the honest name until somebody opens it.
  'google-doc': 'Google Doc',
  'google-sheet': 'Google Sheet',
  'google-slides': 'Google Slides',
  'google-drive': 'Drive file',
}

/** The type, as the file header's mono label says it. */
export const fileTypeLabel = (f: Pick<SessionFile, 'kind' | 'artifactKind' | 'googleType'>): string =>
  TYPE_LABEL[fileGlyph(f)]

/** The title, or a name that says what kind of untitled thing it is. */
export const fileTitle = (f: Pick<SessionFile, 'kind' | 'artifactKind' | 'googleType' | 'title'>): string =>
  f.title?.trim() || UNTITLED[fileGlyph(f)]

const GOOGLE_MIME: Record<GoogleFileType, string | null> = {
  document: 'application/vnd.google-apps.document',
  spreadsheet: 'application/vnd.google-apps.spreadsheet',
  presentation: 'application/vnd.google-apps.presentation',
  drive: null,
}

/** The mime the embed helpers key on (`lib/google-embed`). */
export const googleMime = (f: Pick<SessionFile, 'googleType'>): string | null =>
  f.googleType ? GOOGLE_MIME[f.googleType] : null

/** Where Open ↗ goes for a Google file: the server's canonical URL when it is
 *  a Google https URL, and the one the embed helpers build otherwise. */
export function googleOpenHref(f: Pick<SessionFile, 'id' | 'url' | 'googleType'>): string {
  if (f.url) {
    try {
      const u = new URL(f.url)
      const host = u.hostname.toLowerCase()
      if (u.protocol === 'https:' && (host === 'docs.google.com' || host === 'drive.google.com')) return u.href
    } catch {
      /* fall through to the built URL */
    }
  }
  return openUrl(f.id, googleMime(f))
}

// ── WHICH FILE TAKES THE PANE ───────────────────────────────────────────────

const touchedMs = (at: string | null | undefined): number => (at ? Date.parse(at) : Number.NaN)

/** Has a touch moved FORWARD? A file with no touch time has not been touched;
 *  one that gains a time, or a later one, has. */
const movedForward = (was: string | null | undefined, now: string | null | undefined): boolean => {
  const n = touchedMs(now)
  if (Number.isNaN(n)) return false
  const w = touchedMs(was)
  return Number.isNaN(w) || n > w
}

/** The file a message named most recently; ties go to the later row. */
export function newestTouched<T extends Pick<SessionFile, 'lastTouchedAt'>>(files: readonly T[]): T | null {
  let best: T | null = null
  let bestMs = Number.NEGATIVE_INFINITY
  for (const f of files) {
    const ms = touchedMs(f.lastTouchedAt)
    if (Number.isNaN(ms)) continue
    if (ms >= bestMs) {
      best = f
      bestMs = ms
    }
  }
  return best
}

/** WHERE A SESSION OPENS: the tab this viewer was reading last time, if the
 *  session still lists it; else the file the agent touched most recently;
 *  else the first file. Null when there are no files. */
export function initialFocus(files: readonly SessionFile[], remembered?: string | null): string | null {
  if (remembered && files.some((f) => f.key === remembered)) return remembered
  return (newestTouched(files) ?? files[0])?.key ?? null
}

/** WHAT A REFETCH BROUGHT: files listed for the first time, and files a newer
 *  message touched again. The first read of a session has nothing to compare
 *  with, so it brings nothing — that read opens on `initialFocus` instead. */
export function changedFiles(prev: readonly SessionFile[] | null, next: readonly SessionFile[]): SessionFile[] {
  if (!prev) return []
  const before = new Map(prev.map((f) => [f.key, f]))
  return next.filter((f) => {
    const was = before.get(f.key)
    return !was || movedForward(was.lastTouchedAt, f.lastTouchedAt)
  })
}

/** Of the files a refetch brought, the one the pane should show: the newest
 *  touch wins, and a change with no touch time (a file somebody just opened by
 *  hand) wins only when nothing was touched. */
export const focusTarget = (changed: readonly SessionFile[]): SessionFile | null =>
  newestTouched(changed) ?? changed.at(-1) ?? null

// ── CLOSED TABS, PER VIEWER ─────────────────────────────────────────────────
//
// Closing a file's tab hides it for this viewer in this session — the file is
// still the session's, and every other member still sees it. The memory is a
// per-viewer convenience in localStorage, one bounded key for every session,
// like `talaria:workspace-tab:v1`. Each entry carries the file's touch time AT
// THE MOMENT IT WAS HIDDEN, which is what lets a file come back by itself when
// the agent touches it again — across a reload, not only within one page life.

const HIDDEN_KEY = 'talaria:workspace-hidden:v1'
const HIDDEN_SESSIONS = 40
const HIDDEN_PER_SESSION = 100

/** key → the file's `lastTouchedAt` when it was hidden. */
export type HiddenFiles = Record<string, string | null>
type HiddenStore = Record<string, HiddenFiles>

function parseHidden(raw: unknown): HiddenStore | null {
  if (raw === null || typeof raw !== 'object' || Array.isArray(raw)) return null
  const out: HiddenStore = {}
  for (const [sessionId, entry] of Object.entries(raw as Record<string, unknown>)) {
    if (entry === null || typeof entry !== 'object' || Array.isArray(entry)) continue
    const keys: HiddenFiles = {}
    for (const [key, at] of Object.entries(entry as Record<string, unknown>)) {
      if (typeof at === 'string' || at === null) keys[key] = at
    }
    out[sessionId] = keys
  }
  return out
}

const readAll = (): HiddenStore => readStored(HIDDEN_KEY, parseHidden, {})

function writeSession(sessionId: string, files: HiddenFiles): void {
  const all = readAll()
  delete all[sessionId]
  // Insertion order is the recency: the session just written goes last and the
  // stalest fall off the front, so the key cannot grow without bound.
  const entries = Object.entries(all)
  const kept = Object.entries(files).slice(-HIDDEN_PER_SESSION)
  if (kept.length > 0) entries.push([sessionId, Object.fromEntries(kept)])
  writeStored(HIDDEN_KEY, Object.fromEntries(entries.slice(-HIDDEN_SESSIONS)))
}

/** The files this viewer closed in this session. */
export const hiddenFiles = (sessionId: string): HiddenFiles => readAll()[sessionId] ?? {}

export function hideFile(sessionId: string, file: Pick<SessionFile, 'key' | 'lastTouchedAt'>): void {
  const current = { ...hiddenFiles(sessionId) }
  delete current[file.key]
  current[file.key] = file.lastTouchedAt
  writeSession(sessionId, current)
}

export function unhideFile(sessionId: string, key: string): void {
  const current = hiddenFiles(sessionId)
  if (!(key in current)) return
  const next = { ...current }
  delete next[key]
  writeSession(sessionId, next)
}

/** Hidden files a newer message has touched since they were hidden. */
export const resurfaced = (hidden: HiddenFiles, files: readonly SessionFile[]): string[] =>
  files.filter((f) => f.key in hidden && movedForward(hidden[f.key], f.lastTouchedAt)).map((f) => f.key)

/** The files the strip shows — everything not hidden. */
export const visibleFiles = (files: readonly SessionFile[], hidden: HiddenFiles): SessionFile[] =>
  files.filter((f) => !(f.key in hidden))

// ── OPENING ONE BY HAND ─────────────────────────────────────────────────────

/** The session's pins with one more, at the END and never twice. Appending is
 *  the point: opening a second file must not unpin the first. */
export function appendPin(current: readonly PinnedFile[], file: PinnedFile): PinnedFile[] {
  const rest = current.filter((f) => !(f.kind === file.kind && f.id === file.id))
  return [...rest, file]
}
