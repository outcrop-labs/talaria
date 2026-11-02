<script lang="ts">
  import { SvelteSet } from 'svelte/reactivity'
  import GroupHeader from '@/components/app/GroupHeader.svelte'
  import { groupRows, type CenterRow } from '@/lib/notify-center'
  import type { ClassInfo } from '@/lib/notify-wire'
  import NotificationRow from './NotificationRow.svelte'

  // The rows of a view, with consecutive rows about one thing (a ticket, a
  // channel, an approval) under one collapsible header: "TAL-112 · 3
  // updates". The bell and the page both draw through this, so a row looks the
  // same wherever it is read.
  let {
    rows,
    classes,
    pictures = {},
    compact = false,
    focusedId = null,
    selection,
    selectable = false,
    canArchive = false,
    canUnread = false,
    onOpen,
    onDone,
    onRestore,
    onToggleRead,
    onSelect,
  }: {
    rows: readonly CenterRow[]
    classes: readonly ClassInfo[] | undefined
    /** user id → photo, from the people directory when cached. */
    pictures?: Record<string, string | null | undefined>
    compact?: boolean
    focusedId?: string | null
    selection?: ReadonlySet<string>
    selectable?: boolean
    canArchive?: boolean
    canUnread?: boolean
    onOpen: (row: CenterRow) => void
    onDone?: (row: CenterRow) => void
    onRestore?: (row: CenterRow) => void
    onToggleRead?: (row: CenterRow) => void
    onSelect?: (row: CenterRow) => void
  } = $props()

  const groups = $derived(groupRows(rows))
  // Collapsed groups, by entity — a collapse survives a refetch, and a
  // focused row inside a collapsed group opens it again (below).
  const collapsed = new SvelteSet<string>()
  const groupOf = (key: string) => key.split('@')[0] ?? key

  $effect(() => {
    if (!focusedId) return
    const g = groups.find((x) => x.rows.some((r) => r.id === focusedId))
    if (g?.label && collapsed.has(groupOf(g.key))) collapsed.delete(groupOf(g.key))
  })
</script>

{#each groups as g (g.key)}
  {#if g.label}
    {@const open = !collapsed.has(groupOf(g.key))}
    <div class="border-b border-line" data-notification-group={g.entity ? `${g.entity.type}:${g.entity.id}` : undefined}>
      <div class="px-2 pt-1">
        <GroupHeader
          label={g.label}
          count={g.rows.filter((r) => !r.readAt).length}
          {open}
          onToggle={() => (open ? collapsed.add(groupOf(g.key)) : collapsed.delete(groupOf(g.key)))}
        />
      </div>
      {#if open}
        <div class="pl-3">
          {#each g.rows as row (row.id)}
            <NotificationRow
              {row}
              {classes}
              picture={row.actor?.id ? pictures[row.actor.id] : null}
              {compact}
              focused={focusedId === row.id}
              selected={selection?.has(row.id) ?? false}
              {selectable}
              {canArchive}
              {canUnread}
              {onOpen}
              {onDone}
              {onRestore}
              {onToggleRead}
              {onSelect}
            />
          {/each}
        </div>
      {/if}
    </div>
  {:else}
    {#each g.rows as row (row.id)}
      <NotificationRow
        {row}
        {classes}
        picture={row.actor?.id ? pictures[row.actor.id] : null}
        {compact}
        focused={focusedId === row.id}
        selected={selection?.has(row.id) ?? false}
        {selectable}
        {canArchive}
        {canUnread}
        {onOpen}
        {onDone}
        {onRestore}
        {onToggleRead}
        {onSelect}
      />
    {/each}
  {/if}
{/each}
