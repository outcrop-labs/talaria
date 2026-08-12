// Client view of the credentials agents may USE.
//
// The shapes mirror server/workspace-secrets.ts exactly — INCLUDING the absence
// of any field that could hold a value, which is the point rather than an
// omission: there is nowhere for one to arrive, so no component can render one
// by accident and no cache can hold one.
//
// NOT `lib/secrets.ts`, which is the instance's own secret INVENTORY (provider
// keys, agent credentials, whether each still decrypts). Two different nouns
// share the word; an operator who conflates them will eventually revoke the
// wrong one.
import { createQuery, useQueryClient } from '@tanstack/svelte-query'
import { getJson } from '@/lib/fetch-json'

export type SecretKind = 'vault' | 'relay'

export interface WorkspaceSecret {
  id: string
  name: string
  title: string
  kind: SecretKind
  note: string | null
  createdBy: string | null
  createdAt: string
  expiresAt: string | null
  usesRemaining: number | null
  lastUsedAt: string | null
  entries: Array<{ key: string; label: string }>
  grants: string[]
}

export const WORKSPACE_SECRETS_KEY = ['workspace-secrets']

export const useWorkspaceSecrets = () =>
  createQuery(() => ({
    queryKey: WORKSPACE_SECRETS_KEY,
    queryFn: (): Promise<{ secrets: WorkspaceSecret[] }> => getJson<{ secrets: WorkspaceSecret[] }>('/api/admin/workspace-secrets'),
  }))

type Action =
  | { action: 'create'; name: string; title: string; entries: Array<{ key: string; label: string; value: string }>; kind?: SecretKind; note?: string | null; uses?: number | null; grantTo?: string[] }
  | { action: 'grant'; name: string; agentModel: string }
  | { action: 'revoke'; name: string; agentModel: string }
  | { action: 'delete'; name: string }

/** One writer for all four verbs. The create body is the ONLY place a value ever
 *  travels from this client, and it travels once — nothing here reads one back,
 *  because no response carries one. */
export function useWorkspaceSecretAction() {
  const qc = useQueryClient()
  return async (body: Action): Promise<{ error?: string }> => {
    const r = await fetch('/api/admin/workspace-secrets', {
      method: 'POST',
      credentials: 'same-origin',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(body),
    })
    const j = (await r.json().catch(() => ({}))) as { error?: string }
    await qc.invalidateQueries({ queryKey: WORKSPACE_SECRETS_KEY })
    if (!r.ok) return { error: j.error ?? `${body.action} failed (${r.status})` }
    return {}
  }
}

/** The handles an agent would write for this doc. Mirrors `handleFor` on the
 *  server: the doc-only form is unambiguous only when the doc holds ONE entry,
 *  so a bundle shows the qualified form — which is what stops somebody copying a
 *  handle that will refuse as ambiguous. */
export const handlesFor = (secret: WorkspaceSecret): string[] =>
  secret.entries.length === 1 ? [`«secret:${secret.name}»`] : secret.entries.map((e) => `«secret:${secret.name}.${e.key}»`)
