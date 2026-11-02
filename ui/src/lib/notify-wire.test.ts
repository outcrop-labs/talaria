// The contract's TS mirror against JSON captured from a running api: the
// slot-6 stack (api :5406) answered every value below during the contract's
// live acceptance (notify/rigs/w2-contract/w2-http.mjs), trimmed to one of
// each shape. `satisfies` makes svelte-check prove the types accept what the
// server sends; the runtime asserts pin the key ORDER (wave 1's keys first —
// the shell parses them) and the closed vocabularies.
import { describe, expect, it } from 'vitest'
import { ARRIVALS_KEY } from './notify-arrivals'
import {
  ARRIVALS_KEY as WIRE_ARRIVALS_KEY,
  ENTITY,
  FOREVER,
  GROUP,
  SUMMARY_TAG,
  type ArrivalRecordV2,
  type Category,
  type ClassInfo,
  type Channels,
  type Device,
  type DndWire,
  type NotificationRowV2,
  type NotificationsEventV2,
  type NotificationEventV2,
  type NotificationsPayloadV2Fields,
  type PresenceEvent,
  type PresenceSnapshot,
} from './notify-wire'
import type { UserEvent } from './user-events.svelte'

// A row a producer filed with an actor and an entity (no seq: inserted
// directly, as a pre-contract row would be).
const ROW = {
  "id": "b3acf0aa-7d84-49b8-abe8-f29ee230e4cb",
  "kind": "approval_pending",
  "title": "Approve the deploy?",
  "body": "Bob asked for approval",
  "href": "/approvals/x1",
  "readAt": null,
  "createdAt": "2026-11-01T03:41:25.666Z",
  "seq": null,
  "alert": "banner",
  "class": "approval_pending",
  "category": "needs_you",
  "count": 1,
  "updatedAt": "2026-11-01T03:41:25.666Z",
  "archivedAt": null,
  "actor": {
    "kind": "user",
    "id": "620e512c-cdba-47dd-aa69-3bec79d20efc",
    "name": "Bob Contract"
  },
  "entity": {
    "type": "approval",
    "id": "x1"
  },
  "reason": "Approval waiting"
} satisfies NotificationRowV2

// A row the single writer filed: three DMs coalesced in place (refresh), seq 3.
const SEQ_ROW = {
  "id": "ac360d7b-072b-4509-88ef-0b9fa800273c",
  "kind": "dm",
  "title": "Bob Contract sent you a message",
  "body": "dm three",
  "href": "/comms/channel/b96833f5-713f-4e6c-9bd2-36dd88cea10d",
  "readAt": null,
  "createdAt": "2026-11-01T03:41:25.273Z",
  "seq": 3,
  "alert": "banner",
  "class": "dm",
  "category": "direct",
  "count": 3,
  "updatedAt": "2026-11-01T03:41:25.273Z",
  "archivedAt": null,
  "actor": null,
  "entity": null,
  "reason": "Direct message"
} satisfies NotificationRowV2

const TOP = {
  "unread": 2,
  "unreadForYou": 2,
  "seq": 3,
  "emailFrequency": "1h",
  "features": [
    "seq",
    "classes",
    "channels",
    "dnd-store",
    "presence-store",
    "ack",
    "devices",
    "read-ids",
    "channel-levels-store"
  ],
  "next": null
} satisfies NotificationsPayloadV2Fields & { unread: number }

const CLASS = {
  "id": "mention",
  "label": "Mentions & shares",
  "blurb": "Someone @mentioned you, assigned you a ticket, or shared a doc with you.",
  "category": "direct",
  "defaults": {
    "desktop": true,
    "push": true,
    "email": true
  },
  "breaksThrough": false,
  "adminOnly": false,
  "emailDelayMin": 0,
  "kinds": [
    {
      "id": "mention",
      "reason": "Mentioned you"
    },
    {
      "id": "kb-comment",
      "reason": "Commented on your doc"
    },
    {
      "id": "task-assigned",
      "reason": "Assigned to you"
    },
    {
      "id": "plan-share",
      "reason": "Shared a plan"
    },
    {
      "id": "research-share",
      "reason": "Shared research"
    }
  ]
} satisfies ClassInfo

const CHANNELS = {
  "inApp": true,
  "desktop": true,
  "push": true,
  "email": false
} satisfies Channels

const DND = {
  "pausedUntil": null,
  "schedule": {
    "enabled": true,
    "days": [
      0,
      1,
      2,
      3,
      4,
      5,
      6
    ],
    "start": "01:00",
    "end": "02:00"
  },
  "allowNeedsYou": true,
  "active": true,
  "until": "2026-11-01T05:00:00.000Z",
  "reason": "schedule",
  "timeZone": "America/New_York"
} satisfies DndWire

const NOTIFICATION_FRAME = {
  "type": "notification",
  "notificationId": "ac360d7b-072b-4509-88ef-0b9fa800273c",
  "seq": 1
} satisfies NotificationEventV2

const READ_FRAME = {
  "type": "notifications",
  "unread": 3,
  "unreadForYou": 3,
  "readIds": [
    "e05fa9ce-a19f-48c1-a550-61c7c70314b3",
    "f9bc6fc9-15a8-408a-bc7a-cc6253cf78a7"
  ],
  "all": false
} satisfies NotificationsEventV2

const PRESENCE_FRAME = {
  "type": "presence",
  "looking": "tab-w2cAAAA1",
  "active": null,
  "clients": 1
} satisfies PresenceEvent

