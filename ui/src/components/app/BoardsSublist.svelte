<script lang="ts" module>
  import type { Board } from '@/lib/boards.svelte'

  // Which groups the user has expanded, keyed by group name. Persisted so the
  // rail comes back the way it was left; unknown groups fall back to their
  // default (teams open, Archived closed) rather than being stored eagerly.
  const GROUPS_KEY = 'talaria:board-groups'
  const ARCHIVED_KEY = ' archived'

  function loadGroupState(): Record<string, boolean> {
    try {
      const raw = localStorage.getItem(GROUPS_KEY)
      if (raw) {
        const v = JSON.parse(raw) as unknown
        if (v && typeof v === 'object' && !Array.isArray(v)) {
          return Object.fromEntries(Object.entries(v as Record<string, unknown>).filter(([, o]) => typeof o === 'boolean')) as Record<string, boolean>
        }
      }
    } catch {
      /* ignore */
    }
    return {}
  }

  const byName = (a: Board, b: Board) => a.name.localeCompare(b.name, undefined, { sensitivity: 'base' })
</script>

<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import { Archive, ExternalLink, Link as LinkIcon } from '@lucide/svelte'
  import ContextMenu from '@/components/ui/ContextMenu.svelte'
  import { useContextMenu, copyAppLink } from '@/components/ui/context-menu.svelte'
  import { alert } from '@/components/ui/confirm.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import GroupHeader from './GroupHeader.svelte'
  import SublistFooter from './SublistFooter.svelte'
  import { cn } from '@/lib/cn'
  import { useBoards, useArchivedBoards, moveBoardToTeam } from '@/lib/boards.svelte'
  import { useTeams } from '@/lib/teams'
  import { navigate, p } from '@/router'

  let { activePath, onNew, onTeams }: { activePath: string; onNew: () => void; onTeams: () => void } = $props()

  const qc = useQueryClient()
  const menu = useContextMenu()
  const boardsQuery = useBoards()
  const archivedQuery = useArchivedBoards()
  const teamsQuery = useTeams()
  const boards = $derived(boardsQuery.data ?? [])
  const archived = $derived(archivedQuery.data ?? [])
  const teams = $derived(teamsQuery.data ?? [])
  // Init to defaults (SSR-safe), then hydrate from localStorage on the client.
  let groupState = $state<Record<string, boolean>>({})
  $effect(() => {
    groupState = loadGroupState()
  })
  // Drag a board you own onto a team header to move it (Personal = no team).
  let dragging = $state<Board | null>(null)
  let overGroup = $state<string | null>(null)

  const toggleGroup = (key: string, isOpen: boolean) => {
    const next = { ...groupState, [key]: !isOpen }
    groupState = next
    try {
      localStorage.setItem(GROUPS_KEY, JSON.stringify(next))
    } catch {
      /* ignore */
    }
  }

  const ordered = $derived.by(() => {
    const groups = new Map<string, { teamId: string | null; boards: Board[] }>()
    groups.set('Personal', { teamId: null, boards: [] })
    for (const t of teams) groups.set(t.name, { teamId: t.id, boards: [] })
    for (const b of boards) {
      const key = b.teamName ?? 'Personal'
      if (!groups.has(key)) groups.set(key, { teamId: b.teamId, boards: [] })
      groups.get(key)!.boards.push(b)
    }
    for (const g of groups.values()) g.boards.sort(byName)
    return [...groups.entries()]
      .filter(([key, g]) => g.boards.length > 0 || (dragging !== null && key !== (dragging.teamName ?? 'Personal')))
      .sort((a, b) => (a[0] === 'Personal' ? -1 : b[0] === 'Personal' ? 1 : a[0].localeCompare(b[0])))
  })

  const drop = async (teamId: string | null, key: string) => {
    overGroup = null
    const b = dragging
    dragging = null
    if (!b || (b.teamId ?? null) === teamId) return
    const r = await moveBoardToTeam(b.id, teamId)
    if (r?.error) void alert({ title: 'Could not move board', message: r.error })
    await qc.invalidateQueries({ queryKey: ['boards'] })
    void key
  }

  const retryLists = () => {
    void boardsQuery.refetch()
    void teamsQuery.refetch()
  }
  // Either read failing makes the rail's grouping suspect (teams supply the
  // group names), so one marker covers both — named for whichever broke.
  const staleFailure = $derived(boardsQuery.isError || teamsQuery.isError)
</script>

