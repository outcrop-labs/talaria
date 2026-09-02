// Client view of the secrets inventory. The shapes mirror the Rust secret
// health engine (api/src/secret_health.rs)
// exactly — including the absence of any field that could hold a plaintext
// secret, which is the point: there is nowhere for one to arrive.
import { createQuery, useQueryClient } from '@tanstack/svelte-query'
import { delJson, errorMessage, getJson } from '@/lib/fetch-json'

/** A reactive argument: pass a plain value, or a getter for values that change
 *  over a component's life (route params, selections). */
type MaybeGetter<T> = T | (() => T)
const resolve = <T>(v: MaybeGetter<T>): T => (typeof v === 'function' ? (v as () => T)() : v)

export type SecretState = 'ok' | 'unreadable' | 'missing' | 'env'
export type SecretGroup = 'models' | 'integrations' | 'agents' | 'platform'

export interface SecretRow {
  id: string
  group: SecretGroup
  label: string
  unlocks: string
  surface: string
  href?: string
  state: SecretState
  scope: 'instance' | 'user' | 'agent'
  owner?: string
  setAt?: string | null
  lastUsedAt?: string | null
  expiresAt?: string | null
  clearable: boolean
}

export interface RootHealth {
  via: 'env' | 'file' | 'fallback' | 'absent'
  name: string
  state: 'ok' | 'fallback' | 'absent' | 'unreadable'
  failure: string | null
  activeVersion: number | null
  loadedVersions: number[]
  storedVersions: number
}

export interface SecretHealth {
  root: RootHealth
  rows: SecretRow[]
  counts: { ok: number; unreadable: number; missing: number; env: number }
}

export const SECRETS_KEY = ['admin-secrets'] as const

/** `enabled` so the admin-only banner can mount for everyone and simply not
 *  ask — a member hitting this endpoint gets a 403, and a 403 in the console
 *  on every page load is noise that trains people to ignore the console. */
export const useSecretHealth = (enabled: MaybeGetter<boolean> = true) =>
  createQuery(() => ({
    queryKey: SECRETS_KEY,
    queryFn: (): Promise<SecretHealth> => getJson<SecretHealth>('/api/admin/secrets'),
    enabled: resolve(enabled),
    // The banner mounts app-wide, so this runs on every navigation. One probe
    // is eight queries; a minute of staleness is invisible for a state that
    // only changes when an operator does something.
    staleTime: 60_000,
  }))

export const GROUP_LABELS: Record<SecretGroup, string> = {
  models: 'Models',
  integrations: 'Integrations',
  agents: 'Agents',
  platform: 'Platform',
}

/** What each state means, in the operator's words. Used by the row and by the
 *  banner, so the two can never describe the same state differently. */
export const STATE_COPY: Record<SecretState, { label: string; hint: string }> = {
  ok: { label: 'Readable', hint: 'Sealed and readable with this instance’s current key.' },
  unreadable: {
    label: 'Unreadable',
    hint: 'Sealed with a key this instance no longer has. Restore the original root secret to recover it, or clear it and enter the value again.',
  },
  missing: { label: 'Not set', hint: 'Nothing is configured here.' },
  env: {
    label: 'From environment',
    hint: 'Read from an environment variable rather than stored here. Change it where the process is configured.',
  },
}

/** One place that clears, so the invalidation cannot be forgotten at a call
 *  site — a stale inventory after a clear reads as "the clear did nothing". */
export function useClearSecret() {
  const qc = useQueryClient()
  return async (body: { id: string } | { unreadable: true }): Promise<{ error?: string; cleared?: string[]; failed?: string[]; changed?: boolean }> => {
    // The surface reads an in-band `error` field rather than catching, so a
    // refusal (and a network failure, which used to reject outright) resolves
    // as the same envelope.
    const r = await delJson<{ cleared?: string[]; failed?: string[]; changed?: boolean }>('/api/admin/secrets', body).then(
      (out) => out,
      (e: unknown): { error: string } => ({ error: errorMessage(e) }),
    )
    await qc.invalidateQueries({ queryKey: SECRETS_KEY })
    return r
  }
}