const PRESENCE = {
  "looking": "tab-w2cAAAA1",
  "active": null,
  "clients": [
    {
      "clientId": "tab-w2cAAAA1",
      "kind": "web",
      "state": "looking",
      "at": "2026-11-01T03:41:30.235Z"
    },
    {
      "clientId": "tab-w2cBBBB2",
      "kind": "web",
      "state": "idle",
      "at": "2026-11-01T03:41:29.465Z"
    }
  ]
} satisfies PresenceSnapshot

const DEVICES = [
  {
    "id": "1a55f0a0-e82a-4767-8268-fea495c652f2",
    "label": "Firefox on Linux",
    "createdAt": "2026-11-01T03:41:31.384Z",
    "lastSeenAt": "2026-11-01T03:41:31.384Z",
    "lastAttemptAt": "2026-11-01T03:41:41.619Z",
    "lastStatus": 500,
    "lastError": "500 alert(1) x{} [31moops oops oops oops oops oops oops oops oops oops oops oops oops oops oops oops oops oops oops oop…",
    "failures": 1,
    "thisSession": false
  },
  {
    "id": "b46ea8e7-de46-4d46-9bb2-4c7460da4371",
    "label": "Chrome on Linux",
    "createdAt": "2026-11-01T03:41:31.373Z",
    "lastSeenAt": "2026-11-01T03:41:31.609Z",
    "lastAttemptAt": "2026-11-01T03:41:31.609Z",
    "lastStatus": 201,
    "lastError": null,
    "failures": 0,
    "thisSession": true
  }
] satisfies Device[]

const WAVE1_ROW_KEYS = ['id', 'kind', 'title', 'body', 'href', 'readAt', 'createdAt']
const V2_ROW_KEYS = ['seq', 'alert', 'class', 'category', 'count', 'updatedAt', 'archivedAt', 'actor', 'entity', 'reason']
const CATEGORIES: Category[] = ['needs_you', 'direct', 'following', 'workspace']

describe('notify-wire against the captured api', () => {
  it("keeps wave 1's row keys first and in order, then the v2 keys", () => {
    for (const row of [ROW, SEQ_ROW]) expect(Object.keys(row)).toEqual([...WAVE1_ROW_KEYS, ...V2_ROW_KEYS])
    expect(ROW.seq).toBeNull()
    expect(SEQ_ROW.seq).toBe(3)
    expect(SEQ_ROW.count).toBe(3)
    expect(['banner', 'feed']).toContain(ROW.alert)
    expect(CATEGORIES).toContain(ROW.category)
    expect(ROW.actor).toEqual({ kind: 'user', id: expect.any(String), name: 'Bob Contract' })
    expect(ROW.entity).toEqual({ type: ENTITY.approval, id: 'x1' })
  })

  it('serves the top-level v2 fields, the seq equal to the newest row seq', () => {
    expect(TOP.seq).toBe(SEQ_ROW.seq)
    expect(TOP.features).toContain('seq')
    expect(TOP.next).toBeNull()
    expect(Object.keys(CLASS)).toEqual(['id', 'label', 'blurb', 'category', 'defaults', 'breaksThrough', 'adminOnly', 'emailDelayMin', 'kinds'])
    expect(CHANNELS.inApp).toBe(true)
    expect(Object.keys(DND)).toEqual(['pausedUntil', 'schedule', 'allowNeedsYou', 'active', 'until', 'reason', 'timeZone'])
  })

  it('types every new frame as a UserEvent member', () => {
    const frames: UserEvent[] = [NOTIFICATION_FRAME, READ_FRAME, PRESENCE_FRAME, { type: 'resync', seq: 3, complete: true }, { type: 'session', state: 'ended' }]
    expect(frames.map((f) => f.type)).toEqual(['notification', 'notifications', 'presence', 'resync', 'session'])
    expect(READ_FRAME.readIds).toHaveLength(2)
    expect(READ_FRAME.all).toBe(false)
    expect(PRESENCE_FRAME.looking).toBe(PRESENCE.looking)
  })

  it('names devices and filters their errors', () => {
    expect(DEVICES.map((d) => d.label).sort()).toEqual(['Chrome on Linux', 'Firefox on Linux'])
    expect(DEVICES.filter((d) => d.thisSession)).toHaveLength(1)
    for (const d of DEVICES) {
      if (d.lastError === null) continue
      expect([...d.lastError].length).toBeLessThanOrEqual(120)
      expect(d.lastError).not.toMatch(/[<>\u0000-\u001f]/)
    }
  })

  it('pins the shared names to the server spellings', () => {
    expect(WIRE_ARRIVALS_KEY).toBe(ARRIVALS_KEY)
    expect(SUMMARY_TAG).toBe('talaria-summary')
    expect(FOREVER).toBe('9999-12-31T23:59:59.999Z')
    expect(GROUP.dm('c1')).toBe('dm:c1')
    expect(GROUP.taskStatus('t1')).toBe('task:t1:status')
    expect(GROUP.channelThread('m1')).toBe('reply:m1')
    expect(GROUP.outreach('margaret', '2026-10-31')).toBe('outreach:margaret:2026-10-31')
    expect(Object.values(ENTITY)).toEqual(['task', 'channel', 'conversation', 'approval', 'mcp_server', 'plan', 'research_run', 'kb_doc'])
    const v2: ArrivalRecordV2 = { hw: SEQ_ROW.createdAt, seen: { [SEQ_ROW.id]: SEQ_ROW.createdAt }, at: 0, v: 2, seq: TOP.seq }
    expect(v2.seq).toBe(3)
  })
})
