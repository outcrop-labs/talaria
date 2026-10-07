// STARRED CONVERSATIONS, kept per person in this browser.
//
// The Comms rail's Starred section (above Channels) lists the conversations a
// person has starred. Stars are local, like drafts: they live in localStorage
// under
//
//   talaria:comms-stars:{userId}
//
// as a JSON array of tagged keys, in the order they were starred (newest
// last). The user id is in the key so a shared browser never shows one
// person's favorites to the next. Three kinds of key exist:
//
//   • `channel:<channelId>` — a channel, a relay, or a person's DM (a DM is a
//     channel of kind `dm`), so one key kind covers all three.
//   • `agent:<model>`       — an agent's DM.
//   • `thread:<id>`         — one thread with an agent (a conversation),
//     pinned from its row's hover action; it lists in Starred on its own.
//
// Anything else stored under the key — another kind, a non-string, a
// duplicate — is dropped on read, and toggling an unknown kind is a no-op.

import { readStored, writeStored } from './persist'

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

export function isStarred(userId: string, key: string): boolean {
  return readStars(userId).includes(key as StarKey)
}

/** Star `key`, or unstar it if it is starred. Returns the new list. An unknown
 *  key kind changes nothing. */
export function toggleStar(userId: string, key: string): StarKey[] {
  const stars = readStars(userId)
  if (!isStarKey(key)) return stars
  const next = stars.includes(key) ? stars.filter((k) => k !== key) : [...stars, key]
  writeStored(starsStorageKey(userId), next)
  return next
}
