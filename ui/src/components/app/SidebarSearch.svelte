<script lang="ts" module>
  import type { Task } from '@/lib/task-const'

  interface IndexedTask extends Task {
    boardName: string
  }
</script>

<script lang="ts">
  import { createQueries } from '@tanstack/svelte-query'
  import { FolderKanban, ListTodo, Search, X } from '@lucide/svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import { listQuery } from '@/components/ui/query-state'
  import { fade, listStagger, slide } from '@/lib/motion'
  import { getList } from '@/lib/fetch-json'
  import { useBoards, type Board } from '@/lib/boards.svelte'
  import { p } from '@/router'

  // Sidebar search. This was `SidebarWorkOverview`, which carried two progress
  // meters ("2/7 projects", "14/60 tasks") above the box. They were the first
  // thing in the rail and the least useful: a ratio nobody set a target for,
  // over boards the person may not even work on, that moved on its own. The
  // meters are gone; what people came to this corner for is the box.
  let query = $state('')
  const normalizedQuery = $derived(query.trim().toLowerCase())
  const searching = $derived(normalizedQuery.length > 0)

  // A board read that 500s must never arrive here as an empty list: "no matches"
  // and "we could not look" are different sentences, and only one of them is
  // about the user's search.
  const boardsQuery = useBoards()
  const boardList = listQuery(boardsQuery, {
    title: 'Could not search your projects',
    staleTitle: 'These results may be out of date',
    variant: 'inline',
  })
  const boards = $derived(boardList.rows)
  // Same key and same reader as `useBoardTasks` on purpose: two queryFns under
  // one key hand each other's consumers the other's row shape. The board name
  // is joined below, out of the cached payload, so what lands in the cache is
  // exactly what the board views expect.
  //
  // `enabled` waits for a search. One request per board, on every page, is a
  // real cost that the meters used to justify and nothing does now — the rows
  // are only ever read to answer a query that has been typed.
  const taskQueries = createQueries(() => ({
    queries: boardList.rows.map((board) => ({
      queryKey: ['board-tasks', board.id, false],
      queryFn: (): Promise<Task[]> => getList<Task>(`/api/boards/${board.id}/tasks`, 'tasks'),
      staleTime: 30_000,
      enabled: searching,
    })),
  }))

  const tasks: IndexedTask[] = $derived(
    taskQueries.flatMap((result, index) =>
      (result.data ?? []).map((task) => ({ ...task, boardName: boards[index]?.name ?? '' })),
    ),
  )
  // `fetchStatus !== 'idle'` is what keeps a query that is merely DISABLED from
  // reading as "still loading" — pending and idle is the resting state now.
  const tasksPending = $derived(taskQueries.some((result) => result.isPending && result.fetchStatus !== 'idle'))
  const tasksFailed = $derived(taskQueries.filter((result) => result.isError && result.data === undefined))
  const retryTasks = () => {
    for (const result of taskQueries) if (result.isError) void result.refetch()
  }
  const loading = $derived(boardList.pending || tasksPending)

  const projectMatches = $derived(
    searching ? boards.filter((board) => board.name.toLowerCase().includes(normalizedQuery)).slice(0, 3) : [],
  )
  const taskMatches = $derived(
    searching
      ? tasks
          .filter(
            (task) =>
              task.title.toLowerCase().includes(normalizedQuery) || task.boardName.toLowerCase().includes(normalizedQuery),
          )
          .slice(0, Math.max(0, 6 - projectMatches.length))
      : [],
  )
</script>

{#snippet searchResults(projects: Board[], taskRows: IndexedTask[])}
  {#if loading}
    <div class="px-2 pt-3 font-mono text-[9px] uppercase tracking-[0.08em] text-muted">Searching…</div>
  {:else if projects.length === 0 && taskRows.length === 0}
    <div in:fade={{ duration: 150 }} class="px-2 pt-3 font-sans text-[11px] leading-4 text-muted">
      Nothing matches “{query.trim()}”.
    </div>
  {:else}
    <div class="mt-2 max-h-52 space-y-0.5 overflow-y-auto" use:listStagger>
      {#each projects as board (board.id)}
        <a
          in:fade={{ duration: 150 }}
          out:slide={{ duration: 120 }}
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
          in:fade={{ duration: 150 }}
          out:slide={{ duration: 120 }}
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

<section aria-label="Search" class="mt-5 shrink-0">
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
      aria-label="Search"
      placeholder="Search"
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

  <!-- Failures are reported while the person is SEARCHING, which is the only
       time this component reads anything. A standing error box over an idle
       search field is noise about work nobody asked for; a silent empty result
       set while a read is failing is the lie this app keeps hunting down. -->
  {#if searching}
    {#if boardList.notice}
      <div class="mt-3 px-2"><QueryError {...boardList.notice} /></div>
    {/if}
    {#if tasksFailed.length > 0}
      <QueryError
        error={tasksFailed[0]?.error}
        title="Could not search your tasks"
        variant="inline"
        onRetry={retryTasks}
        class="mt-3 px-2"
      />
    {/if}
    {#if !boardList.failed}
      {@render searchResults(projectMatches, taskMatches)}
    {/if}
  {/if}
</section>
