// Shared types, hooks, and tables for the Home/Today suite (Home.svelte +
// the home/ tab components). Like the query hooks in lib/, the use* functions
// call createQuery and so must run during component init.
import { createQuery } from '@tanstack/svelte-query'
import { getJson, readJson } from '@/lib/fetch-json'
import type { DotStatus } from '@/components/ui/chip'

export const HOME_TABS = ['inbox', 'boards', 'comms', 'plans', 'research', 'docs'] as const
export type HomeTab = (typeof HOME_TABS)[number]

export interface WorkItem {
  id: string
  boardId: string
  board: string
  ticketRef: string | null
  title: string
  status: string
  updatedAt: string
}
export interface Queue {
  count: number
  items: WorkItem[]
}
export interface OrgActivity {
  at: string
  kind: string
  actor: string
  context: string
  detail: string
  href: string
}

export interface HomeSummary {
  org: {
    name: string
    activity: OrgActivity[]
    alerts: number | null
    costToday: { tokens: number; usd: number } | null
  }
  queues: { triage: Queue; review: Queue; blocked: Queue }
  unread: number
  boards: number
}

export const useHome = () =>
  createQuery(() => ({
    queryKey: ['home'],
    queryFn: (): Promise<HomeSummary> => getJson<HomeSummary>('/api/home'),
    refetchInterval: 30_000,
  }))

/** The whole query object, passed to tabs that need `isError`/`refetch` too. */
export type HomeQuery = ReturnType<typeof useHome>

// Time-aware and lightly varied: the pool is keyed by hour + day-of-year so
// the pick is stable across re-renders (no flicker) but changes through the
// day and from day to day.
export const greeting = (name?: string | null) => {
  const who = name?.split(' ')[0] ?? name ?? 'there'
  const now = new Date()
  const h = now.getHours()
  const pools: Array<[boolean, string[]]> = [
    [h < 5, [`Up late, ${who}?`, `Quiet hours, ${who}`, `Night shift, ${who}?`]],
    [h < 12, [`Morning, ${who}`, `Good morning, ${who}`, `Fresh start, ${who}`]],
    [h < 17, [`Afternoon, ${who}`, `Good afternoon, ${who}`, `Back at it, ${who}`]],
    [h < 22, [`Evening, ${who}`, `Good evening, ${who}`, `Winding down, ${who}?`]],
    [true, [`Late one, ${who}?`, `Still here, ${who}?`]],
  ]
  const pool = pools.find(([match]) => match)![1]
  const dayOfYear = Math.floor((now.getTime() - new Date(now.getFullYear(), 0, 0).getTime()) / 86_400_000)
  return pool[(dayOfYear + h) % pool.length]!
}

// ── Shared: an area-scoped slice of the workspace activity feed ─────────────
export interface FeedEvent {
  at: string
  actor: string
  context: string
  detail: string
  href: string
}

// ── Boards: board-scoped briefing, full queues behind a sidebar, audit trail ─
export type QueueKey = 'triage' | 'review' | 'blocked'
export const QUEUE_META: Record<QueueKey, { label: string; hint: string; dot: DotStatus }> = {
  triage: { label: 'To triage', hint: 'New tickets waiting to be assigned', dot: 'accent' },
  review: { label: 'To review', hint: 'Agent work awaiting your sign-off', dot: 'ok' },
  blocked: { label: 'Blocked', hint: 'Stalled, needs you to unblock', dot: 'warn' },
}

// ── Research: runs in flight and fresh reports ──────────────────────────────
export const RUN_DOT: Record<string, DotStatus> = {
  queued: 'warn',
  running: 'accent',
  done: 'ok',
  error: 'danger',
}

// ── The assistant's summary is the DAILY BRIEF, not a per-tab panel. Each
// console tab used to open with its own ephemeral briefing (`AssistantBriefing`);
// those are gone — the brief on the Inbox tab is the one summary a person is
// given, and asking about a tab's work happens from the brief's own chat.

export interface AgendaEvent {
  id: string
  summary: string
  start: string | null
  end: string | null
  allDay: boolean
  location: string | null
  htmlLink: string | null
}

export { groupAgendaByDay, formatAgendaTime } from '@/lib/agenda'

// The user's Google connection status — one cheap 200 up front (cache shared
// with Settings via the query key) so the agenda/mail panels never fire the
// data requests that answer 409 for the unconnected.
export const useGoogleStatus = () =>
  createQuery(() => ({
    queryKey: ['integration-google'],
    queryFn: async (): Promise<{ available: boolean; connected: boolean }> => {
      const r = await fetch('/api/integrations/google', { credentials: 'same-origin' })
      if (!r.ok) throw new Error('failed')
      return r.json()
    },
    retry: false,
  }))

// Fired only once the status says connected; both the panel and the
// quiet-inbox sentinel subscribe (same key → one request). `enabled` is a
// getter so the query switches on when the status resolves.
export const useAgenda = (enabled: () => boolean) =>
  createQuery(() => ({
    queryKey: ['agenda'],
    enabled: enabled(),
    queryFn: async (): Promise<{ events?: AgendaEvent[]; error?: string }> => {
      const r = await fetch('/api/integrations/google/calendar/events')
      // 409 (not connected) and 502 (Google hiccup) are ANSWERS this panel
      // knows how to render — it hides. Every other non-2xx is a failure, and
      // readJson turns it into an HttpError carrying the server's message
      // instead of a bare "failed".
      if (r.status === 409 || r.status === 502) return { error: 'unavailable' }
      return readJson<{ events?: AgendaEvent[] }>(r)
    },
    retry: false,
    refetchInterval: 5 * 60_000,
  }))

export interface Mail {
  id: string
  threadId: string
  from: string
  subject: string
  snippet: string
  date: string | null
  unread: boolean
}

// Fired only once the status says connected — same key-sharing pattern as
// useAgenda, so the unconnected never see the 409-answering request at all.
export const useGmail = (enabled: () => boolean) =>
  createQuery(() => ({
    queryKey: ['gmail'],
    enabled: enabled(),
    queryFn: async (): Promise<{ messages?: Mail[]; error?: string }> => {
      const r = await fetch('/api/integrations/google/gmail/messages')
      // Same contract as Agenda: 409 / 502 mean "nothing to show here".
      if (r.status === 409 || r.status === 502) return { error: 'unavailable' }
      return readJson<{ messages?: Mail[] }>(r)
    },
    retry: false,
    refetchInterval: 5 * 60_000,
  }))
