// Shared bits of the Files surface (Artifacts.svelte + the browser rows +
// the editor).
//
// The UI says "Files"; the code side stays "artifact" end to end — route,
// queries, API, and types keep the original noun. Only what a person READS
// changed, because "artifact" is a word this app taught its users and a word
// nobody arriving from Drive or Dropbox has ever needed.
import { FileText, Globe2, Paperclip, Table, type LucideIcon as IconType } from '@lucide/svelte'
import type { Artifact, ArtifactFolder, ArtifactKind } from '@/lib/artifacts'
import type { DirectoryUser } from '@/lib/users'

export const KIND_ICON: Record<ArtifactKind, IconType> = { doc: FileText, sheet: Table, microsite: Globe2, file: Paperclip }

/** Plain words for the four kinds. "Microsite" is jargon to everyone who hasn't
 *  read the schema; the wire values never change, only the reading does. */
export const KIND_LABEL: Record<ArtifactKind, string> = {
  doc: 'Document',
  sheet: 'Spreadsheet',
  microsite: 'Web page',
  file: 'File',
}

/** Kinds "+ New" can create. Uploading is a file picker, not a kind, so it sits
 *  beside these in the menu rather than inside them. */
export const NEW_KINDS: { kind: ArtifactKind; label: string; icon: IconType }[] = [
  { kind: 'doc', label: 'Document', icon: FileText },
  { kind: 'sheet', label: 'Spreadsheet', icon: Table },
  { kind: 'microsite', label: 'Web page', icon: Globe2 },
]

/** The folder agents file their output under (server: `AGENTS_ROOT`). The
 *  browser marks it as the fleet's cabinet rather than one of your folders. */
export const AGENTS_ROOT = 'Agents'

/** WHOSE a file is, which is a different question from who can SEE it.
 *
 *  A personal file has an owner: a person's own work, or a personal
 *  assistant's output, which belongs to the human it works for. A workspace
 *  file is ownerless on purpose — an org agent's output belongs to the
 *  organization, not to whoever happened to trigger it, and it outlives any
 *  one person's account. `visibility` then decides who can READ either one;
 *  the two axes are independent and must not be collapsed. */
export type Scope = 'personal' | 'workspace'
export const scopeOf = (a: { ownerUserId: string | null }): Scope => (a.ownerUserId ? 'personal' : 'workspace')

/** Rail places, a clean partition of every file by OWNERSHIP: mine, someone
 *  else's, or nobody's. Folders exist only inside My Files — the rest are flat
 *  views across the whole store, the way Drive treats them, so a file you were
 *  looking for can't hide one level down. */
export type Place = 'my' | 'shared' | 'workspace' | 'official' | 'recent' | 'secrets'
export const PLACES: { id: Place; label: string; glyph: string; empty: string; hint?: string }[] = [
  { id: 'my', label: 'My Files', glyph: '◆', empty: 'Nothing here yet.', hint: 'Drop files anywhere to upload, or use New.' },
  { id: 'shared', label: 'Shared with me', glyph: '◇', empty: 'Nothing shared with you yet.' },
  { id: 'workspace', label: 'Workspace', glyph: '⊞', empty: 'No workspace files yet.', hint: 'Files the organization owns, mostly written by agents.' },
  { id: 'official', label: 'Official', glyph: '★', empty: 'No official files yet.', hint: 'Official files are mirrored into the knowledgebase.' },
  { id: 'recent', label: 'Recent', glyph: '◷', empty: 'Nothing edited recently.' },
  // NOT A FILE, and it gets its own place rather than rows in the table for
  // exactly that reason. Every other place lists ARTIFACTS, whose bodies are
  // indexed for retrieval, exported to Google, downloadable, and served
  // unauthenticated at /api/artifacts/public/$slug. A credential must never
  // reach any of that, so it is never an artifact row — the cabinet is shared,
  // the store is not.
  { id: 'secrets', label: 'Secrets', glyph: '⚿', empty: 'No secrets yet.', hint: 'Credentials you are working with: sealed, shared deliberately, every reveal recorded.' },
]

/** How each general-access tier reads to a person, not to the schema. Shared
 *  by the Properties dialog and the row's access marker. */
export const VISIBILITY_LABEL: Record<string, string> = {
  private: 'Restricted',
  org: 'Everyone in the workspace',
  public: 'Anyone with the link',
}

/** A drag carries the whole SELECTION, not the one row under the cursor —
 *  dragging a selected row moves everything selected with it. */
export type Drag = { folders: string[]; artifacts: string[] } | null

/** The payload also rides on the dataTransfer, not just component state, so
 *  targets OUTSIDE the browser (the breadcrumb, which is the only way to move
 *  something up a level now that there's no tree) can accept a drop without
 *  the two components having to share a variable. `dragover` may only inspect
 *  `types`, never the data — which is exactly enough to know a drop is ours. */
export const DRAG_MIME = 'application/x-talaria-files'

export type SortKey = 'name' | 'kind' | 'owner' | 'modified'
export type SortDir = 'asc' | 'desc'

/** One line in the browser. Folders and artifacts are flattened into a single
 *  shape so sorting, selection, and the row markup don't each have to unpick
 *  the union three separate times. */
