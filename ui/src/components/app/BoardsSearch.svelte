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

  // THE STRIP'S SEARCH — the box that left the sidebar with the rail (TALA-84).
  // An icon beside the bell; a click expands it into the input. The search it
  // does is the one the rail's box did (boards + tasks across all boards), not
  // a new global search: there is no command palette backend yet, and a box
  // that answers "which board, which ticket" is honest about that.
  //
  // One request per board fires only while a query is typed — `enabled`
  // waits for the box to be expanded AND have text, so a resting strip makes
  // no reads at all.
  let expanded = $state(false)
  let query = $state('')
  const normalizedQuery = $derived(query.trim().toLowerCase())
  const searching = $derived(expanded && normalizedQuery.length > 0)

  // A board read that 500s must never arrive here as an empty list: "no
  // matches" and "we could not look" are different sentences, and only one of
  // them is about the user's search.
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

  let inputEl = $state<HTMLInputElement | null>(null)

  function collapse() {
    expanded = false
    query = ''
  }

  function onKeydown(event: KeyboardEvent) {
    if (event.key === 'Escape') {
      collapse()
      return
    }
    if (event.key === 'Enter' && projectMatches.length > 0 && taskMatches.length === 0) {
      void p('/boards/:boardId', { params: { boardId: projectMatches[0]!.id } })
      collapse()
    }
  }

  function pick() {
    collapse()
  }
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
          onclick={pick}
          class="flex min-h-8 items-center gap-2 rounded-md px-2 py-1 text-muted transition-colors dither-fill hover:text-fg"
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
          onclick={pick}
          class="flex min-h-8 items-start gap-2 rounded-md px-2 py-1.5 text-muted transition-colors dither-fill hover:text-fg"
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

<section aria-label="Search" class="relative shrink-0">
  {#if expanded}
    <!-- The expanded box, in the strip itself (spec §6's expanding search):
         the input replaces the icon in place, results drop below, Escape or
         the X hands the width back. Autofocused — a click on a search icon
         IS the intent to type. -->
    <div class="relative w-64">
      <Search
        size={14}
        strokeWidth={1.7}
        class="pointer-events-none absolute left-3 top-1/2 -translate-y-1/2 text-muted"
        aria-hidden="true"
      />
      <input
        bind:this={inputEl}
        bind:value={query}
        onkeydown={onKeydown}
        aria-label="Search"
        placeholder="Search"
        class="h-7 w-full rounded-lg border border-line bg-raised pl-8 pr-7 font-sans text-xs text-fg outline-none transition-colors placeholder:text-muted hover:border-line-strong focus:border-[color:var(--theme-accent-border)] focus:ring-1 focus:ring-[color:var(--theme-accent-border)]"
      />
      <button
        type="button"
        onclick={collapse}
        aria-label="Close search"
        class="absolute right-1.5 top-1/2 grid h-5 w-5 -translate-y-1/2 place-items-center rounded text-muted transition-colors dither-fill hover:text-fg"
      >
        <X size={12} strokeWidth={1.7} />
      </button>
    </div>
    <!-- Failures are reported while the person is SEARCHING, which is the only
         time this component reads anything. A standing error box over an idle
         search field is noise about work nobody asked for; a silent empty result
         set while a read is failing is the lie this app keeps hunting down.
         Positioned under the box, right-aligned with the strip's cluster. -->
    <div class="absolute right-0 top-9 z-50 w-80 rounded-lg border border-line bg-panel p-2 shadow-[var(--theme-shadow-2)]">
      {#if searching}
        {#if boardList.notice}
          <div class="px-2 pb-2"><QueryError {...boardList.notice} /></div>
        {/if}
        {#if tasksFailed.length > 0}
          <QueryError
            error={tasksFailed[0]?.error}
            title="Could not search your tasks"
            variant="inline"
            onRetry={retryTasks}
            class="px-2 pb-2"
          />
        {/if}
        {#if !boardList.failed}
          {@render searchResults(projectMatches, taskMatches)}
        {/if}
      {/if}
    </div>
  {:else}
    <button
      type="button"
      onclick={() => {
        expanded = true
        queueMicrotask(() => inputEl?.focus())
      }}
      aria-label="Search"
      class="flex h-7 w-7 items-center justify-center rounded-md text-muted transition-colors duration-[120ms] dither-fill hover:text-fg"
    >
      <Search size={15} class="shrink-0" />
    </button>
  {/if}
</section>
