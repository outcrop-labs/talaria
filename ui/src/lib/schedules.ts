// Schedules on a conversation: put the agent you are already talking to on a
// clock, and read back what it is scheduled to do. The cron's output needs no
// client of its own — every run posts an ordinary agent turn into the same
// thread, so the conversation's own query already shows it.
//
// A SCHEDULE HAS TWO HALVES AND THEY FAIL SEPARATELY, which is the only thing
// about this wire shape worth knowing. The stored half (name, schedule, when,
// which job) is Talaria's row and is always there. The live half (`enabled`,
// `state`, `nextRunAt`, `lastRunAt`, `lastStatus`) is read from the agent's
// own scheduler (`/api/jobs` on its API server) at request time and is null
// whenever the agent cannot be asked — a rolling agent, a stopped one. A surface
// that read a null live half as "no schedule" would tell its owner their
// nightly digest had vanished every time their agent restarted, so
// `scheduleTone` below keeps "cannot ask" as its own answer.

import { resolve, type MaybeGetter } from '@/lib/reactive-arg'
import { createQuery } from '@tanstack/svelte-query'
import { delJson, getList, postJson } from '@/lib/fetch-json'

/** One schedule on a conversation: Talaria's stored row, with the agent's
 *  live view of the job merged on top when its container could be read. */
export interface ScheduleRow {
  jobId: string
  conversationId: string
  name: string
  schedule: string
  createdAt: string
  /** The live half — null, every field of it, when the agent could not be
   *  asked or no longer lists this job. */
  enabled: boolean | null
  state: string | null
  nextRunAt: string | null
  lastRunAt: string | null
  lastStatus: string | null
}

/** What a row's dot means. `unknown` is its own answer and not a shade of
 *  `error`: the schedule is stored, it will fire as soon as the agent is
 *  back, and nothing is wrong with it — we simply could not ask. */
export type ScheduleTone = 'ok' | 'paused' | 'error' | 'unknown'

export function scheduleTone(row: Pick<ScheduleRow, 'enabled' | 'state' | 'lastStatus'>): ScheduleTone {
  // `enabled` is the live half's own presence: the server fills all five of
  // its fields or none of them.
  if (row.enabled === null) return 'unknown'
  if (!row.enabled || row.state === 'paused') return 'paused'
  // The agent reports its last outcome in its own words ('error', 'failed',
  // 'Error: timed out'), so this matches rather than compares.
  if (row.lastStatus && /error|fail/i.test(row.lastStatus)) return 'error'
  return 'ok'
}

/** Schedule this conversation's agent. Throws with the server's sentence for
 *  the answers a person needs to read — 403 you do not manage this agent, and
 *  a 400 naming the agent as not running, or quoting Hermes on a schedule it
 *  will not take. Resolves with the new job's id. */
export async function scheduleFromConversation(
  conversationId: string,
  input: { name: string; schedule: string; prompt: string },
): Promise<string> {
  const { jobId } = await postJson<{ ok: true; jobId: string }>(`/api/conversations/${conversationId}/schedule`, input)
  return jobId
}

/** This conversation's schedules. No polling: a schedule changes only when
 *  somebody changes it, and its live half moves on the agent's own clock —
 *  the caller invalidates after a create. */
/** Take a schedule away — the job and this thread's listener row. */
export async function unscheduleFromConversation(conversationId: string, jobId: string): Promise<void> {
  await delJson(`/api/conversations/${encodeURIComponent(conversationId)}/schedules/${encodeURIComponent(jobId)}`)
}

export function useConversationSchedules(conversationId: MaybeGetter<string | null>) {
  return createQuery(() => {
    const id = resolve(conversationId)
    return {
      queryKey: ['conversation-schedules', id],
      enabled: !!id,
      queryFn: (): Promise<ScheduleRow[]> => getList<ScheduleRow>(`/api/conversations/${id}/schedules`, 'schedules'),
    }
  })
}
