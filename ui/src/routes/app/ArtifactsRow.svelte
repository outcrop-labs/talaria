<script lang="ts">
  import { Building2, Folder, Globe } from '@lucide/svelte'
  import { cn } from '@/lib/cn'
  import { relativeTime } from '@/lib/fleet'
  import { KIND_ICON, type Row } from './artifacts'

  // One line in the Files browser — a folder or a file, same geometry either
  // way. The column widths are the grid template in ArtifactsBrowser; this row
  // repeats it so the header and every line stay locked together as the pane
  // resizes.
  //
  // The whole row is ONE button, with the checkbox floated over the icon lane
  // beside it: a click anywhere opens (the interaction we chose), and the
  // checkbox is a real control rather than a div wearing a click handler.
  let {
    row,
    selected,
    /** Something is selected somewhere, so every checkbox stays visible —
     *  hunting for a hover target mid-multi-select is miserable. */
    anySelected,
    active,
    dropTarget,
    onOpen,
    onToggle,
    onContextMenu,
    ondragstart,
    ondragend,
    ondragover,
    ondragleave,
    ondrop,
  }: {
    row: Row
    selected: boolean
    anySelected: boolean
    active: boolean
    dropTarget: boolean
    onOpen: () => void
    onToggle: (e: MouseEvent) => void
    onContextMenu: (e: MouseEvent) => void
    ondragstart: (e: DragEvent) => void
    ondragend: () => void
    ondragover?: (e: DragEvent) => void
    ondragleave?: () => void
    ondrop?: (e: DragEvent) => void
  } = $props()

  const Icon = $derived(row.kind ? KIND_ICON[row.kind] : Folder)
  const showBox = $derived(selected || anySelected)
</script>

<div
  class="group relative"
  draggable="true"
  {ondragstart}
  {ondragend}
  {ondragover}
  {ondragleave}
  {ondrop}
  oncontextmenu={onContextMenu}
  role="presentation"
>
  <button
    type="button"
    onclick={onOpen}
    class={cn(
      'grid w-full grid-cols-[minmax(0,1fr)_7rem_10rem_8rem] items-center gap-3 rounded-lg px-2 py-1.5 text-left transition-colors',
      selected ? 'bg-raised' : 'dither-fill',
      active && !selected && 'bg-card',
      dropTarget && 'ring-1 ring-accent/60',
    )}
  >
    <span class="flex min-w-0 items-center gap-2">
      <!-- Fixed icon lane — emoji and lucide share one slot, and the checkbox
           takes it over on hover so neither costs a column. -->
      <span class={cn('grid h-5 w-5 shrink-0 place-items-center transition-opacity', showBox ? 'opacity-0' : 'opacity-100 group-hover:opacity-0')}>
        {#if row.icon}
          <span class="text-[15px] leading-none">{row.icon}</span>
        {:else}
          <Icon size={15} class={row.type === 'folder' ? 'text-accent' : 'text-muted'} />
        {/if}
      </span>
      <span class="min-w-0 truncate font-sans text-sm text-fg">{row.name}</span>
      <!-- Public is the one access state worth interrupting a filename for:
           everything else is between colleagues, this one is the internet. -->
      {#if row.artifact?.visibility === 'public'}
        <Globe size={12} class="shrink-0 text-accent" aria-label="Public on the internet" />
      {/if}
    </span>

    <span class="truncate font-mono text-[11px] tracking-[0.05em] text-muted">{row.kindLabel}</span>
    <span class="flex min-w-0 items-center gap-1.5 font-sans text-[13px] text-muted">
      {#if row.scope === 'workspace'}
        <!-- Ownerless: the organization's file, and the name beside it is
             whichever agent wrote it rather than a person who owns it. -->
        <Building2 size={12} class="shrink-0 opacity-70" aria-label="Owned by the workspace" />
      {/if}
      <span class="truncate">{row.owner}</span>
    </span>
    <span class="truncate font-mono text-[11px] tracking-[0.05em] text-muted">{relativeTime(row.modified)}</span>
  </button>

  <input
    type="checkbox"
    checked={selected}
    aria-label={`Select ${row.name}`}
    onclick={(e) => {
      e.stopPropagation()
      onToggle(e)
    }}
    class={cn(
      'absolute left-[11px] top-1/2 h-3.5 w-3.5 -translate-y-1/2 cursor-pointer accent-accent transition-opacity',
      showBox ? 'opacity-100' : 'opacity-0 group-hover:opacity-100',
    )}
  />
</div>
