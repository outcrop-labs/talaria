// What a live work session is doing, in words a person reads at a glance.
//
// The server owns every fact here (`activity` and `spend` on
// /api/tasks/{id}/work-session and /api/boards/{id}/work-sessions, built in
// talaria-routes-boards' work_status.rs from the run row, the watch tail and
// the ledger). The browser only turns them into labels and ticks the clocks:
// times arrive as server epoch ms, and "for how long" is worked out against
// the server's clock (`now` on the same answer), so a refresh, a second tab
// or a slow poll all read the same.
import { formatTokens, formatUsd } from '@/lib/format'
import type { ChipTone } from '@/components/ui/chip'

export type PulseState = 'running' | 'thinking' | 'quiet' | 'sending'

export interface PulseStep {
  tool: string
  detail: string
  since: number | null
}

export interface WorkActivity {
  turn: number | null
  turnAt: number | null
  lastAt: number | null
  step: PulseStep | null
  outer: PulseStep | null
  last: { tool: string; detail: string; ok: boolean; at: number | null; ms: number | null } | null
  output: string | null
  said: string | null
  steps: number
  failures: number
  failure: { tool: string; text: string; at: number | null } | null
  partial: boolean
  accepted: boolean
  state: PulseState
}

export interface WorkSpend {
  calls: number
  tokens: number
  cost: number | null
  unpriced: number
  missing: { calls: number; reason: 'untagged' | 'shared-key' } | null
}

/** "Doug" from `doug-engineering` — the name the board already uses. */
export function agentName(model: string | null | undefined): string {
  const head = (model ?? '').split('-')[0] ?? ''
  return head ? head[0]!.toUpperCase() + head.slice(1) : 'The agent'
}

/** A running clock: "42s", "3m 05s", "1h 04m". */
export function elapsed(ms: number): string {
  const s = Math.max(0, Math.floor(ms / 1000))
  if (s < 60) return `${s}s`
  const m = Math.floor(s / 60)
  if (m < 60) return `${m}m ${String(s % 60).padStart(2, '0')}s`
  return `${Math.floor(m / 60)}h ${String(m % 60).padStart(2, '0')}m`
}

/** Milliseconds since a server-clock instant, read on the client's clock.
 *  `skew` is the server's `now` minus when the answer arrived. */
export function since(at: number | null | undefined, clientNow: number, skew: number): number | null {
  return at == null ? null : clientNow + skew - at
}

const TOOL_WORD: Record<string, string> = {
  terminal: 'shell',
  execute_code: 'python',
  process_manage: 'waiting on a process',
  mcp__workbench__start_job: 'start_job',
  mcp__workbench__prepare_env: 'prepare_env',
  mcp__workbench__finish_job: 'finish_job',
}

/** A tool as a person reads it: `shell`, `python`, `get_ticket`. */
export function toolWord(tool: string): string {
  return TOOL_WORD[tool] ?? tool.replace(/^mcp__[a-z0-9_-]+?__/, '')
}

/** One step on one line: `shell · cargo test -p talaria-notify`. */
export function stepLine(step: { tool: string; detail: string } | null | undefined): string {
  if (!step) return ''
  const detail = step.detail.replace(/\s+/g, ' ').trim()
  return detail ? `${toolWord(step.tool)} · ${detail}` : toolWord(step.tool)
}

/** The session's state as a chip: the word and its tone. `quietFor` is how
 *  long nothing has arrived, for the quiet state's label. */
export function stateChip(
  state: PulseState,
  agent: string,
  quietFor: number | null,
): { label: string; tone: ChipTone; title: string } {
  switch (state) {
    case 'running':
      return { label: 'running', tone: 'accent', title: `${agent} is running a step` }
    case 'thinking':
      return { label: 'thinking', tone: 'accent', title: `${agent} took the turn and is deciding its next step` }
    case 'quiet':
      return {
        label: quietFor != null ? `quiet ${elapsed(quietFor)}` : 'quiet',
        tone: 'warn',
        title: `${agent} is mid-turn and has sent nothing for a while`,
      }
    case 'sending':
      return {
        label: `waiting for ${agent}`,
        tone: 'warn',
        title: `The turn was sent and ${agent} has not taken it yet: still starting, at its concurrent-run cap, or unreachable`,
      }
  }
}

/** Tokens and cost on one line, or why there is nothing to show. */
export function spendLine(spend: WorkSpend | null | undefined): { text: string; title: string; muted: boolean } {
  if (!spend) return { text: '', title: '', muted: true }
  if (spend.calls > 0) {
    const cost = spend.cost == null || spend.unpriced === spend.calls ? 'unpriced' : formatUsd(spend.cost)
    return {
      text: `${formatTokens(spend.tokens)} tok · ${cost}`,
      title: `${spend.calls} model call${spend.calls === 1 ? '' : 's'} on this ticket since the session started${spend.unpriced ? ` (${spend.unpriced} unpriced)` : ''}`,
      muted: false,
    }
  }
  if (spend.missing?.reason === 'untagged') {
    return {
      text: 'not metered to this ticket',
      title: `This agent made ${spend.missing.calls} model call${spend.missing.calls === 1 ? '' : 's'} since the session started, booked to the agent but not to a ticket: its Hermes does not tag calls with the session it is working (Hermes v0.22 and later do).`,
      muted: true,
    }
  }
  if (spend.missing?.reason === 'shared-key') {
    return {
      text: 'not metered to this ticket',
      title: `${spend.missing.calls} model call${spend.missing.calls === 1 ? '' : 's'} since the session started went through the fleet's shared key, which names no agent. Roll the agent from Agents so it runs on its own key.`,
      muted: true,
    }
  }
  return { text: 'no model calls yet', title: 'Nothing has been booked to this ticket since the session started', muted: true }
}
