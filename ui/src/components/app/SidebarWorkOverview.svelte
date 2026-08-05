<script lang="ts" module>
  import type { Task } from '@/lib/task-const'

  interface IndexedTask extends Task {
    boardName: string
  }

  const DONE_STATUSES = new Set(['done', 'completed', 'cancelled'])

  const isDone = (task: Task) => task.completedAt !== null || DONE_STATUSES.has(task.status)
</script>

<script lang="ts">
  import { createQueries } from '@tanstack/svelte-query'
  import { FolderKanban, ListTodo, Search, X } from '@lucide/svelte'
  import type { LucideIcon as IconType } from '@lucide/svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import { listQuery } from '@/components/ui/query-state'
  import { cn } from '@/lib/cn'
  import { getList } from '@/lib/fetch-json'
  import { useBoards, type Board } from '@/lib/boards.svelte'
  import { p } from '@/router'

  let query = $state('')
  // The rows AND the sentence that says they are missing. This sidebar states
  // counts ("2/7 projects"); a board read that 500s must never arrive here as
  // an empty list, or the rail confidently reports that the org has no work.
  const boardsQuery = useBoards()
  const boardList = listQuery(boardsQuery, {
    title: 'Could not load your projects',
    staleTitle: 'These projects may be out of date',
    variant: 'inline',
  })
  const boards = $derived(boardList.rows)
  // Same key and same reader as `useBoardTasks` on purpose: two queryFns under
  // one key hand each other's consumers the other's row shape. The board name
  // is joined below, out of the cached payload, so what lands in the cache is
  // exactly what the board views expect.
  const taskQueries = createQueries(() => ({
    queries: boardList.rows.map((board) => ({
      queryKey: ['board-tasks', board.id, false],
      queryFn: (): Promise<Task[]> => getList<Task>(`/api/boards/${board.id}/tasks`, 'tasks'),
      staleTime: 30_000,
    })),
  }))

  const tasks: IndexedTask[] = $derived(
    taskQueries.flatMap((result, index) =>
      (result.data ?? []).map((task) => ({ ...task, boardName: boards[index]?.name ?? '' })),
    ),
  )
  const tasksPending = $derived(taskQueries.some((result) => result.isPending && result.fetchStatus !== 'idle'))
  const tasksFailed = $derived(taskQueries.filter((result) => result.isError && result.data === undefined))
  // Nothing came back at all: the counts below would be zeros invented by an
  // outage, so they are withheld rather than stated.
  const tasksUnknown = $derived(taskQueries.length > 0 && tasksFailed.length === taskQueries.length)
  const retryTasks = () => {
    for (const result of taskQueries) if (result.isError) void result.refetch()
  }
  const loading = $derived(boardList.pending || tasksPending)
  const activeProjects = $derived(
    boards.filter((board) => tasks.some((task) => task.boardId === board.id && !isDone(task))).length,
  )
  const completedTasks = $derived(tasks.filter(isDone).length)
  const normalizedQuery = $derived(query.trim().toLowerCase())

  const projectMatches = $derived(
    normalizedQuery
      ? boards.filter((board) => board.name.toLowerCase().includes(normalizedQuery)).slice(0, 3)
      : [],
  )
  const taskMatches = $derived(
    normalizedQuery
      ? tasks
          .filter(
            (task) =>
              task.title.toLowerCase().includes(normalizedQuery) || task.boardName.toLowerCase().includes(normalizedQuery),
          )
          .slice(0, Math.max(0, 6 - projectMatches.length))
      : [],
  )
</script>

{#snippet segmentMeter(value: number, total: number, segments: number)}
  {@const filled = total > 0 ? Math.max(value > 0 ? 1 : 0, Math.round((value / total) * segments)) : 0}
  <span class="flex h-4 shrink-0 items-center gap-[2px]" aria-hidden="true">
    {#each Array.from({ length: segments }) as _, index (index)}
      <span
        class={cn(
          'h-3 w-[3px] rounded-[1px] bg-line-strong transition-colors',
          index < filled && 'bg-[#9d87ff] shadow-[0_0_5px_rgba(157,135,255,0.22)]',
        )}
      ></span>
    {/each}
  </span>
{/snippet}

{#snippet summaryRow(row: {
  to: 'projects' | 'tasks'
  icon: IconType
  label: string
  value: number
  total: number
  segments: number
  loading: boolean
  /** The read failed: state nothing rather than a number the outage produced. */
  unknown?: boolean
  hint: string
})}
  {@const unknown = row.unknown ?? false}
  <a
    href={row.to === 'projects' ? p('/boards') : '/?tab=boards'}
    title={row.hint}
    class="flex h-8 w-full items-center gap-2 rounded-md px-2 text-left transition-colors duration-[120ms] hover:bg-hover"
  >
    <span class="grid h-4 w-4 shrink-0 place-items-center text-muted"><row.icon size={16} strokeWidth={1.5} /></span>
    <span class="min-w-0 flex-1 truncate font-sans text-[13px] text-fg">{row.label}</span>
    {@render segmentMeter(unknown ? 0 : row.value, unknown ? 0 : row.total, row.segments)}
    <span class="w-[34px] shrink-0 text-right font-mono text-[10px] tracking-[0.02em] text-muted">
      {row.loading || unknown ? '—' : `${row.value}/${row.total}`}
    </span>
  </a>
{/snippet}