export interface Row {
  type: 'folder' | 'artifact'
  id: string
  name: string
  /** Emoji override, when someone set one. */
  icon: string | null
  kind: ArtifactKind | null
  kindLabel: string
  owner: string
  /** Folders are org-wide containers, so only artifacts carry a scope. */
  scope: Scope | null
  /** ISO timestamp — sorted as a string deliberately (ISO sorts correctly). */
  modified: string
  artifact: Artifact | null
}

type Me = { id: string; email: string | null; name: string | null } | null

/** Owner-or-creator, in the Owner column's words. Yourself reads as "me", the
 *  way every file browser has done it since Drive made the habit. */
export function ownerOf(rec: { ownerUserId?: string | null; createdBy: string | null }, users: DirectoryUser[], me: Me): string {
  const uid = rec.ownerUserId ?? null
  if (uid) {
    if (me && uid === me.id) return 'me'
    const u = users.find((x) => x.id === uid)
    return u ? (u.name ?? u.email ?? 'someone') : 'someone'
  }
  const by = rec.createdBy
  if (!by) return 'unknown'
  // Agent-authored artifacts carry a model name here, which is the right thing
  // to show: the fleet member that wrote it.
  if (me && (by === me.email || by === me.name)) return 'me'
  return by
}

/** The same ownership rule the editor uses for its owner-only controls. */
export function isMine(a: Artifact, me: Me): boolean {
  if (!me) return false
  return a.ownerUserId ? a.ownerUserId === me.id : a.createdBy === (me.email ?? me.name)
}

/** Which place a file belongs to. Deliberately keyed on `ownerUserId` rather
 *  than `isMine`, so the three places partition the store exactly once: an
 *  ownerless file is the WORKSPACE's even when you were the one who triggered
 *  it, and it can never show up in two places at the same time. */
export function placeOf(a: Artifact, me: Me): Extract<Place, 'my' | 'shared' | 'workspace'> {
  if (!a.ownerUserId) return 'workspace'
  return isMine(a, me) ? 'my' : 'shared'
}

export function toRow(a: Artifact, users: DirectoryUser[], me: Me): Row {
  return {
    type: 'artifact',
    id: a.id,
    name: a.title,
    icon: a.icon,
    kind: a.kind,
    kindLabel: a.contentType?.startsWith('image/') ? 'Image' : KIND_LABEL[a.kind],
    owner: ownerOf(a, users, me),
    scope: scopeOf(a),
    modified: a.updatedAt,
    artifact: a,
  }
}

export function folderRow(f: ArtifactFolder, users: DirectoryUser[], me: Me, modified: string): Row {
  return {
    type: 'folder',
    id: f.id,
    name: f.name,
    icon: f.icon,
    kind: null,
    kindLabel: 'Folder',
    owner: ownerOf(f, users, me),
    scope: null,
    // Folders have no updated_at, so they wear their newest child's timestamp —
    // an empty folder falls back to when it was made.
    modified,
    artifact: null,
  }
}

export function sortRows(rows: Row[], key: SortKey, dir: SortDir): Row[] {
  const sign = dir === 'asc' ? 1 : -1
  return [...rows].sort((a, b) => {
    // Folders are places, not files: they head the list under every sort, the
    // same way they do in Drive, Dropbox, and Finder.
    if (a.type !== b.type) return a.type === 'folder' ? -1 : 1
    const cmp =
      key === 'modified'
        ? a.modified.localeCompare(b.modified)
        : key === 'kind'
          ? a.kindLabel.localeCompare(b.kindLabel)
          : key === 'owner'
            ? a.owner.localeCompare(b.owner)
            : a.name.localeCompare(b.name, undefined, { numeric: true, sensitivity: 'base' })
    // Name breaks every tie, so equal timestamps don't shuffle between renders.
    return cmp * sign || a.name.localeCompare(b.name, undefined, { numeric: true, sensitivity: 'base' })
  })
}

/** Root → … → this folder, for the breadcrumb. Bounded: a cycle in the folder
 *  table must not spin the render loop. */
export function ancestry(folderId: string | null, folders: ArtifactFolder[]): ArtifactFolder[] {
  const byId = new Map(folders.map((f) => [f.id, f]))
  const chain: ArtifactFolder[] = []
  const seen = new Set<string>()
  let cur = folderId
  while (cur && !seen.has(cur)) {
    seen.add(cur)
    const f = byId.get(cur)
    if (!f) break
    chain.unshift(f)
    cur = f.parentId
  }
  return chain
}

// A spreadsheet-style grid. The sheet body is JSON `string[][]` — row 0 is the
// header row. Edit mode is an editable grid with add/delete row+column; read
// mode renders a table. Autosaves (debounced).
export function parseGrid(body: string): string[][] {
  try {
    const g = JSON.parse(body)
    if (Array.isArray(g) && g.length && g.every((r) => Array.isArray(r))) return g.map((r: unknown[]) => r.map((c) => String(c ?? '')))
  } catch {
    /* fall through to a starter grid */
  }
  return [['Column A', 'Column B', 'Column C'], ['', '', ''], ['', '', '']]
}
