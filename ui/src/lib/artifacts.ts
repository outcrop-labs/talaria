import { resolve, type MaybeGetter } from '@/lib/reactive-arg'
import { createQuery } from '@tanstack/svelte-query'
import { delJson, delJsonGone, getJsonOr404, getList, postJson, putJson } from '@/lib/fetch-json'
import type { EditPolicy, KbEditor, Visibility } from '@/lib/kb'

/** A reactive argument: pass a plain value, or a getter for values that change
 *  over a component's life (route params, selections). */

export type ArtifactKind = 'doc' | 'sheet' | 'microsite' | 'file' | 'run-transcript'

export interface Artifact {
  id: string
  kind: ArtifactKind
  title: string
  icon: string | null
  body: string
  contentType: string | null
  storageRef: string | null
  visibility: Visibility
  editPolicy: EditPolicy
  publicSlug: string | null
  official: boolean
  /** RAG routing: 'auto' | 'none' | a custom brain id. */
  ragRouting: string
  kbDocId: string | null
  folderId: string | null
  ownerUserId: string | null
  googleFileId: string | null
  googleFileUrl: string | null
  createdBy: string | null
  updatedBy: string | null
  createdAt: string
  updatedAt: string
}

export interface ArtifactFolder {
  id: string
  name: string
  icon: string | null
  parentId: string | null
  /** Folders carry the same access model as artifacts and KB docs. */
  visibility: Visibility
  editPolicy: EditPolicy
  ownerUserId: string | null
  createdBy: string | null
  createdAt: string
}

export const useArtifacts = () =>
  createQuery(() => ({
    queryKey: ['artifacts'],
    queryFn: (): Promise<Artifact[]> => getList<Artifact>('/api/artifacts', 'artifacts'),
  }))

export const useArtifact = (id: MaybeGetter<string | null>) =>
  createQuery(() => {
    const i = resolve(id)
    return {
      queryKey: ['artifact', i],
      enabled: !!i,
      // 404 = deleted or not shared with you: a real "not found" the surface
      // tells the truth about. 403/500 must not masquerade as that.
      queryFn: async (): Promise<Artifact | null> =>
        (await getJsonOr404<{ artifact: Artifact }>(`/api/artifacts/${i}`))?.artifact ?? null,
    }
  })

export const createArtifact = (input: {
  kind?: ArtifactKind
  title?: string
  /** The body to save WITH the row, rather than an empty document and a
   *  follow-up PUT. The route has always taken it (`parse_body`: body, up to
   *  2,000,000 UTF-16 units) and saves it in the same request, so a caller
   *  that already holds the whole document — a transcript, a generated report
   *  — never shows an empty artifact for a beat. */
  body?: string
  /** A root folder BY NAME, find-or-create.
   *
   *  HONOURED FOR AN AGENT CALLER ONLY, which is the route's own split: an
   *  agent files its output deliberately (or lands in its cabinet), while a
   *  person's create lands where the browser made it. Passed through because
   *  the wire takes it, not because this client has a user-facing use for it
   *  yet. */
  folder?: string
}) => postJson<{ artifact: Artifact }>('/api/artifacts', input).then((r) => r.artifact)

/** WHICH OFFICE FILE A KIND BECOMES, or null where there is nothing to render:
 *  a `file` artifact's own bytes already download through /api/uploads, so
 *  offering to build a document out of it would be offering the 400 the route
 *  answers.
 *
 *  The format is the KIND's answer and never the caller's — a sheet is a grid
 *  and a document is prose, and `/api/artifacts/{id}/export` refuses the
 *  mismatch — so every surface that offers this reads it from here instead of
 *  spelling out its own ternary. */
export const officeExport = (
  kind: ArtifactKind | null | undefined,
): { format: 'docx' | 'xlsx'; label: string } | null =>
  !kind || kind === 'file'
    ? null
    : kind === 'sheet'
      ? { format: 'xlsx', label: 'Excel (.xlsx)' }
      : { format: 'docx', label: 'Word (.docx)' }

/** The download URL for one of those exports. */
export const officeExportHref = (id: string, format: 'docx' | 'xlsx'): string =>
  `/api/artifacts/${id}/export?format=${format}`

export const saveArtifact = (
  id: string,
  patch: Partial<Pick<Artifact, 'title' | 'body' | 'icon' | 'storageRef' | 'contentType' | 'folderId' | 'visibility' | 'editPolicy' | 'official' | 'ragRouting'>> & { editors?: KbEditor[] },
) => putJson<{ artifact: Artifact }>(`/api/artifacts/${id}`, patch).then((r) => r.artifact)

// ── Folders ──────────────────────────────────────────────────────────────────
export const useFolders = () =>
  createQuery(() => ({
    queryKey: ['artifact-folders'],
    queryFn: (): Promise<ArtifactFolder[]> => getList<ArtifactFolder>('/api/artifact-folders', 'folders'),
  }))

export const createFolder = (name: string, parentId?: string | null) =>
  postJson<{ folder: ArtifactFolder }>('/api/artifact-folders', { name, parentId }).then((r) => r.folder)

export const updateFolder = (
  id: string,
  patch: Partial<Pick<ArtifactFolder, 'name' | 'icon' | 'parentId' | 'visibility' | 'editPolicy'>> & { editors?: KbEditor[] },
) => putJson<{ folder: ArtifactFolder }>(`/api/artifact-folders/${id}`, patch).then((r) => r.folder)

export const duplicateFolder = (id: string) =>
  postJson<{ folder: ArtifactFolder }>(`/api/artifact-folders/${id}/duplicate`).then((r) => r.folder)

/** A folder another tab already deleted is a 404, and gone is what this
 *  asked for. */
export const deleteFolder = (id: string) => delJsonGone(`/api/artifact-folders/${id}`)

/** Upload a file (reuses the shared uploads store) → returns its id + metadata.
 *  Rejects with the server's sentence on failure, like every other mutation. */
export const uploadFile = async (file: File): Promise<{ id: string; filename: string; mime: string; size: number }> => {
  const form = new FormData()
  form.append('file', file)
  return postJson<{ id: string; filename: string; mime: string; size: number }>('/api/uploads', form)
}

export const duplicateArtifact = (id: string) =>
  postJson<{ artifact: Artifact }>(`/api/artifacts/${id}/duplicate`).then((r) => r.artifact)

export const deleteArtifact = (id: string) => delJson<{ ok: true }>(`/api/artifacts/${id}`)

// ── Attachments ──────────────────────────────────────────────────────────────
export const useTargetArtifacts = (targetType: string, targetId: MaybeGetter<string | null>) =>
  createQuery(() => {
    const i = resolve(targetId)
    return {
      queryKey: ['artifacts-for', targetType, i],
      enabled: !!i,
      queryFn: (): Promise<Artifact[]> =>
        getList<Artifact>(`/api/artifacts/for?targetType=${targetType}&targetId=${i}`, 'artifacts'),
    }
  })

export const attachArtifact = (id: string, targetType: string, targetId: string) =>
  postJson<{ ok: true }>(`/api/artifacts/${id}/links`, { targetType, targetId })

export const detachArtifact = (id: string, targetType: string, targetId: string) =>
  delJson<{ ok: true }>(`/api/artifacts/${id}/links`, { targetType, targetId })
