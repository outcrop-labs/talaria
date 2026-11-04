import assert from 'node:assert/strict'
import { test, vi } from 'vitest'
import type { ContextMenuEntry, ContextMenuItem } from '@/components/ui/context-menu.svelte'
import {
  AGENT_CLICK,
  agentClickTarget,
  agentMenu,
  conversationMenu,
  personMenu,
  type AgentActions,
  type ConversationActions,
  type ConversationRow,
  type RowMenuContext,
} from './comms-row-menu'

const NOW = Date.parse('2026-11-04T12:00:00.000Z')
const ctx = (over: Partial<RowMenuContext> = {}): RowMenuContext => ({
  signedIn: true,
  pinned: false,
  desktop: false,
  now: NOW,
  ...over,
})

const convActions = (): ConversationActions => ({
  open: vi.fn(),
  openElsewhere: vi.fn(),
  copyLink: vi.fn(),
  markRead: vi.fn(),
  markUnread: vi.fn(),
  togglePin: vi.fn(),
  rename: vi.fn(),
  setLevel: vi.fn(),
  mute: vi.fn(),
  archive: vi.fn(),
  remove: vi.fn(),
})

const agentActions = (): AgentActions => ({
  newConversation: vi.fn(),
  copyLink: vi.fn(),
  viewProfile: vi.fn(),
  editAgent: vi.fn(),
  togglePin: vi.fn(),
  muteDms: vi.fn(),
})

/** The menu as its visible words: labels, '—' for a separator, a submenu as
 *  `Label ▸ [children]`. */
const words = (entries: ContextMenuEntry[]): unknown[] =>
  entries.map((e) => (e === 'sep' ? '—' : e.children ? [`${e.label} ▸`, words(e.children)] : e.label))

const item = (entries: ContextMenuEntry[], label: string): ContextMenuItem => {
  const found = entries.find((e): e is ContextMenuItem => e !== 'sep' && e.label === label)
  assert.ok(found, `no "${label}" in ${JSON.stringify(words(entries))}`)
  return found
}

const thread = (over: Partial<Extract<ConversationRow, { kind: 'thread' }>> = {}): ConversationRow => ({
  kind: 'thread',
  id: 't1',
  model: 'dev',
  owner: true,
  unread: 0,
  active: false,
  ...over,
})

const channel = (over: Partial<Extract<ConversationRow, { kind: 'channel' }>> = {}): ConversationRow => ({
  kind: 'channel',
  id: 'c1',
  channelKind: 'channel',
  groupDm: false,
  owner: true,
  unread: 0,
  active: false,
  notify: { level: 'mentions', mutedUntil: null, settable: true },
  ...over,
})

test("a thread's owner gets the whole menu, primary first, Copy link second, destructive last", () => {
  assert.deepEqual(words(conversationMenu(thread(), ctx(), convActions())), [
    'Open',
    'Copy link',
    'Open in new tab',
    '—',
    'Mark unread',
    'Pin',
    'Rename…',
    '—',
    'Archive',
    'Delete…',
  ])
  const del = item(conversationMenu(thread(), ctx(), convActions()), 'Delete…')
  assert.equal(del.danger, true)
})

test('a collaborator on a thread cannot rename, archive or delete it', () => {
  assert.deepEqual(words(conversationMenu(thread({ owner: false }), ctx(), convActions())), [
    'Open',
    'Copy link',
    'Open in new tab',
    '—',
    'Mark unread',
    'Pin',
  ])
})

test('read state: unread offers Mark read; read offers Mark unread, except on the open thread', () => {
  const labels = (row: ConversationRow) => words(conversationMenu(row, ctx(), convActions()))
  assert.ok(labels(thread({ unread: 3 })).includes('Mark read'))
  assert.ok(!labels(thread({ unread: 3 })).includes('Mark unread'))
  assert.ok(labels(thread({ unread: 0 })).includes('Mark unread'))
  // The open thread advances its own cursor as it is read: no Mark unread.
  assert.ok(!labels(thread({ unread: 0, active: true })).includes('Mark unread'))
  assert.ok(!labels(thread({ unread: 0, active: true })).includes('Mark read'))
})

