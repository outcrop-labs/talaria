// Client view of WORKING SECRETS — the credentials a person saves, shares, and
// reads back.
//
// NOT `lib/workspace-secrets.ts`, which is the admin view of credentials agents
// SPEND and nobody ever reads. Three nouns now share the word "secret" in this
// codebase; they are kept in separate files because an operator — or a
// developer — who conflates them will eventually reveal, revoke or delete the
// wrong one.
//
// THE ONE SHAPE THAT CARRIES A VALUE is `reveal()`'s return, and it is a
// function call rather than a field on a cached row. That is deliberate: a value
// on `WorkingSecret` would sit in the query cache, get serialised into devtools,
// and survive every re-render until the tab closed. Here it exists as a local in
// whoever asked, for as long as they hold it.
import { createQuery, useQueryClient } from '@tanstack/svelte-query'
import { delJson, errorMessage, getJson, patchJson, postJson } from '@/lib/fetch-json'

export interface WorkingSecret {
  id: string
  name: string
  title: string
  note: string | null
  createdAt: string
  expiresAt: string | null
  lastUsedAt: string | null
  ownerUserId: string | null
  folderId: string | null
  /** Keys and labels. There is no field here that could hold a value. */
  entries: Array<{ key: string; label: string }>
  /** People who may reveal it. */
  readers: string[]
  /** Agents that may SPEND it — handle only; they can never read it. */
  grants: string[]
  allowedHosts: string[]
}

export interface SecretFolder {
  id: string
  name: string
  ownerUserId: string | null
  createdAt: string
  /** People the whole folder is shared with — they can reveal everything in it. */
  readers: string[]
  /** Agents that may SPEND everything in it, and read none of it. */
  grants: string[]
  count: number
}

export const WORKING_SECRETS_KEY = ['working-secrets']
export const SECRET_FOLDERS_KEY = ['secret-folders']

export const useSecretFolders = () =>
  createQuery(() => ({
    queryKey: SECRET_FOLDERS_KEY,
    queryFn: (): Promise<{ folders: SecretFolder[] }> => getJson<{ folders: SecretFolder[] }>('/api/secrets/folders'),
  }))

export const useWorkingSecrets = () =>
  createQuery(() => ({
    queryKey: WORKING_SECRETS_KEY,
    queryFn: (): Promise<{ secrets: WorkingSecret[] }> => getJson<{ secrets: WorkingSecret[] }>('/api/secrets'),
  }))

export interface NewSecret {
  title: string
  entries: Array<{ key: string; label: string; value: string }>
  note?: string | null
  folderId?: string | null
  readers?: string[]
  grantTo?: string[]
  allowedHosts?: string[]
}

// The vault surfaces failures as an in-band `error` field (the panels read it
// rather than catching), so this wrapper resolves every failure into that
// envelope instead of letting the door's rejection escape.
const post = async (url: string, body: unknown, method: 'POST' | 'PATCH' | 'DELETE' = 'POST'): Promise<{ error?: string; value?: string }> => {
  const verb = method === 'PATCH' ? patchJson : method === 'DELETE' ? delJson : postJson
  return verb<{ error?: string; value?: string }>(url, body).catch((e: unknown) => ({ error: errorMessage(e) }))
}

export function useSecretsVault() {
  const qc = useQueryClient()
  // Folder changes move secrets between piles and can change who can see what,
  // so both lists are refetched together rather than each verb guessing which
  // it touched.
  const refresh = async () => {
    await qc.invalidateQueries({ queryKey: WORKING_SECRETS_KEY })
    await qc.invalidateQueries({ queryKey: SECRET_FOLDERS_KEY })
  }

  return {
    save: async (s: NewSecret) => {
      const r = await post('/api/secrets', s)
      await refresh()
      return r
    },
    /** File it into a folder, or out of one with `null`. */
    move: async (name: string, folderId: string | null) => {
      const r = await post('/api/secrets', { name, folderId }, 'PATCH')
      await refresh()
      return r
    },
    remove: async (name: string) => {
      const r = await post('/api/secrets', { name }, 'DELETE')
      await refresh()
      return r
    },
    share: async (name: string, userId: string, on: boolean) => {
      const r = await post('/api/secrets/share', { action: on ? 'share' : 'unshare', name, userId })
      await refresh()
      return r
    },
    grant: async (name: string, agentModel: string, on: boolean) => {
      const r = await post('/api/secrets/share', { action: on ? 'grant' : 'revoke', name, agentModel })
      await refresh()
      return r
    },
    /** THE ONE CALL THAT RETURNS A CREDENTIAL. Never cached, never stored — the
     *  caller holds it in a local and drops it. One entry per call, because the
     *  server audits per entry and a bulk reveal would record one look for
     *  several credentials. */
    newFolder: async (name: string) => {
      const r = await post('/api/secrets/folders', { action: 'create', name })
      await refresh()
      return r
    },
    renameFolder: async (id: string, name: string) => {
      const r = await post('/api/secrets/folders', { action: 'rename', id, name })
      await refresh()
      return r
    },
    deleteFolder: async (id: string) => {
      const r = await post('/api/secrets/folders', { action: 'delete', id })
      await refresh()
      return r
    },
    /** Share a whole folder — with a person (can reveal all of it) or an agent
     *  (can spend all of it, sees none of it). */
    shareFolder: async (id: string, who: { userId?: string; agentModel?: string }, on: boolean) => {
      const r = await post('/api/secrets/folders', { action: 'share', id, on, ...who })
      await refresh()
      return r
    },
    reveal: async (name: string, key: string): Promise<{ value?: string; error?: string }> => post('/api/secrets/reveal', { name, key }),
  }
}

/** SEARCH, over everything EXCEPT the one field that matters.
 *
 *  Titles, notes, entry labels and keys — never a value, and there is not one in
 *  the client to search anyway. Worth stating because "search your secrets" is a
 *  sentence somebody could reasonably read as searching their contents, and the
 *  honest answer is that we cannot and would not. */
export const matchesSecret = (s: WorkingSecret, needle: string): boolean => {
  const q = needle.trim().toLowerCase()
  if (!q) return true
  const hay = [s.title, s.note ?? '', ...s.entries.map((e) => `${e.label} ${e.key}`), ...s.allowedHosts, ...s.grants].join(' ').toLowerCase()
  return q.split(/\s+/).every((word) => hay.includes(word))
}

/** How a refusal reads to the person who hit it. The server answers with a bare
 *  reason so it can stay a stable API; the sentence belongs here. */
export const REVEAL_ERROR: Record<string, string> = {
  unknown: 'That secret no longer exists.',
  'not-revealable': 'This is an agent credential. Nobody can read it, by design.',
  'not-shared': 'This has not been shared with you.',
  expired: 'This secret has expired.',
  'no-such-entry': 'That entry is not in this secret.',
  destroyed: 'This was a one-shot and has already been used. The value is gone.',
}
