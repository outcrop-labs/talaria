// The MCP gateway's per-call record, as the Observability MCP tab and the
// server cards read it (GET /api/mcp/calls and /api/mcp/calls/summary, both
// admin). The rows never carry a call's arguments or its result — only who
// called what, how it went, how long it took, and the failure's sentence.
import { createQuery } from '@tanstack/svelte-query'
import type { ChipTone } from '@/components/ui/chip'
import { getJson } from '@/lib/fetch-json'
import { resolve, type MaybeGetter } from '@/lib/reactive-arg'

export const OUTCOMES = ['ok', 'error', 'denied', 'not_connected', 'upstream_error', 'spend_error', 'cancelled'] as const
export type McpCallOutcome = (typeof OUTCOMES)[number]

/** Label + tint per outcome. Orange (danger) is a real failure; a refusal is
 *  attention (warn); a missing connection is setup and a hang-up is the
 *  client's choice — neither is the server failing (neutral). */
export const OUTCOME_META: Record<McpCallOutcome, { label: string; tone: ChipTone; blurb: string }> = {
  ok: { label: 'ok', tone: 'success', blurb: 'The tool answered.' },
  error: { label: 'error', tone: 'danger', blurb: 'The tool or server answered with an error.' },
  denied: { label: 'denied', tone: 'warn', blurb: 'Refused by the gateway: no access, or the tool is outside the allowed set.' },
  not_connected: { label: 'not connected', tone: 'neutral', blurb: 'Granted, but the account it would speak as is not connected.' },
  upstream_error: { label: 'upstream', tone: 'danger', blurb: 'The server could not be reached, or failed at HTTP level.' },
  spend_error: { label: 'spend', tone: 'danger', blurb: 'The credential spend boundary failed; nothing was sent.' },
  cancelled: { label: 'cancelled', tone: 'neutral', blurb: 'The caller hung up before the answer finished.' },
}

export const isOutcome = (v: string | null | undefined): v is McpCallOutcome =>
  !!v && (OUTCOMES as readonly string[]).includes(v)

export interface McpCall {
  id: number
  at: string
  serverId: string
  serverName: string
  agent: string
  userId: string | null
  userName: string | null
  method: string
  tool: string | null
  outcome: McpCallOutcome
  status: number | null
  latencyMs: number | null
  error: string | null
}

export interface McpCallCounts {
  calls: number
  errors: number
  denied: number
  notConnected: number
}

export interface McpServerCalls extends McpCallCounts {
  serverId: string
  serverName: string
  p50Ms: number | null
  p95Ms: number | null
  topTools: { tool: string; calls: number; errors: number }[]
}

export interface McpCallsSummary {
  since: string
  totals: McpCallCounts
  servers: McpServerCalls[]
  agents: { agent: string; calls: number; errors: number }[]
}

/** The windows the tab offers. */
export const WINDOWS = [
  { id: '1h', label: '1h', ms: 60 * 60_000 },
  { id: '24h', label: '24h', ms: 24 * 60 * 60_000 },
  { id: '7d', label: '7d', ms: 7 * 24 * 60 * 60_000 },
] as const
export type McpCallsWindow = (typeof WINDOWS)[number]['id']

export const isWindow = (v: string | null | undefined): v is McpCallsWindow => WINDOWS.some((w) => w.id === v)

/** The window's start in epoch ms, floored to the minute — so a poll inside
 *  the same minute asks the same question, and the query key stays put. */
export function sinceFor(window: McpCallsWindow, now: number = Date.now()): number {
  const ms = WINDOWS.find((w) => w.id === window)!.ms
  return Math.floor((now - ms) / 60_000) * 60_000
}

export interface McpCallsFilter {
  server?: string | null
  agent?: string | null
  tool?: string | null
  outcome?: McpCallOutcome | null
  since?: number | null
  limit?: number | null
}

/** The list URL for a filter; blank members are left out. */
export function callsUrl(f: McpCallsFilter): string {
  const q = new URLSearchParams()
  if (f.server) q.set('server', f.server)
  if (f.agent) q.set('agent', f.agent)
  if (f.tool) q.set('tool', f.tool)
  if (f.outcome) q.set('outcome', f.outcome)
  if (f.since != null) q.set('since', String(f.since))
  if (f.limit != null) q.set('limit', String(f.limit))
  const s = q.toString()
  return `/api/mcp/calls${s ? `?${s}` : ''}`
}

export function summaryUrl(since: number, server?: string | null): string {
  const q = new URLSearchParams({ since: String(since) })
  if (server) q.set('server', server)
  return `/api/mcp/calls/summary?${q.toString()}`
}

/** "12 ms", "1.4 s", "2m 05s"; an unmeasured call is a dash. */
export function formatLatency(ms: number | null | undefined): string {
  if (ms == null || !Number.isFinite(ms)) return '—'
  const r = Math.max(0, Math.round(ms))
  if (r < 1000) return `${r} ms`
  if (r < 60_000) return `${(r / 1000).toFixed(r < 10_000 ? 1 : 0)} s`
  const m = Math.floor(r / 60_000)
  const s = Math.floor((r % 60_000) / 1000)
  return `${m}m ${String(s).padStart(2, '0')}s`
}

