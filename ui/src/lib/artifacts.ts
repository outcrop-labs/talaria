import { useQuery } from '@tanstack/react-query'
import type { EditPolicy, KbEditor, Visibility } from '@/lib/kb'

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
  createdBy: string | null
  createdAt: string
}

export const useArtifacts = () =>
  useQuery({
    queryKey: ['artifacts'],
    queryFn: async (): Promise<Artifact[]> => {
      const r = await fetch('/api/artifacts')
      if (!r.ok) return []
      return ((await r.json()) as { artifacts: Artifact[] }).artifacts
    },
  })

export const useArtifact = (id: string | null) =>
  useQuery({
    queryKey: ['artifact', id],
    enabled: !!id,
    queryFn: async (): Promise<Artifact | null> => {
      const r = await fetch(`/api/artifacts/${id}`)
      if (!r.ok) return null
      return ((await r.json()) as { artifact: Artifact }).artifact
    },
  })

export const createArtifact = (input: { kind?: ArtifactKind; title?: string }) =>
  fetch('/api/artifacts', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(input) }).then((r) => r.json())

export const saveArtifact = (
  id: string,
  patch: Partial<Pick<Artifact, 'title' | 'body' | 'icon' | 'storageRef' | 'contentType' | 'folderId' | 'visibility' | 'editPolicy' | 'official'>> & { editors?: KbEditor[] },
) => fetch(`/api/artifacts/${id}`, { method: 'PUT', headers: { 'content-type': 'application/json' }, body: JSON.stringify(patch) }).then((r) => r.json())

// ── Folders ──────────────────────────────────────────────────────────────────
export const useFolders = () =>
  useQuery({
    queryKey: ['artifact-folders'],
    queryFn: async (): Promise<ArtifactFolder[]> => {
      const r = await fetch('/api/artifact-folders')
      if (!r.ok) return []
      return ((await r.json()) as { folders: ArtifactFolder[] }).folders
    },
  })

export const createFolder = (name: string, parentId?: string | null) =>
  fetch('/api/artifact-folders', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ name, parentId }) }).then((r) => r.json())

export const updateFolder = (id: string, patch: Partial<Pick<ArtifactFolder, 'name' | 'icon' | 'parentId'>>) =>
  fetch(`/api/artifact-folders/${id}`, { method: 'PUT', headers: { 'content-type': 'application/json' }, body: JSON.stringify(patch) }).then((r) => r.json())

export const deleteFolder = (id: string) => fetch(`/api/artifact-folders/${id}`, { method: 'DELETE' })

/** Upload a file (reuses the shared uploads store) → returns its id + metadata. */
export const uploadFile = async (file: File): Promise<{ id: string; filename: string; mime: string; size: number }> => {
  const form = new FormData()
  form.append('file', file)
  const r = await fetch('/api/uploads', { method: 'POST', body: form })
  if (!r.ok) throw new Error('upload failed')
  return r.json()
}

export const deleteArtifact = (id: string) => fetch(`/api/artifacts/${id}`, { method: 'DELETE' })

// ── Attachments ──────────────────────────────────────────────────────────────
export const useTargetArtifacts = (targetType: string, targetId: string | null) =>
  useQuery({
    queryKey: ['artifacts-for', targetType, targetId],
    enabled: !!targetId,
    queryFn: async (): Promise<Artifact[]> => {
      const r = await fetch(`/api/artifacts/for?targetType=${targetType}&targetId=${targetId}`)
      if (!r.ok) return []
      return ((await r.json()) as { artifacts: Artifact[] }).artifacts
    },
  })

export const attachArtifact = (id: string, targetType: string, targetId: string) =>
  fetch(`/api/artifacts/${id}/links`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ targetType, targetId }) })

export const detachArtifact = (id: string, targetType: string, targetId: string) =>
  fetch(`/api/artifacts/${id}/links`, { method: 'DELETE', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ targetType, targetId }) })
