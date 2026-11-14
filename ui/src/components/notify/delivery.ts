// Where a kind of notification is delivered, in the words the settings page
// shows. PURE. The inbox always keeps everything, so it is never a choice
// here: these are the channels a person can add to it.

import type { MatrixChannel } from '@/lib/notify-matrix'
import type { Channels, ClassInfo } from '@/lib/notify-wire'

export const DELIVERY_CHANNELS: ReadonlyArray<{ id: MatrixChannel; label: string }> = [
  { id: 'desktop', label: 'Desktop' },
  { id: 'push', label: 'Push' },
  { id: 'email', label: 'Email' },
]

export const CHANNEL_INFO =
  'Desktop is a pop-up in Talaria, or a banner on this computer when you are looking elsewhere. ' +
  'Push is a banner from your browser with Talaria closed. ' +
  'Email is sent only when you have not seen it in the app first.'

export const EMAIL_OFF_REASON = 'Off for this workspace'
export const PUSH_UNREGISTERED_REASON = 'No browser registered'

/** The channels that deliver a kind right now: the ones switched on, less
 *  email while the workspace sends none. */
export function deliveredOn(channels: Pick<Channels, MatrixChannel>, emailEnabled: boolean): MatrixChannel[] {
  return DELIVERY_CHANNELS.map((c) => c.id).filter((id) => channels[id] && (id !== 'email' || emailEnabled))
}

/** "Desktop, push and email", "Desktop only", "Inbox only". */
export function deliverySummary(on: readonly MatrixChannel[]): string {
  const names = DELIVERY_CHANNELS.filter((c) => on.includes(c.id)).map((c) => c.label)
  if (names.length === 0) return 'Inbox only'
  if (names.length === 1) return `${names[0]} only`
  const words = names.map((n, i) => (i === 0 ? n : n.toLowerCase()))
  return `${words.slice(0, -1).join(', ')} and ${words.at(-1)}`
}

// The registry's blurbs are full sentences. A row has room for a few words,
// so a kind whose label leaves a question open gets a short line, and a kind
// whose label says it all gets none. A kind the server adds later is in
// neither table and shows the registry's own sentence.
const KIND_HINT: Readonly<Record<string, string>> = {
  judge_escalation: 'The quality gate handed work to a person.',
  mention: 'Mentions, assigned tickets and shared docs.',
  account: 'Changes to what can act as you.',
  work_complete: 'Tickets, research and designs you asked for.',
  channel_activity: 'Channels set to notify on every message.',
  gap_reported: 'An agent could not do something.',
}
const LABEL_IS_ENOUGH: ReadonlySet<string> = new Set([
  'approval_pending',
  'agent_blocked',
  'workchain_turn',
  'dm',
  'participating',
  'workchain_paused',
])

export function kindHint(cls: Pick<ClassInfo, 'id' | 'blurb'>): string | undefined {
  return LABEL_IS_ENOUGH.has(cls.id) ? undefined : (KIND_HINT[cls.id] ?? cls.blurb)
}
