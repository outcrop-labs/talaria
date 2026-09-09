<script lang="ts">
  import { Folder } from '@lucide/svelte'
  import Checkbox from '@/components/ui/Checkbox.svelte'
  import { cn } from '@/lib/cn'
  import { KIND_ICON, type Row } from './artifacts'

  // The grid view's cell — a desktop icon view, not a card. Big mark, name
  // centred underneath, no border and no meta line: the density and the silence
  // ARE the idiom, and the list view is where Kind/Owner/Modified live. Same
  // selection and drag grammar as ArtifactsRow, so switching views never
  // changes what a click does.
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
    rowKey: string
    selected: boolean
    focused: boolean
    active: boolean
    dropTarget: boolean
    onOpen: () => void
    onToggle: (e: Event) => void
    onFocusIn: () => void
    onPointerDown: (e: PointerEvent) => void
    onContextMenu: (e: MouseEvent) => void
    ondragstart: (e: DragEvent) => void
    ondragend: () => void
    ondragover?: (e: DragEvent) => void
    ondragleave?: () => void
    ondrop?: (e: DragEvent) => void
  } = $props()

  const Icon = $derived(row.kind ? KIND_ICON[row.kind] : Folder)
  // An icon view is big enough to be its own thumbnail: uploaded images show
  // themselves rather than a generic paperclip.
  const thumb = $derived(
    row.artifact?.storageRef && row.artifact.contentType?.startsWith('image/') ? `/api/uploads/${row.artifact.storageRef}` : null,
  )
</script>

<div
  data-row-key={rowKey}
  role="presentation"
  draggable="true"
  {ondragstart}
  {ondragend}
  {ondragover}
  {ondragleave}
  {ondrop}
  oncontextmenu={onContextMenu}
  onfocusin={onFocusIn}
  onpointerdown={onPointerDown}
  class="group relative"
>
  <button
    type="button"
    tabindex={focused ? 0 : -1}
    onclick={(e) => {
      // Modifier clicks select (the desktop grammar); a plain click opens.
      if (e.metaKey || e.ctrlKey || e.shiftKey) { e.preventDefault(); onToggle(e) }
      else onOpen()
    }}
    title={`${row.name} · ${row.kindLabel}`}
    class={cn(
      'flex w-full flex-col items-center gap-1.5 rounded-lg px-1 py-2.5 transition-colors',
      'focus-visible:outline-2 focus-visible:outline-accent focus-visible:outline-offset-1',
      selected ? 'bg-raised' : 'dither-fill',
      active && !selected && 'bg-card',
      dropTarget && 'ring-1 ring-accent/60',
    )}
  >
    <span class="grid h-12 w-12 shrink-0 place-items-center">
      {#if thumb}
        <img src={thumb} alt="" class="max-h-12 max-w-12 rounded border border-line-subtle object-contain" />
      {:else if row.icon}
        <span class="text-[34px] leading-none">{row.icon}</span>
      {:else}
        <Icon size={38} strokeWidth={1.25} class={row.type === 'folder' ? 'text-accent' : 'text-muted'} />
      {/if}
    </span>
    <!-- Two lines, then an ellipsis — a filename is the one thing an icon view
         must never hide, and one line hides most of them. -->
    <span class="line-clamp-2 w-full break-words px-0.5 text-center font-sans text-[11px] leading-snug text-fg">
      {row.name}
    </span>
  </button>

  <!-- Corner checkbox, permanently visible — tiles have no icon lane to share,
       and a selection control that hides until hunted for is no affordance. -->
  <Checkbox
    bare
    checked={selected}
    title={`Select ${row.name}`}
    onChange={(_checked, e) => {
      e.stopPropagation()
      onToggle(e)
    }}
    class="absolute left-1.5 top-1.5 h-3.5 w-3.5"
  />
</div>
