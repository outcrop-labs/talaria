// WHAT EACH COMMS RAIL ROW'S MENU OFFERS — the right-click, the Menu key, a
// long-press and the row's "…" button all open this one list. Pure: the rail
// (CommsSidebar.svelte) says what a row is and what the person may do, hands
// in the functions that do it, and gets the entries back, so which items a
// row shows — by kind and by permission — is tested here without a browser.
//
// TWO FAMILIES. A CONVERSATION (a thread with an agent, a channel, a relay, a
// DM, a group DM) is something you read: open it, share it, file it, tune
// what it rings for, retire it. An AGENT is someone you talk to: start a
// conversation, look them up, file them, quiet their DMs. Every item is an
// existing server action (or a small new one) — nothing here keeps state of
// its own.
//
// ONLY WHAT THE PERSON MAY DO. A thread's rename, archive and delete are its
// owner's; a channel's are its owner's, and a DM has none of them; an
// agent's settings are its managers'. An item they may not use is left out,
// not shown disabled. Pinning needs a signed-in person.
//
// Order follows docs/UI-CONVENTIONS.md §Context menus: the primary action,
// Copy link, then the rest, and anything destructive last behind a separator.

import { Bell, BellOff } from '@lucide/svelte'
import type { ContextMenuEntry, ContextMenuItem } from '@/components/ui/context-menu.svelte'
import {
  LEVELS,
  MUTE_PRESETS,
  isMuted,
  levelLabel,
  muteLabel,
  muteUntil,
  mutedLabel,
  type ChannelLevel,
  type ChannelNotify,
} from './channel-notify'

export type ChannelRowKind = 'channel' | 'group' | 'dm'

export type ConversationRow =
  | {
      kind: 'thread'
      id: string
      model: string
      /** The caller owns the thread (a collaborator does not). */
      owner: boolean
      unread: number
      /** The thread is the one open now: its cursor advances as it is read,
       *  so "Mark unread" would undo itself. */
      active: boolean
    }
  | {
      kind: 'channel'
      id: string
      channelKind: ChannelRowKind
      /** The caller's channel role is owner. */
      owner: boolean
      unread: number
      active: boolean
      /** The caller's own level and mute (null: an api that serves none). */
      notify: ChannelNotify | null
    }

export interface AgentRowSubject {
  kind: 'agent'
  model: string
  /** May open the agent's settings (an admin, or one of its managers). */
  canManage: boolean
  /** The caller's running mute of this agent's DMs (ISO), or null. */
  mutedUntil: string | null
}

export interface RowMenuContext {
  /** Signed in — pins and mutes are a person's own. */
  signedIn: boolean
  /** The row is pinned to the top of the caller's rail. */
  pinned: boolean
  /** Inside Talaria Desktop, where a new-window link opens in the browser. */
  desktop: boolean
  now: number
}

/** What a conversation row's items do. */
export interface ConversationActions {
  open: () => void
  openElsewhere: (path: string) => void
  copyLink: (path: string) => void
  markRead: () => void
  markUnread: () => void
  togglePin: () => void
  rename: () => void
  setLevel: (level: ChannelLevel) => void
  /** ISO to mute until, or null to unmute. */
  mute: (until: string | null) => void
  archive: () => void
  remove: () => void
  /** A DM's other person — present only for a two-person DM. */
  viewProfile?: () => void
}

/** What an agent row's items do. */
export interface AgentActions {
  newConversation: () => void
  copyLink: (path: string) => void
  viewProfile: () => void
  editAgent: () => void
  togglePin: () => void
  muteDms: (until: string | null) => void
}

export const conversationPath = (row: ConversationRow): string =>
  row.kind === 'thread' ? `/comms/agent/${row.model}/${row.id}` : `/comms/channel/${row.id}`

export const agentPath = (model: string): string => `/comms/agent/${model}`

/** "Open in new tab" in a browser; in Talaria Desktop a new window is the
 *  default browser (#671 hands off every http(s) window there), so it says
 *  where it goes. */
export const openElsewhereLabel = (desktop: boolean): string => (desktop ? 'Open in browser' : 'Open in new tab')

function pinItem(ctx: RowMenuContext, toggle: () => void): ContextMenuEntry[] {
  return ctx.signedIn ? [{ label: ctx.pinned ? 'Unpin' : 'Pin', onSelect: toggle }] : []
}

/** The timed-mute presets, and Unmute while one runs. */
function muteChildren(mutedUntil: string | null, now: number, mute: (until: string | null) => void): ContextMenuEntry[] {
  // Icons as the header's bell menu has them, which also lines the labels up
  // with a level list's check lane above them.
  const out: ContextMenuEntry[] = MUTE_PRESETS.map((p) => ({
    label: muteLabel(p),
    icon: [BellOff, { size: 14 }],
    onSelect: () => mute(muteUntil(p, now)),
  }))
  if (isMuted({ mutedUntil }, now)) out.push('sep', { label: 'Unmute', icon: [Bell, { size: 14 }], onSelect: () => mute(null) })
  return out
}

