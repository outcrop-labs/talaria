import { useAgents } from '@/lib/agents'
import { useArtifacts } from '@/lib/artifacts'
import { useClock } from '@/lib/clock.svelte'
import { useConversations } from '@/lib/conversations.svelte'
import type { BriefView } from '@/lib/daily-brief-types'
import { buildInboxCards, laneCards, type InboxCard, type InboxLane } from '@/lib/inbox-board'
import { useResearchRuns } from '@/lib/research'
import { useHome } from '@/routes/app/home/home'
import { useTicketAssignees } from './ticket-assignees.svelte'

export const ACTION = { size: 'xs', voice: 'sans' } as const
export const FILLED = 'h-7 gap-1.5 bg-fg px-2 text-surface hover:bg-fg/85 hover:brightness-100'
export const GHOST = 'h-7 gap-1 px-1.5 hover:bg-hover'
export const LINK = 'text-[13px] font-medium text-fg underline-offset-4'
export const LINK_HOVER = 'hover:underline focus-visible:underline'

export const DRAWER_PX = 520
export const BOARD_MIN_PX = 1100

export interface ListColumns {
  hasStatus: boolean
  hasTypes: boolean
  hasRefs: boolean
}

export const COLUMNS = 'hidden gap-x-3 pl-3 @[60rem]:flex'
export const COLUMN = {
  status: 'w-28 shrink-0 truncate',
  type: 'w-28 shrink-0 truncate',
  time: 'w-16 shrink-0 truncate',
  ref: 'w-[5.5rem] shrink-0 truncate',
  owner: 'w-24 shrink-0 truncate',
} as const

export const ROW_GRID = '@5xl:col-span-full @5xl:grid @5xl:grid-cols-subgrid'

export const SECTION = 'font-sans text-[16px] font-semibold leading-5 tracking-[-0.005em] text-fg'
export const SECTION_COUNT = 'text-[13px] font-normal tabular-nums text-muted'

export const LABEL = 'font-sans text-[13px] font-medium leading-5 text-fg'
export const BREAK = 'mt-5 border-t border-line pt-4 group-data-[full]/drawer:mt-6 group-data-[full]/drawer:pt-6'

export interface BoardActions {
  markItem(sourceKey: string, action: 'check' | 'dismiss' | 'restore'): Promise<boolean>
  decideApproval(id: string, decision: 'approve' | 'reject'): Promise<{ ok: true } | { ok: false; error: string }>
}

export interface TodayActions {
  markItem(sourceKey: string, action: 'check' | 'dismiss'): Promise<boolean>
  decideReply(draftId: string, decision: 'approve' | 'reject'): Promise<{ ok: boolean; error?: string }>
  setDelegated(channelId: string, granted: boolean): Promise<boolean>
  invalidate(): void
}

export interface InboxBoardState {
  readonly cards: InboxCard[]
  readonly lanes: Record<InboxLane, InboxCard[]>
  readonly pending: boolean
  readonly failure: { sources: string[]; error: unknown } | null
  readonly now: number
  retry(): void
}

export function useInboxBoard(brief: () => BriefView | null, briefSettled: () => boolean): InboxBoardState {
  const home = useHome()
  const sessions = useConversations('sessions')
  const runs = useResearchRuns()
  const artifacts = useArtifacts()
  const sources = [
    { name: 'tickets', query: home },
    { name: 'sessions', query: sessions },
    { name: 'research', query: runs },
    { name: 'files', query: artifacts },
  ]
  const agents = useAgents()
  const clock = useClock()
  const assignees = useTicketAssignees(() => home.data?.queues ?? null, () => agents.data?.agents ?? [])

  const minute = $derived(Math.floor(clock.now / 60_000))
  const agentLabels = $derived(new Map((agents.data?.agents ?? []).map((a) => [a.id, a.label])))

  const cards = $derived(
    buildInboxCards({
      now: minute * 60_000,
      brief: brief(),
      queues: home.data?.queues ?? null,
      sessions: sessions.data ?? null,
      runs: runs.data ?? null,
      artifacts: artifacts.data ?? null,
      agentName: (id) => agentLabels.get(id) ?? null,
      assignees,
    }),
  )
  const lanes = $derived(laneCards(cards))

  return {
    get cards() {
      return cards
    },
    get lanes() {
      return lanes
    },
    get pending() {
      return !briefSettled() || sources.some((s) => s.query.data === undefined && !s.query.isError)
    },
    get failure() {
      const failed = sources.filter((s) => s.query.isError && s.query.data === undefined)
      return failed.length > 0 ? { sources: failed.map((s) => s.name), error: failed[0]!.query.error } : null
    },
    get now() {
      return clock.now
    },
    retry() {
      for (const s of sources) if (s.query.isError) void s.query.refetch()
    },
  }
}
