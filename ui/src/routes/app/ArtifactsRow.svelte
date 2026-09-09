<script lang="ts">
  import { Building2, Folder, Globe } from '@lucide/svelte'
  import Checkbox from '@/components/ui/Checkbox.svelte'
  import { cn } from '@/lib/cn'
  import { relativeTime } from '@/lib/fleet'
  import { KIND_ICON, ROW_GRID, type Row } from './artifacts'

  // One line in the Files browser — a folder or a file, same geometry either
  // way. The grid template lives in artifacts.ts (ROW_GRID) so the header and
  // every line stay locked together as the pane resizes.
  //
  // The whole row is ONE button, and the checkbox is a sibling in its own
  // lane: a click anywhere on the button opens (the interaction we chose),
  // the checkbox is a real control (Checkbox bare — the row around it is the
  // hit context), and BOTH are permanently visible. The checkbox used to
  // float over the kind icon and swap with it on hover — hiding the thing you
  // were about to click was the bug that retired that.
  let {
    row,
    rowKey,
    selected,
    focused,
    active,
    dropTarget,
    onOpen,
    onToggle,
    onFocusIn,
    onPointerDown,
    onContextMenu,
    ondragstart,
    ondragend,
    ondragover,
    ondragleave,
    ondrop,
  }: {
    row: Row
    /** keyOf(row) — the browser's focus model addresses rows by it, and the
     *  data attribute is what its keyboard nav queries the DOM by. */
    rowKey: string
    selected: boolean
    focused: boolean
    active: boolean
    dropTarget: boolean
    onOpen: () => void
    /** Shift rides the event for range selection, exactly like a body click. */
    onToggle: (e: Event) => void
    onFocusIn: () => void
    /** The browser reads shiftKey here — a checkbox `change` event carries no
     *  modifier state, but the pointerdown that caused it does. */
    onPointerDown: (e: PointerEvent) => void
    onContextMenu: (e: MouseEvent) => void
    ondragstart: (e: DragEvent) => void
    ondragend: () => void
    ondragover?: (e: DragEvent) => void
    ondragleave?: () => void
    ondrop?: (e: DragEvent) => void
  } = $props()

  const Icon = $derived(row.kind ? KIND_ICON[row.kind] : Folder)
</script>

<div
  data-row-key={rowKey}
  class="group relative"
  draggable="true"
  {ondragstart}
  {ondragend}
  {ondragover}
  {ondragleave}
  {ondrop}
  oncontextmenu={onContextMenu}
  onfocusin={onFocusIn}
  onpointerdown={onPointerDown}
  role="presentation"
>
  <button
    type="button"
    tabindex={focused ? 0 : -1}
    onclick={(e) => {
      // Modifier clicks select (the desktop grammar); a plain click opens.
      if (e.metaKey || e.ctrlKey || e.shiftKey) { e.preventDefault(); onToggle(e) }
      else onOpen()
    }}
    class={cn(
      'grid w-full items-center gap-3 rounded-lg px-2 py-1.5 text-left transition-colors',
      ROW_GRID,
      'focus-visible:outline-2 focus-visible:outline-accent focus-visible:outline-offset-1',
      selected ? 'bg-raised' : 'dither-fill',
      active && !selected && 'bg-card',
      dropTarget && 'ring-1 ring-accent/60',
    )}
  >
    <!-- Track 1: the checkbox lane. The Checkbox itself is absolutely
         centered over it (a bare input cannot be a grid child of the button
         without nesting interactive elements), so the track reserves the
         space and keeps name/kind/owner/modified aligned with the header. -->
    <span aria-hidden="true"></span>

    <span class="flex min-w-0 items-center gap-2">
      <!-- The kind icon, always visible — selection must never hide what a
           thing IS. -->
      <span class="grid h-5 w-5 shrink-0 place-items-center">
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

  <Checkbox
    bare
    checked={selected}
    title={`Select ${row.name}`}
    onChange={(_checked, e) => {
      // A checkbox change always carries its event; the optional signature
      // exists for Toggle's click path, not this one.
      e?.stopPropagation()
      if (e) onToggle(e)
    }}
    class="absolute left-[15px] top-1/2 h-3.5 w-3.5 -translate-y-1/2"
  />
</div>
