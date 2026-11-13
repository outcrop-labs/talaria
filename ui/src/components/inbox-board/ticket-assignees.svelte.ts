import { createQueries } from '@tanstack/svelte-query'
import { assigneeInfo } from '@/lib/assignees'
import type { BoardMember } from '@/lib/boards.svelte'
import { getList } from '@/lib/fetch-json'
import type { InboxPerson } from '@/lib/inbox-board'
import type { Task } from '@/lib/task-const'
import type { HomeSummary } from '@/routes/app/home/home'

/** Every read's data under one key. `createQueries` hands back an array whose length is no
 *  signal, so a reader that saw it empty (the queues not yet read, so no boards) never sees
 *  the boards' answers; a combined object is one reactive field that does. */
const dataOf = <T>(results: ReadonlyArray<{ data: T | undefined }>): { data: Array<T | undefined> } => ({
  data: results.map((r) => r.data),
})

export function personOf(assignee: string, agents: Array<{ id: string; label: string }>, members: BoardMember[]): InboxPerson {
  const info = assigneeInfo(assignee, agents, members)
  return { name: info.label, agent: !info.human }
}

export function useTicketAssignees(
  queues: () => HomeSummary['queues'] | null,
  agents: () => Array<{ id: string; label: string }>,
): (taskId: string) => readonly InboxPerson[] | null {
  const boardIds = $derived([...new Set(Object.values(queues() ?? {}).flatMap((q) => q.items.map((t) => t.boardId)))])
  const tasks = createQueries(() => ({
    queries: boardIds.map((id) => ({
      queryKey: ['board-tasks', id, false],
      queryFn: (): Promise<Task[]> => getList<Task>(`/api/boards/${id}/tasks`, 'tasks'),
      staleTime: 30_000,
    })),
    combine: dataOf<Task[]>,
  }))
  const members = createQueries(() => ({
    queries: boardIds.map((id) => ({
      queryKey: ['board-members', id],
      queryFn: (): Promise<BoardMember[]> => getList<BoardMember>(`/api/boards/${id}/members`, 'members'),
      staleTime: 30_000,
    })),
    combine: dataOf<BoardMember[]>,
  }))
  const names = $derived(
    new Map(
      tasks.data.flatMap((list, i) =>
        (list ?? []).map((t) => [t.id, t.assignees.map((a) => personOf(a, agents(), members.data[i] ?? []))] as const),
      ),
    ),
  )
  return (taskId) => names.get(taskId) ?? null
}
