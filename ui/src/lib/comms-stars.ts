// PINNED CONVERSATIONS — the key grammar, and the legacy local copy.
//
// The Comms rail's Pinned section (above Channels) lists what a person
// pinned. The SERVER holds the list now (`/api/me/pins`, read and written by
// comms-stars.svelte.ts); this file keeps the grammar both sides share, and
// the reader for the copy older builds kept in localStorage under
//
//   talaria:comms-stars:{userId}
//
// which the hook folds into the server once and then clears. The names say
// "star" because that is what the rail called a pin before; three kinds of
// key exist:
//
//   • `channel:<channelId>` — a channel, a relay, or a person's DM (a DM is a
//     channel of kind `dm`), so one key kind covers all three.
//   • `agent:<model>`       — an agent's DM.
//   • `thread:<id>`         — one thread with an agent (a conversation); it
//     lists in Pinned on its own.
//
// Anything else — another kind, a non-string, a duplicate — is dropped on
// read, and toggling an unknown kind is a no-op.

import { HttpError } from './fetch-json'
import { readStored } from './persist'

export type StarKey = `channel:${string}` | `agent:${string}` | `thread:${string}`
export type StarKind = 'channel' | 'agent' | 'thread'

const PREFIX = 'talaria:comms-stars:'

export const starsStorageKey = (userId: string): string => `${PREFIX}${userId}`

/** The user id behind a stars storage key; null for any other key. */
export const starsStorageUser = (storageKey: string): string | null =>
  storageKey.startsWith(PREFIX) ? storageKey.slice(PREFIX.length) : null

export const channelStarKey = (channelId: string): StarKey => `channel:${channelId}`
export const agentStarKey = (model: string): StarKey => `agent:${model}`
export const threadStarKey = (conversationId: string): StarKey => `thread:${conversationId}`

export function isStarKey(key: unknown): key is StarKey {
  return typeof key === 'string' && /^(channel|agent|thread):./.test(key)
}

/** A star key split into its kind and target id; null for an unknown kind. */
export function parseStarKey(key: string): { kind: StarKind; id: string } | null {
  if (!isStarKey(key)) return null
  const at = key.indexOf(':')
  return { kind: key.slice(0, at) as StarKind, id: key.slice(at + 1) }
}

function parseStars(raw: unknown): StarKey[] | null {
  if (!Array.isArray(raw)) return null
  return [...new Set(raw.filter(isStarKey))]
}

export function readStars(userId: string): StarKey[] {
  return readStored(starsStorageKey(userId), parseStars, [])
}

/** What one legacy star's upload to the server came to. */
export type UploadOutcome = 'landed' | 'refused' | 'retry'

/** Classify an upload's failure. A 4xx the server will give again — a key it
 *  will not take, the pin cap — is `refused`: keeping it would retry it on
 *  every load for ever. No answer at all, a 5xx, a 401 or 403 (a session
 *  that lapsed or is mid-refresh), a 408 or a 429 is `retry`: the copy stays
 *  in this browser for the next load. */
export function uploadOutcome(e: unknown): Exclude<UploadOutcome, 'landed'> {
  if (!(e instanceof HttpError)) return 'retry'
  const s = e.status
  if (s >= 500 || s === 401 || s === 403 || s === 408 || s === 429) return 'retry'
  return s >= 400 ? 'refused' : 'retry'
}

/** The legacy stars to keep in this browser after an upload pass: only the
 *  ones worth another try. Everything that landed or was refused for good
 *  goes, so a pass where nothing needs retrying clears the copy. */
export function starsToKeep(results: readonly { key: StarKey; outcome: UploadOutcome }[]): StarKey[] {
  return results.filter((r) => r.outcome === 'retry').map((r) => r.key)
}