test('pin follows the pinned state, and is not offered signed out', () => {
  assert.ok(words(conversationMenu(thread(), ctx({ pinned: true }), convActions())).includes('Unpin'))
  const out = words(conversationMenu(thread(), ctx({ signedIn: false }), convActions()))
  assert.ok(!out.includes('Pin') && !out.includes('Unpin'))
})

test('Talaria Desktop says where a new window goes', () => {
  assert.ok(words(conversationMenu(thread(), ctx({ desktop: true }), convActions())).includes('Open in browser'))
})

test("each item runs its own action, with the row's deep link", () => {
  const a = convActions()
  const m = conversationMenu(thread({ id: 't9', model: 'atlas' }), ctx(), a)
  item(m, 'Open').onSelect?.()
  item(m, 'Copy link').onSelect?.()
  item(m, 'Open in new tab').onSelect?.()
  item(m, 'Mark unread').onSelect?.()
  item(m, 'Pin').onSelect?.()
  item(m, 'Rename…').onSelect?.()
  item(m, 'Archive').onSelect?.()
  item(m, 'Delete…').onSelect?.()
  assert.equal(vi.mocked(a.open).mock.calls.length, 1)
  assert.deepEqual(vi.mocked(a.copyLink).mock.calls, [['/comms/agent/atlas/t9']])
  assert.deepEqual(vi.mocked(a.openElsewhere).mock.calls, [['/comms/agent/atlas/t9']])
  for (const f of [a.markUnread, a.togglePin, a.rename, a.archive, a.remove]) assert.equal(vi.mocked(f).mock.calls.length, 1)
  const c = conversationMenu(channel({ id: 'c7' }), ctx(), a)
  item(c, 'Copy link').onSelect?.()
  assert.deepEqual(vi.mocked(a.copyLink).mock.calls[1], ['/comms/channel/c7'])
})

test("a channel's owner gets notifications with its level checked, rename, archive and delete", () => {
  assert.deepEqual(words(conversationMenu(channel(), ctx(), convActions())), [
    'Open',
    'Copy link',
    'Open in new tab',
    '—',
    'Mark unread',
    'Pin',
    [
      'Notifications ▸',
      [
        'All messages',
        'Mentions only',
        'Nothing',
        '—',
        'Mute for 1 hour',
        'Mute for 8 hours',
        'Mute for 24 hours',
        'Mute until I turn it back on',
      ],
    ],
    'Rename…',
    '—',
    'Archive',
    'Delete…',
  ])
  const notif = item(conversationMenu(channel(), ctx(), convActions()), 'Notifications')
  assert.equal(notif.hint, 'Mentions only')
  const checked = (notif.children ?? []).filter((e): e is ContextMenuItem => e !== 'sep' && e.checked === true)
  assert.deepEqual(
    checked.map((e) => e.label),
    ['Mentions only'],
  )
})

test('levels and timed mutes write through their actions; a running mute adds Unmute', () => {
  const a = convActions()
  const muted = channel({ notify: { level: 'all', mutedUntil: '2026-11-04T13:00:00.000Z', settable: true } })
  const notif = item(conversationMenu(muted, ctx(), a), 'Notifications')
  assert.match(notif.hint ?? '', /^Muted until /)
  const kids = notif.children ?? []
  item(kids, 'Nothing').onSelect?.()
  item(kids, 'Mute for 1 hour').onSelect?.()
  item(kids, 'Unmute').onSelect?.()
  assert.deepEqual(vi.mocked(a.setLevel).mock.calls, [['none']])
  assert.deepEqual(vi.mocked(a.mute).mock.calls, [['2026-11-04T13:00:00.000Z'], [null]])
})

test("a member of a channel gets no rename, archive or delete; a room with no level of theirs no Notifications", () => {
  const member = words(conversationMenu(channel({ owner: false }), ctx(), convActions()))
  assert.ok(!member.includes('Rename…') && !member.includes('Archive') && !member.includes('Delete…'))
  const roster = words(conversationMenu(channel({ notify: { level: 'mentions', mutedUntil: null, settable: false } }), ctx(), convActions()))
  assert.ok(!roster.some((w) => Array.isArray(w)))
  const old = words(conversationMenu(channel({ notify: null }), ctx(), convActions()))
  assert.ok(!old.some((w) => Array.isArray(w)))
})

