// The server calls behind the Comms rail's row menus that had no client
// helper yet (lib/comms-row-menu.ts lists the items). Each is one request;
// the rail refreshes its own queries afterwards.

import { postJson } from './fetch-json'

/** Step the caller's read cursor back one countable turn, so the thread
 *  reads unread again. `marked` is false when it holds nothing that could. */
export const markConversationUnread = (id: string) =>
  postJson<{ ok: true; marked: boolean }>(`/api/conversations/${id}/unread`)

/** The same for a channel, a relay or a DM, on the main feed. */
export const markChannelUnread = (id: string) => postJson<{ ok: true; marked: boolean }>(`/api/channels/${id}/unread`)

/** Open an app path in a new browser tab (in Talaria Desktop, the default
 *  browser — the shell hands every new window there). */
export function openInNewTab(path: string): void {
  window.open(`${window.location.origin}${path}`, '_blank', 'noopener')
}
