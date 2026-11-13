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
// so the kinds this app knows get a short line. A kind the server adds later
// is not in this table and shows the registry's own sentence.
const KIND_HINT: Readonly<Record<string, string>> = {
  approval_pending: 'Work waits for your yes or no.',
  judge_escalation: 'The quality gate handed work to a person.',
  agent_blocked: 'An agent hit a problem and stopped.',
  workchain_turn: 'The next step is yours.',
  mention: 'Mentions, assigned tickets and shared docs.',
  dm: 'From a teammate or an agent.',
  participating: 'Replies where you took part.',
  account: 'Changes to what can act as you.',
  work_complete: 'Tickets, research and designs you asked for.',
  workchain_paused: 'A workchain you run is waiting.',
  channel_activity: 'Channels set to notify on every message.',
  gap_reported: 'An agent could not do something.',
}

export function kindHint(cls: Pick<ClassInfo, 'id' | 'blurb'>): string {
  return KIND_HINT[cls.id] ?? cls.blurb
}