{#snippet boardLink(b: Board)}
  <a
    href={p('/boards/:boardId', { params: { boardId: b.id } })}
    draggable={b.role === 'owner'}
    oncontextmenu={(e) =>
      menu.openMenu(e, [
        { label: 'Open', icon: [ExternalLink, { size: 14 }], onSelect: () => navigate('/boards/:boardId', { params: { boardId: b.id } }) },
        { label: 'Copy link', icon: [LinkIcon, { size: 14 }], onSelect: () => copyAppLink(`/boards/${b.id}`) },
      ])}
    ondragstart={() => (dragging = b)}
    ondragend={() => {
      dragging = null
      overGroup = null
    }}
    class={cn(
      // Indented past the group chevron so the nesting reads at a glance.
      'block truncate rounded-md py-1 pl-[22px] pr-2 text-xs transition-colors duration-[120ms] hover:bg-hover hover:text-fg',
      activePath === `/boards/${b.id}` ? 'bg-raised font-medium text-fg' : 'text-muted',
      dragging?.id === b.id && 'opacity-40',
    )}
  >
    {b.name}
  </a>
{/snippet}

{#if boardsQuery.isLoading || teamsQuery.isLoading}
  <!-- While boards/teams load, hold the sublist's shape (group headers + rows) so
       the footer buttons don't render alone and get pushed down when data lands. -->
  <div class="ml-3 mt-0.5 border-l border-line-subtle pl-2">
    <div class="space-y-1.5 px-1.5 py-1">
      <Skeleton class="h-2.5 w-16 rounded-full" />
      <Skeleton class="ml-3 h-2.5 w-24 rounded-full" delay={0.12} />
      <Skeleton class="ml-3 h-2.5 w-20 rounded-full" delay={0.24} />
      <Skeleton class="h-2.5 w-14 rounded-full" delay={0.36} />
      <Skeleton class="ml-3 h-2.5 w-24 rounded-full" delay={0.48} />
    </div>
    <SublistFooter {onNew} {onTeams} />
  </div>
{:else if boardsQuery.isError && boardsQuery.data === undefined}
  <!-- A failed read is not "you have no boards". `data` is undefined on error and
       the `= []` default turns that into an empty list, so every group filters
       out and the rail renders a header, a footer, and nothing between them — the
       original incident, still alive in the sidebar. Say it broke; offer a retry.

       But only when the failure left NOTHING to show. The query cache keeps the
       last good list through a failed background refetch, and blanking a populated
       rail over a transient blip — boards vanishing mid-session — is the worse lie
       of the two. Stale beats blank: below, the list still renders and the failure
       rides along as an inline marker. (Same rule QueryState applies.) -->
  <div class="ml-3 mt-0.5 border-l border-line-subtle pl-2">
    <QueryError
      variant="inline"
      class="px-1.5 py-1"
      title="Could not load boards"
      error={boardsQuery.error}
      onRetry={retryLists}
    />
    <SublistFooter {onNew} {onTeams} />
  </div>
{:else}
  <div class="ml-3 mt-0.5 space-y-2 border-l border-line-subtle pl-2">
    <!-- The list below is the last good read — a refresh failed. Keeping the
         boards on screen and marking them stale beats replacing them with an
         error, but saying nothing would let the rail pass off old data as
         current. -->
    {#if staleFailure}
      <QueryError
        variant="inline"
        class="px-1.5 py-1"
        title={boardsQuery.isError ? 'Boards may be out of date' : 'Teams may be out of date'}
        error={boardsQuery.isError ? boardsQuery.error : teamsQuery.error}
        onRetry={retryLists}
      />
    {/if}
    {#each ordered as [group, g] (group)}
      <!-- Groups start open; a drag force-opens every one so there is always a
           visible drop target. A collapsed group still shows the board you are
           currently on, so the rail never hides where you are. -->
      {@const open = (groupState[group] ?? true) || dragging !== null}
      {@const rows = open ? g.boards : g.boards.filter((b) => activePath === `/boards/${b.id}`)}
      <div
        role="group"
        ondragover={(e) => {
          if (!dragging) return
          e.preventDefault()
          overGroup = group
        }}
        ondragleave={() => {
          if (overGroup === group) overGroup = null
        }}
        ondrop={(e) => {
          e.preventDefault()
          void drop(g.teamId, group)
        }}
        class={cn(dragging && overGroup === group && 'rounded-md bg-raised ring-1 ring-[color:var(--theme-accent-border)]')}
      >
        <GroupHeader label={group} count={g.boards.length} {open} onToggle={() => toggleGroup(group, open)} />
        <ul class="space-y-px">
          {#each rows as b (b.id)}
            <li>{@render boardLink(b)}</li>
          {/each}
          {#if g.boards.length === 0 && dragging}
            <li class="py-0.5 pl-[22px] text-[10px] italic text-muted">drop here</li>
          {/if}
        </ul>
      </div>
    {/each}

    <!-- Archived fails on its own budget: the live boards above loaded, so
         they stay. Omitting this section silently would read as "nothing is
         archived", which is a different claim from "we couldn't ask". -->
    {#if archivedQuery.isError}
      <QueryError
        variant="inline"
        class="px-1.5 py-1"
        title="Could not load archived"
        error={archivedQuery.error}
        onRetry={() => void archivedQuery.refetch()}
      />
    {/if}

    {#if archived.length > 0}
      {@const open = groupState[ARCHIVED_KEY] ?? false}
      <div>
        <GroupHeader
          label="Archived"
          count={archived.length}
          {open}
          onToggle={() => toggleGroup(ARCHIVED_KEY, open)}
        >
          {#snippet icon()}<Archive size={11} />{/snippet}
        </GroupHeader>
        {#if open}
          <ul class="space-y-px">
            {#each [...archived].sort(byName) as b (b.id)}
              <li>{@render boardLink(b)}</li>
            {/each}
          </ul>
        {/if}
      </div>
    {/if}

    <SublistFooter {onNew} {onTeams} />
    <ContextMenu {menu} />
  </div>
{/if}