/** Drop a separator that leads, trails or doubles — the sections above are
 *  conditional, so the joins are tidied once here. */
function tidy(entries: ContextMenuEntry[]): ContextMenuEntry[] {
  const out: ContextMenuEntry[] = []
  for (const e of entries) {
    if (e === 'sep' && (out.length === 0 || out[out.length - 1] === 'sep')) continue
    out.push(e)
  }
  while (out[out.length - 1] === 'sep') out.pop()
  return out
}

export function conversationMenu(row: ConversationRow, ctx: RowMenuContext, a: ConversationActions): ContextMenuEntry[] {
  const path = conversationPath(row)
  const isDm = row.kind === 'channel' && row.channelKind === 'dm'
  // A channel's or a relay's name and life are its owner's; a DM has neither
  // a name to change nor a life to end from here.
  const mayManage = row.kind === 'thread' ? row.owner : row.owner && !isDm
  // A room the person is in through its board or a team has no read marker
  // of their own to move back (the server answers 409), so it offers none.
  const ownMarker = row.kind === 'thread' || row.notify?.settable !== false
  const read: ContextMenuItem[] =
    row.unread > 0
      ? [{ label: 'Mark read', onSelect: a.markRead }]
      : row.active || !ownMarker
        ? []
        : [{ label: 'Mark unread', onSelect: a.markUnread }]
  const notify: ContextMenuEntry[] = []
  if (row.kind === 'channel' && row.notify?.settable && ctx.signedIn) {
    const n = row.notify
    const muted = isMuted(n, ctx.now)
    notify.push({
      label: 'Notifications',
      hint: muted ? (mutedLabel(n, ctx.now) ?? 'Muted') : levelLabel(n.level),
      children: [
        ...LEVELS.map((l) => ({ label: levelLabel(l), checked: l === n.level, onSelect: () => a.setLevel(l) })),
        'sep',
        ...muteChildren(n.mutedUntil, ctx.now, a.mute),
      ],
    })
  }
  return tidy([
    { label: 'Open', onSelect: a.open },
    { label: 'Copy link', onSelect: () => a.copyLink(path) },
    { label: openElsewhereLabel(ctx.desktop), onSelect: () => a.openElsewhere(path) },
    ...(a.viewProfile ? [{ label: 'View profile', onSelect: a.viewProfile }] : []),
    'sep',
    ...read,
    ...pinItem(ctx, a.togglePin),
    ...notify,
    ...(mayManage ? [{ label: 'Rename…', onSelect: a.rename }] : []),
    'sep',
    ...(mayManage
      ? ([
          { label: 'Archive', onSelect: a.archive },
          { label: 'Delete…', danger: true, onSelect: a.remove },
        ] as ContextMenuEntry[])
      : []),
  ])
}

export function agentMenu(row: AgentRowSubject, ctx: RowMenuContext, a: AgentActions): ContextMenuEntry[] {
  const muted = isMuted({ mutedUntil: row.mutedUntil }, ctx.now)
  return tidy([
    { label: 'New conversation', onSelect: a.newConversation },
    { label: 'Copy link', onSelect: () => a.copyLink(agentPath(row.model)) },
    { label: 'View profile', onSelect: a.viewProfile },
    ...(row.canManage ? [{ label: 'Agent settings', onSelect: a.editAgent }] : []),
    'sep',
    ...pinItem(ctx, a.togglePin),
    ...(ctx.signedIn
      ? [
          {
            label: muted ? 'DMs muted' : 'Mute DMs',
            hint: muted ? (mutedLabel({ mutedUntil: row.mutedUntil }, ctx.now) ?? undefined) : undefined,
            children: muteChildren(row.mutedUntil, ctx.now, a.muteDms),
          },
        ]
      : []),
  ])
}

/** The menu for a person with no DM yet: there is nothing to file or tune
 *  until a conversation exists. */
export function personMenu(a: { message: () => void; viewProfile: () => void }): ContextMenuEntry[] {
  return [
    { label: 'Message', onSelect: a.message },
    { label: 'View profile', onSelect: a.viewProfile },
  ]
}

/** What a single click on an agent's row does: open a BLANK conversation
 *  with it — a fresh composer addressed to the agent, with no thread created
 *  until the first message is sent — and fan its recent threads out under
 *  the row so the one to resume is a click away. The owner's ticket floated
 *  "or jump to the existing recent one"; blank is the default it chose. Flip
 *  `AGENT_CLICK` to 'recent' to land on the newest thread instead. */
export const AGENT_CLICK: 'blank' | 'recent' = 'blank'

export type AgentClick = { t: 'new' } | { t: 'thread'; id: string }

/** Where an agent click lands, given its threads newest first. */
export function agentClickTarget(threads: readonly { id: string }[], mode: 'blank' | 'recent' = AGENT_CLICK): AgentClick {
  if (mode === 'recent' && threads[0]) return { t: 'thread', id: threads[0].id }
  return { t: 'new' }
}
