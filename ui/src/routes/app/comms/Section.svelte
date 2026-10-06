<script lang="ts">
  import type { Snippet } from 'svelte'
  import { ChevronRight, Ellipsis, Plus } from '@lucide/svelte'
  import { cn } from '@/lib/cn'
  import { listStagger, slide } from '@/lib/motion'
  import Input from '@/components/ui/Input.svelte'
  import IconButton from '@/components/ui/IconButton.svelte'
  import DropdownMenu from '@/components/ui/DropdownMenu.svelte'
  import Materialize from '@/components/ui/Materialize.svelte'
  import type { ContextMenuEntry } from '@/components/ui/context-menu.svelte'

  // A collapsible sidebar section (R15). The header is the §8 canonical — 10px
  // mono uppercase 0.08em ink-dim, right-aligned mono meta — led by a chevron
  // that toggles the section (aria-expanded). Its two controls, a create `+`
  // and a more-actions `…`, show on header hover and keyboard focus, and stay
  // up while one of their menus is open.
  //
  // `+` is either a click (`onAdd`) or a menu (`addItems` — the people picker
  // for a new DM). The inline create input is driven by the parent (`create`),
  // because one section can create two kinds of thing (Channels: a channel
  // from `+`, a relay from `…`).
  let {
    label,
    meta,
    collapsed = false,
    onToggle,
    addTitle,
    onAdd,
    addItems,
    moreItems,
    create = null,
    loading = false,
    count = 3,
    rowSkeleton,
    children,
  }: {
    label: string
    /** Right-aligned mono meta — real counts only, no fabricated data. */
    meta?: string
    collapsed?: boolean
    onToggle?: () => void
    addTitle?: string
    onAdd?: () => void
    addItems?: ContextMenuEntry[] | (() => ContextMenuEntry[])
    moreItems?: ContextMenuEntry[] | (() => ContextMenuEntry[])
    /** The inline name input, while one is open. */
    create?: { placeholder: string; submit: (name: string | null) => void } | null
    /** First load of the query behind this section — row-shaped skeletons
     *  materialize into the rows (Materialize) instead of a branch swap. */
    loading?: boolean
    count?: number
    /** One row's silhouette, mirroring this section's RailRow anatomy. */
    rowSkeleton?: Snippet<[number]>
    children: Snippet
  } = $props()

  let name = $state('')
  const submit = (cancelled: boolean) => {
    const v = name.trim()
    name = ''
    create?.submit(v && !cancelled ? v : null)
  }

  const sectionId = $derived(`comms-section-${label.toLowerCase().replace(/\s+/g, '-')}`)
</script>

<div class="mb-3">
  <div class="group mb-1 flex h-6 items-center gap-1 px-1">
    <button
      type="button"
      aria-expanded={!collapsed}
      aria-controls={sectionId}
      onclick={onToggle}
      class="flex min-w-0 items-center gap-1 rounded px-1 py-0.5 text-ink-dim outline-none transition-colors hover:text-fg focus-visible:ring-2 focus-visible:ring-accent-soft"
    >
      <ChevronRight size={11} class={cn('shrink-0 transition-transform duration-150', !collapsed && 'rotate-90')} />
      <span class="font-mono text-[10px] uppercase tracking-[0.08em]">{label}</span>
    </button>
    <span class="ml-auto"></span>
    {#if meta}<span class="font-mono text-[10px] tracking-[0.05em] text-muted">{meta}</span>{/if}
    <div
      class={cn(
        'flex items-center gap-0.5 opacity-0 transition-opacity group-hover:opacity-100 group-focus-within:opacity-100',
        // A menu that is open keeps its trigger in sight.
        'has-[[aria-expanded=true]]:opacity-100',
      )}
    >
      {#if moreItems}
        <DropdownMenu items={moreItems}>
          {#snippet trigger(open)}
            <IconButton size="sm" class="h-5 w-5" title={`${label}: more actions`} active={open} aria-haspopup="menu" aria-expanded={open}>
              <Ellipsis size={13} />
            </IconButton>
          {/snippet}
        </DropdownMenu>
      {/if}
      {#if addItems}
        <DropdownMenu items={addItems}>
          {#snippet trigger(open)}
            <IconButton size="sm" class="h-5 w-5" title={addTitle ?? `New in ${label}`} active={open} aria-haspopup="menu" aria-expanded={open}>
              <Plus size={13} />
            </IconButton>
          {/snippet}
        </DropdownMenu>
      {:else if onAdd}
        <IconButton size="sm" class="h-5 w-5" title={addTitle ?? `New in ${label}`} onclick={onAdd}>
          <Plus size={13} />
        </IconButton>
      {/if}
    </div>
  </div>
  {#if create}
    <div transition:slide={{ duration: 150 }} class="mb-1 px-1">
      <!-- svelte-ignore a11y_autofocus -->
      <Input
        autofocus
        size="sm"
        bind:value={name}
        placeholder={create.placeholder}
        onkeydown={(e) => {
          if (e.key === 'Enter') submit(false)
          else if (e.key === 'Escape') submit(true)
        }}
        onblur={() => submit(false)}
      />
    </div>
  {/if}
  <!-- Any grid or list staggers its items on mount (ANIMATIONS.md). With a
       rowSkeleton the section renders through Materialize: row-shaped
       skeletons occupy the same stack and the real rows stagger in over them
       (its content branch owns the cascade). Rows are divs (see RailRow).
       Collapsed, the parent still passes the active and unread rows. -->
  <div id={sectionId}>
    {#if rowSkeleton}
      <Materialize {loading} {count} class="space-y-0.5">
        {#snippet skeleton(i)}{@render rowSkeleton(i)}{/snippet}
        {@render children()}
      </Materialize>
    {:else}
      <div use:listStagger class="space-y-0.5">{@render children()}</div>
    {/if}
  </div>
</div>
