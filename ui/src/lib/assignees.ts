// Mixed-assignee display resolution: agent model ids render with their fleet
// label; `user:<uuid>` entries render with the member's name. One helper so
// kanban cards, list rows, pickers, and filters all agree.
import { isHumanAssignee, humanAssigneeId } from './task-const'
import type { BoardMember } from './boards.svelte'

export interface AssigneeInfo {
  /** The raw assignee string (stable key). */
  key: string
  label: string
  human: boolean
}

export function assigneeInfo(
  a: string,
  agents: Array<{ id: string; label: string }>,
  members: BoardMember[],
): AssigneeInfo {
  if (isHumanAssignee(a)) {
    const m = members.find((x) => x.userId === humanAssigneeId(a))
    return { key: a, label: m?.name ?? m?.email ?? 'teammate', human: true }
  }
  return { key: a, label: agents.find((x) => x.id === a)?.label ?? a, human: false }
}

export const userAssignee = (userId: string): string => `user:${userId}`