{#snippet searchResults(projects: Board[], taskRows: IndexedTask[])}
  {#if loading}
    <div class="px-2 pt-3 font-mono text-[9px] uppercase tracking-[0.08em] text-muted">Searching…</div>
  {:else if projects.length === 0 && taskRows.length === 0}
    <div class="px-2 pt-3 font-sans text-[11px] leading-4 text-muted">
      No projects or tasks match “{query.trim()}”.
    </div>
  {:else}
    <div class="mt-2 max-h-52 space-y-0.5 overflow-y-auto">
      {#each projects as board (board.id)}
        <a
          href={p('/boards/:boardId', { params: { boardId: board.id } })}
          onclick={() => (query = '')}
          class="flex min-h-8 items-center gap-2 rounded-md px-2 py-1 text-muted transition-colors hover:bg-hover hover:text-fg"
        >
          <FolderKanban size={14} strokeWidth={1.5} class="shrink-0" />
          <span class="min-w-0 flex-1 truncate font-sans text-[12px]">{board.name}</span>
          <span class="font-mono text-[9px] uppercase tracking-[0.06em] text-ink-dim">Project</span>
        </a>
      {/each}
      {#each taskRows as task (task.id)}
        <a
          href={p('/boards/:boardId/:taskId', { params: { boardId: task.boardId, taskId: task.id } })}
          onclick={() => (query = '')}
          class="flex min-h-8 items-start gap-2 rounded-md px-2 py-1.5 text-muted transition-colors hover:bg-hover hover:text-fg"
        >
          <ListTodo size={14} strokeWidth={1.5} class="mt-px shrink-0" />
          <span class="min-w-0 flex-1">
            <span class="block truncate font-sans text-[12px] text-fg">{task.title}</span>
            <span class="block truncate font-mono text-[9px] uppercase tracking-[0.05em] text-ink-dim">
              {task.boardName}
            </span>
          </span>
        </a>
      {/each}
    </div>
  {/if}
{/snippet}

<section aria-label="Project and task overview" class="mt-5 shrink-0 border-b border-line-subtle pb-4">
  <div class="relative">
    <Search
      size={14}
      strokeWidth={1.7}
      class="pointer-events-none absolute left-3 top-1/2 -translate-y-1/2 text-muted"
      aria-hidden="true"
    />
    <input
      bind:value={query}
      onkeydown={(event) => {
        if (event.key === 'Escape') query = ''
      }}
      aria-label="Search projects and tasks"
      placeholder="Search projects & tasks"
      class="h-9 w-full rounded-lg border border-line bg-raised pl-9 pr-8 font-sans text-xs text-fg outline-none transition-colors placeholder:text-muted hover:border-line-strong focus:border-[color:var(--theme-accent-border)] focus:ring-1 focus:ring-[color:var(--theme-accent-border)]"
    />
    {#if query}
      <button
        type="button"
        onclick={() => (query = '')}
        aria-label="Clear search"
        class="absolute right-2 top-1/2 grid h-5 w-5 -translate-y-1/2 place-items-center rounded text-muted transition-colors hover:bg-hover hover:text-fg"
      >
        <X size={12} strokeWidth={1.7} />
      </button>
    {/if}
  </div>

  {#if boardList.failed}
    <!-- The read broke and there is nothing to show. The notice replaces the
         rows — an empty list beside an error reads as "and also there are
         none", which is the lie twice. -->
    <div class="mt-3 px-2">
      {#if boardList.notice}<QueryError {...boardList.notice} />{/if}
    </div>
  {:else}
    {#if boardList.notice}
      <div class="mt-3 px-2"><QueryError {...boardList.notice} /></div>
    {/if}
    {#if tasksFailed.length > 0}
      <QueryError
        error={tasksFailed[0]?.error}
        title={tasksUnknown ? 'Could not load your tasks' : 'Some task counts are missing'}
        variant="inline"
        onRetry={retryTasks}
        class="mt-3 px-2"
      />
    {/if}
    {#if normalizedQuery}
      {@render searchResults(projectMatches, taskMatches)}
    {:else}
      <div class="mt-3 space-y-0.5">
        {@render summaryRow({
          to: 'projects',
          icon: FolderKanban,
          label: 'Projects',
          value: activeProjects,
          total: boards.length,
          segments: 3,
          loading,
          // "Active" is derived from the tasks on each board, so with the
          // task reads down the numerator is unknown, not zero.
          unknown: tasksUnknown,
          hint: tasksUnknown
            ? `${boards.length} visible projects; how many are active is unknown while tasks fail to load`
            : `${activeProjects} active of ${boards.length} visible projects`,
        })}
        {@render summaryRow({
          to: 'tasks',
          icon: ListTodo,
          label: 'Tasks',
          value: completedTasks,
          total: tasks.length,
          segments: 10,
          loading,
          unknown: tasksUnknown,
          hint: tasksUnknown
            ? 'Task counts are unavailable — the task read failed'
            : `${completedTasks} completed of ${tasks.length} visible tasks`,
        })}
      </div>
    {/if}
  {/if}
</section>
