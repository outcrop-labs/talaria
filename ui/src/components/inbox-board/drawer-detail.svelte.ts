import { useAgents } from '@/lib/agents'
import { useBoardMembers, useBoardTasks, useTask } from '@/lib/boards.svelte'
import type { BriefEvidence } from '@/lib/daily-brief-types'
import { ownersOf, type InboxCard, type InboxPerson } from '@/lib/inbox-board'
import { personOf } from './ticket-assignees.svelte'

export interface DrawerTicketRow {
  id: string
  ref: string | null
  title: string
  status: string
  relation?: string
}

export interface DrawerStep {
  actor: string | null
  label: string
  at: string | null
}

export interface DrawerDetail {
  /** Who is on it; empty when the ticket has nobody, null while that is unknown. */
  readonly people: readonly InboxPerson[] | null
  readonly due: string | null
  readonly body: string | null
  readonly subtasks: DrawerTicketRow[]
  readonly activity: DrawerStep[]
  readonly evidence: BriefEvidence[]
  readonly linked: DrawerTicketRow[]
}

export function useDrawerDetail(card: () => InboxCard): DrawerDetail {
  const task = useTask(() => card().taskId)
  const boardId = (): string | null => task.data?.task.boardId ?? null
  const members = useBoardMembers(boardId)
  const boardTasks = useBoardTasks(boardId)
  const agents = useAgents()

  const ticket = $derived(task.data ?? null)

  const people = $derived<readonly InboxPerson[] | null>(
    ticket ? ticket.task.assignees.map((a) => personOf(a, agents.data?.agents ?? [], members.data ?? [])) : ownersOf(card()),
  )

  const activity = $derived<DrawerStep[]>(
    ticket
      ? [...ticket.activity]
          .sort((a, b) => Date.parse(a.createdAt) - Date.parse(b.createdAt))
          .slice(-12)
          .map((a) => ({ actor: a.actor, label: a.description, at: a.createdAt as string | null }))
      : card().steps.map((s) => ({ actor: null, label: s.label, at: s.at })),
  )

  const row = (t: { id: string; ticketRef?: string | null; title: string; status: string }, relation?: string): DrawerTicketRow => ({
    id: t.id,
    ref: t.ticketRef ?? null,
    title: t.title,
    status: t.status,
    relation,
  })

  return {
    get people() {
      return people
    },
    get due() {
      return ticket?.task.dueDate ?? null
    },
    get body() {
      return ticket?.task.description?.trim() || card().summary
    },
    get subtasks() {
      return ticket ? (boardTasks.data ?? []).filter((t) => t.parentId === ticket.task.id).map((t) => row(t)) : []
    },
    get activity() {
      return activity
    },
    get evidence() {
      const line = card().line
      return line && line.current.sourceType !== 'approval' ? line.current.evidence : []
    },
    get linked() {
      return ticket ? [...ticket.blockedBy.map((t) => row(t, 'Blocked by')), ...ticket.blocks.map((t) => row(t, 'Blocks'))] : []
    },
  }
}
