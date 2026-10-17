// Blueprint client: reusable prompt bodies, offered by the composer's "/" menu.
//
// A blueprint is text a person wants to send again — a brief they always open
// with, a review checklist, the shape of a handoff. Two kinds, one table: a
// PERSONAL one (owner = its author, nobody else sees it) and a SHARED one
// (owner null, every member reads it, writing it needs `templates.manage`).
// The list read answers both at once — the caller's own plus the shared set —
// because the menu that shows them does not care which is which beyond the
// badge, and two reads would have meant two loading states for one list.
//
// Templates (lib/templates.ts) are the other library and stay separate: a
// template is the FORMAT of a record the app writes (a ticket, a plan), bound
// to boards and filled by an agent. A blueprint is a message you are about to
// send, and its whole life is the composer.
import { createQuery } from '@tanstack/svelte-query'
import { delJson, getList, postJson, putJson } from '@/lib/fetch-json'
import { toastError } from '@/lib/toast.svelte'

export interface Blueprint {
  id: string
  name: string
  body: string
  /** Org-wide (owner null) rather than the viewer's own. */
  shared: boolean
  /** Null on a shared one. */
  ownerUserId: string | null
  /** Which composer offers it ('work' today). */
  surface: string
  sort: number
  createdAt: string
  updatedAt: string
}

/** One key, because it is one read: the viewer's own plus the shared set. */
export const BLUEPRINTS_KEY = ['blueprints'] as const

export function useBlueprints() {
  return createQuery(() => ({
    queryKey: BLUEPRINTS_KEY,
    queryFn: (): Promise<Blueprint[]> => getList<Blueprint>('/api/blueprints', 'blueprints'),
  }))
}

export const createBlueprint = (b: { name: string; body?: string; shared?: boolean }) =>
  postJson<{ blueprint: Blueprint }>('/api/blueprints', b)

export const updateBlueprint = (id: string, patch: { name?: string; body?: string; shared?: boolean }) =>
  putJson<{ blueprint: Blueprint }>(`/api/blueprints/${id}`, patch)

/** The delete is fired from a row-hover control that does not wait on it
 *  (`void remove(b)`), so a refusal — a shared blueprint without
 *  `templates.manage`, someone else's personal one — is surfaced here rather
 *  than left as an unhandled rejection. Same bargain as `deleteTemplate`. */
export const deleteBlueprint = (id: string) =>
  delJson<{ ok: true }>(`/api/blueprints/${id}`).catch((e: unknown) => toastError('Delete failed', e))

/** The one line a blueprint shows beside its name: the first line of the body
 *  that says anything, with the markdown that makes it a heading, a quote or a
 *  bullet taken off. Capped so a body with no newlines cannot hand the menu a
 *  20,000-character string to measure — the row truncates visually, and a
 *  marker would be the ellipsis the copy rules ban. */
export function blueprintSummary(body: string): string {
  for (const raw of body.split('\n')) {
    const line = raw.replace(/^\s*(?:[#>]+\s*|[*+-]\s+|\d+[.)]\s+)/, '').trim()
    if (line) return line.slice(0, 120)
  }
  return ''
}

/** Whose row offers a delete. Only the viewer's OWN personal blueprint: a
 *  shared one is the org's and goes through the library that manages it (the
 *  api refuses it here anyway, and a control that only ever fails is worse
 *  than no control). */
export function canDeleteBlueprint(b: Pick<Blueprint, 'ownerUserId'>, viewerId: string | null | undefined): boolean {
  return !!viewerId && b.ownerUserId === viewerId
}
