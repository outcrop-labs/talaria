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
  ownerUserId: string | null
  createdBy: string | null
  updatedBy: string | null
  createdAt: string
  updatedAt: string
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
  patch: Partial<Pick<Artifact, 'title' | 'body' | 'icon' | 'visibility' | 'editPolicy' | 'official'>> & { editors?: KbEditor[] },
) => fetch(`/api/artifacts/${id}`, { method: 'PUT', headers: { 'content-type': 'application/json' }, body: JSON.stringify(patch) }).then((r) => r.json())

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