const count = (n: number, one: string, many: string = `${one}s`) => `${n.toLocaleString('en-US')} ${n === 1 ? one : many}`

/** The server card's one line: "Last 24h: 142 calls · 3 errors · 1 denied".
 *  Calls and errors always (a zero error count is the reassuring part);
 *  refusals and missing connections only when there were some. */
export function cardLine(c: McpCallCounts | null | undefined, window = 'Last 24h'): string {
  if (!c || c.calls === 0) return `${window}: no calls`
  const parts = [count(c.calls, 'call'), count(c.errors, 'error')]
  if (c.denied > 0) parts.push(`${c.denied.toLocaleString('en-US')} denied`)
  if (c.notConnected > 0) parts.push(`${c.notConnected.toLocaleString('en-US')} not connected`)
  return `${window}: ${parts.join(' · ')}`
}

/** One server's counts out of a summary, by id or name (the card knows both;
 *  the deep link uses the name). Null when it had no calls in the window. */
export function serverCalls(s: McpCallsSummary | undefined, idOrName: string): McpServerCalls | null {
  return s?.servers.find((x) => x.serverId === idOrName || x.serverName === idOrName) ?? null
}

/** Errors as a share of calls — "2.1%", "0%"; a dash for no calls. */
export function errorRate(c: Pick<McpCallCounts, 'calls' | 'errors'>): string {
  if (c.calls === 0) return '—'
  const pct = (c.errors / c.calls) * 100
  if (pct === 0) return '0%'
  return pct < 10 ? `${pct.toFixed(1)}%` : `${Math.round(pct)}%`
}

/** Where a card's line leads: the tab, filtered to that server. */
export const panelHref = (serverName: string): string => `/observability/mcp?server=${encodeURIComponent(serverName)}`

/** The tab's filter: the window rides as its id and the start is computed per
 *  fetch, so a poll slides the window without changing the query key. */
export interface McpCallsView {
  server?: string | null
  agent?: string | null
  outcome?: McpCallOutcome | null
  window: McpCallsWindow
}

export const useMcpCalls = (view: MaybeGetter<McpCallsView>, enabled: MaybeGetter<boolean> = true) =>
  createQuery(() => {
    const v = resolve(view)
    return {
      queryKey: ['mcp-calls', v.window, v.server ?? null, v.agent ?? null, v.outcome ?? null],
      queryFn: async (): Promise<McpCall[]> =>
        (await getJson<{ calls: McpCall[] }>(callsUrl({ ...v, since: sinceFor(v.window), limit: 200 }))).calls,
      enabled: resolve(enabled),
      refetchInterval: 30_000,
      // A filter change keeps the last rows up (dimmed) instead of flashing
      // the skeleton between two answers.
      placeholderData: (prev: McpCall[] | undefined) => prev,
    }
  })

export const useMcpCallsSummary = (window: MaybeGetter<McpCallsWindow>, enabled: MaybeGetter<boolean> = true) =>
  createQuery(() => {
    const w = resolve(window)
    return {
      // The window, not its start: the start is computed per fetch, so a
      // background refetch slides the window instead of freezing it.
      queryKey: ['mcp-calls-summary', w],
      queryFn: (): Promise<McpCallsSummary> => getJson<McpCallsSummary>(summaryUrl(sinceFor(w))),
      enabled: resolve(enabled),
      refetchInterval: 60_000,
    }
  })

// ── Retention ────────────────────────────────────────────────────────────────

/** How long a row is kept, as GET/PUT /api/mcp/calls/retention speak it. */
export interface McpCallsRetention {
  days: number
}

export const RETENTION_URL = '/api/mcp/calls/retention'

/** The choices the tab offers. A stored value outside them (set by SQL, or a
 *  future choice) is offered too, so the select never misstates it. */
export const RETENTION_CHOICES = [7, 14, 30, 90, 180, 365] as const

export function retentionOptions(current: number | null | undefined): number[] {
  const all: number[] = [...RETENTION_CHOICES]
  if (current != null && Number.isInteger(current) && current > 0 && !all.includes(current)) all.push(current)
  return all.sort((a, b) => a - b)
}

/** "7 days", "1 day", "1 year". */
export function retentionLabel(days: number): string {
  if (days === 365) return '1 year'
  return days === 1 ? '1 day' : `${days.toLocaleString('en-US')} days`
}

export const useMcpCallsRetention = (enabled: MaybeGetter<boolean> = true) =>
  createQuery(() => ({
    queryKey: ['mcp-calls-retention'],
    queryFn: (): Promise<McpCallsRetention> => getJson<McpCallsRetention>(RETENTION_URL),
    enabled: resolve(enabled),
  }))
