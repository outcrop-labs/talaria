// What a channel transcript feeds lib/message-groups: one GroupFacts per
// ChannelMessage. Pure and type-only on its imports (channel-view.ts pulls in
// runtime modules), so the wiring is tested, not just the rules.
//
// The main feed (ChannelView) reads the main-flow page only — thread replies
// never appear in it — so nothing there needs a `fresh` rule. The thread
// panel (ThreadPanel) lists the root first and its replies after the
// "N replies" rule, so the first reply always opens a block: the root stands
// alone even when its author replied straight after.

import type { GroupFacts } from '@/lib/message-groups'
import type { ChannelMessage } from '@/lib/channels.svelte'

type Row = Pick<ChannelMessage, 'authorType' | 'author' | 'createdAt'>

/** A main-feed row (channels, DMs, ticket discussions): grouped by author. */
export const channelGroupFacts = (m: Row): GroupFacts => ({ sender: `${m.authorType}:${m.author}`, at: m.createdAt })

/** A thread-panel row: index 0 is the root, so index 1 starts fresh. */
export const threadGroupFacts = (m: Row, index: number): GroupFacts => ({ ...channelGroupFacts(m), fresh: index === 1 })
