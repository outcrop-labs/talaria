// UNSENT COMPOSER TEXT, kept per person per conversation (KTD10).
//
// A draft is local: it lives in localStorage under
//
//   comms-draft:{userId}:{key}
//
// where `key` is the channel id, the agent conversation id, or
// `agent:<model>:new` for an agent thread that does not exist yet (the composer
// at /comms/agent/<model>). The user id is in the key so a shared browser never
// shows one person's half-written message to the next person who signs in.
//
// The composers write on input (debounced), restore on mount, and clear on
// send. Whitespace-only text is not a draft — saving it removes the key, so
// deleting everything you typed deletes the draft too.
//
// The composer finds its key through a Svelte context that Comms provides
// (`provideCommsDraftScope`), because the composers are mounted by views that
// do not know they are inside Comms. One composer OWNS a key at a time: the
// channel's main composer claims the channel id first, so a thread-reply
// composer or a modal's composer in the same channel cannot read or overwrite
// that draft.
//
// Every save and clear announces itself with a DRAFTS_CHANGED window event, so
// the rail's draft marks (comms-drafts.svelte.ts) follow the composer as you
// type. Another tab's writes arrive as the browser's own `storage` event.

import { getContext, setContext } from 'svelte'
import { readStored, writeStored, writeText } from './persist'
import type { CommsSelection } from './comms-selection'

const PREFIX = 'comms-draft:'

/** Fired on `window` after any draft is saved or cleared in this tab. */
export const DRAFTS_CHANGED = 'comms-drafts-changed'

/** True for a localStorage key that holds a Comms draft. */
export const isDraftStorageKey = (storageKey: string | null): boolean => !!storageKey?.startsWith(PREFIX)

function announce(): void {
  try {
    if (typeof window.dispatchEvent === 'function') window.dispatchEvent(new Event(DRAFTS_CHANGED))
  } catch {
    // No window (tests, SSR): nobody is listening.
  }
}

export interface CommsDraft {
  key: string
  text: string
  /** Epoch ms of the last save — the Drafts view lists newest first. */
  updatedAt: number
}

interface StoredDraft {
  text: string
  at: number
}

function parseDraft(raw: unknown): StoredDraft | null {
  if (!raw || typeof raw !== 'object') return null
  const v = raw as Record<string, unknown>
  if (typeof v.text !== 'string' || !v.text.trim()) return null
  return { text: v.text, at: typeof v.at === 'number' ? v.at : 0 }
}

export const draftStorageKey = (userId: string, key: string): string => `${PREFIX}${userId}:${key}`

/** The key for an agent thread that has not been created yet. */
export const newAgentDraftKey = (model: string): string => `agent:${model}:new`

/** The agent model of a `newAgentDraftKey` key; null for any other key. */
export function parseNewAgentDraftKey(key: string): string | null {
  return /^agent:(.+):new$/.exec(key)?.[1] ?? null
}

/** The draft key for whatever Comms has selected; null where there is no composer. */
export function commsDraftKey(sel: CommsSelection | null): string | null {
  if (!sel) return null
  if (sel.t === 'channel') return sel.id
  if (sel.t === 'agent') return sel.conversationId ?? newAgentDraftKey(sel.model)
  return null
}

export function readDraft(userId: string, key: string): string | null {
  return readStored<StoredDraft | null>(draftStorageKey(userId, key), parseDraft, null)?.text ?? null
}

/** Save, or remove when the text is only whitespace. */
export function saveDraft(userId: string, key: string, text: string, now: number = Date.now()): void {
  if (!text.trim()) {
    clearDraft(userId, key)
    return
  }
  writeStored(draftStorageKey(userId, key), { text, at: now } satisfies StoredDraft)
  announce()
}

export function clearDraft(userId: string, key: string): void {
  writeText(draftStorageKey(userId, key), null)
  announce()
}

/** This person's drafts, newest first. Another user's keys are never read. */
export function listDrafts(userId: string): CommsDraft[] {
  const prefix = `${PREFIX}${userId}:`
  const keys: string[] = []
  try {
    const store = window.localStorage
    for (let i = 0; i < store.length; i++) {
      const k = store.key(i)
      if (k?.startsWith(prefix)) keys.push(k)
    }
  } catch {
    return []
  }
  const out: CommsDraft[] = []
  for (const storageKey of keys) {
    const d = readStored<StoredDraft | null>(storageKey, parseDraft, null)
    if (d) out.push({ key: storageKey.slice(prefix.length), text: d.text, updatedAt: d.at })
  }
  return out.sort((a, b) => b.updatedAt - a.updatedAt)
}

/**
 * Where a draft lives, as a Comms path — or null when its conversation is gone
 * (or has not loaded). A channel id is checked before a conversation id; the
 * two id spaces do not overlap in practice, and a channel is the likelier
 * owner of a plain id.
 */
export function draftTarget(
  key: string,
  lookup: { channelIds: string[]; conversations: { id: string; agentModel: string }[] },
): string | null {
  const fresh = parseNewAgentDraftKey(key)
  if (fresh !== null) return `/comms/agent/${encodeURIComponent(fresh)}`
  if (lookup.channelIds.includes(key)) return `/comms/channel/${encodeURIComponent(key)}`
  const conv = lookup.conversations.find((c) => c.id === key)
  if (conv) return `/comms/agent/${encodeURIComponent(conv.agentModel)}/${encodeURIComponent(conv.id)}`
  return null
}

// ── the scope Comms hands its composers ─────────────────────────────────────

export interface CommsDraftScope {
  userId: () => string | null
  key: () => string | null
  /** key → the composer that owns it right now. */
  owners: Map<string, symbol>
}

const SCOPE = Symbol('comms-draft-scope')

/** Comms calls this during init; every composer below it saves drafts. */
export function provideCommsDraftScope(userId: () => string | null, key: () => string | null): void {
  setContext<CommsDraftScope>(SCOPE, { userId, key, owners: new Map() })
}

/** A composer calls this during init; undefined outside Comms (no drafts). */
export function useCommsDraftScope(): CommsDraftScope | undefined {
  return getContext<CommsDraftScope | undefined>(SCOPE)
}

/** First composer to ask for a key gets it; true if `token` owns it now. */
export function claimDraftKey(scope: CommsDraftScope, key: string, token: symbol): boolean {
  const owner = scope.owners.get(key)
  if (owner && owner !== token) return false
  scope.owners.set(key, token)
  return true
}

export function releaseDraftKey(scope: CommsDraftScope, key: string, token: symbol): void {
  if (scope.owners.get(key) === token) scope.owners.delete(key)
}