test('a DM can be tuned and pinned but never renamed, archived or deleted, even by its owner', () => {
  const a = { ...convActions(), viewProfile: vi.fn() }
  const dm = channel({ channelKind: 'dm', notify: { level: 'all', mutedUntil: null, settable: true } })
  assert.deepEqual(
    words(conversationMenu(dm, ctx(), a)).map((w) => (Array.isArray(w) ? w[0] : w)),
    ['Open', 'Copy link', 'Open in new tab', 'View profile', '—', 'Mark unread', 'Pin', 'Notifications ▸'],
  )
  const group = words(conversationMenu(channel({ channelKind: 'dm', groupDm: true }), ctx(), convActions()))
  assert.ok(!group.includes('Rename…') && !group.includes('Delete…'))
})

test('a relay follows the channel rules', () => {
  const relay = words(conversationMenu(channel({ channelKind: 'group' }), ctx(), convActions()))
  assert.ok(relay.includes('Rename…') && relay.includes('Archive') && relay.includes('Delete…'))
})

test("an agent's menu: new conversation first, then its link, profile, settings for a manager, pin and mute", () => {
  const row = { kind: 'agent' as const, model: 'dev', canManage: true, mutedUntil: null }
  assert.deepEqual(words(agentMenu(row, ctx(), agentActions())), [
    'New conversation',
    'Copy link',
    'View profile',
    'Agent settings',
    '—',
    'Pin',
    ['Mute DMs ▸', ['Mute for 1 hour', 'Mute for 8 hours', 'Mute for 24 hours', 'Mute until I turn it back on']],
  ])
  // Not a manager: no settings.
  assert.ok(!words(agentMenu({ ...row, canManage: false }, ctx(), agentActions())).includes('Agent settings'))
  // Signed out: nothing of their own to file or mute.
  assert.deepEqual(words(agentMenu({ ...row, canManage: false }, ctx({ signedIn: false }), agentActions())), [
    'New conversation',
    'Copy link',
    'View profile',
  ])
})

test("an agent's mute writes through, and a running one reads as muted with Unmute", () => {
  const a = agentActions()
  const m = agentMenu({ kind: 'agent', model: 'dev', canManage: false, mutedUntil: '9999-12-31T23:59:59.999Z' }, ctx({ pinned: true }), a)
  assert.ok(words(m).includes('Unpin'))
  const mute = item(m, 'DMs muted')
  assert.equal(mute.hint, 'Muted until you unmute')
  item(mute.children ?? [], 'Mute for 24 hours').onSelect?.()
  item(mute.children ?? [], 'Unmute').onSelect?.()
  assert.deepEqual(vi.mocked(a.muteDms).mock.calls, [['2026-11-05T12:00:00.000Z'], [null]])
  item(m, 'Copy link').onSelect?.()
  assert.deepEqual(vi.mocked(a.copyLink).mock.calls, [['/comms/agent/dev']])
  item(m, 'New conversation').onSelect?.()
  assert.equal(vi.mocked(a.newConversation).mock.calls.length, 1)
})

test('a person with no DM yet can only be messaged or looked up', () => {
  assert.deepEqual(words(personMenu({ message: vi.fn(), viewProfile: vi.fn() })), ['Message', 'View profile'])
})

test('a click on an agent opens a blank conversation by default, never a thread it would have to guess', () => {
  assert.equal(AGENT_CLICK, 'blank')
  assert.deepEqual(agentClickTarget([]), { t: 'new' })
  assert.deepEqual(agentClickTarget([{ id: 'newest' }, { id: 'older' }]), { t: 'new' })
  // The flip the PR names: 'recent' lands on the newest thread, and still a
  // blank one when there is none.
  assert.deepEqual(agentClickTarget([{ id: 'newest' }, { id: 'older' }], 'recent'), { t: 'thread', id: 'newest' })
  assert.deepEqual(agentClickTarget([], 'recent'), { t: 'new' })
})
