import { createQuery } from '@tanstack/svelte-query'
import { delJson, getJsonOr404, getList, postJson, putJson } from '@/lib/fetch-json'
import type { EditPolicy, KbEditor, Visibility } from '@/lib/kb'

/** A reactive argument: pass a plain value, or a getter for values that change
 *  over a component's life (route params, selections). */
type MaybeGetter<T> = T | (() => T)
const resolve = <T>(v: MaybeGetter<T>): T => (typeof v === 'function' ? (v as () => T)() : v)

export type ArtifactKind = 'doc' | 'sheet' | 'microsite' | 'file'

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

export const createArtifact = (input: { kind?: ArtifactKind; title?: string }) =>
  postJson<{ artifact: Artifact }>('/api/artifacts', input).then((r) => r.artifact)

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

export const deleteFolder = (id: string) => delJson<{ ok: true }>(`/api/artifact-folders/${id}`)

/** Upload a file (reuses the shared uploads store) → returns its id + metadata.
 *  Rejects with the server's sentence on failure, like every other mutation. */
export const uploadFile = async (file: File): Promise<{ id: string; filename: string; mime: string; size: number }> => {
  const form = new FormData()
  form.append('file', file)
  return postJson<{ id: string; filename: string; mime: string; size: number }>('/api/uploads', form)
}

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
