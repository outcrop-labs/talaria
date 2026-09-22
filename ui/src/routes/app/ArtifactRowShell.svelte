<script lang="ts">
  import type { Snippet } from 'svelte'
  import Checkbox from '@/components/ui/Checkbox.svelte'
  import { cn } from '@/lib/cn'
  import type { RowInteraction } from './artifacts'

  // The wrapper every Files view shares: one draggable line (the list's row,
  // the grid's cell), one button that opens or selects it, one permanent
  // checkbox — and the modifier-click grammar that decides which of the two a
  // click means. It lives here rather than in artifacts.ts because it is
  // markup; the props it reads are the one RowInteraction type declared there,
  // which is what keeps the two views' idea of a click from drifting.
  //
  // What is INSIDE the button is the view's own: the columns and the meta line
  // are the list's, the big mark and the centred name are the grid's. Only the
  // geometry knobs come in as props.
  let {
    row,
    rowKey,
    selected,
    cut,
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
    /** The button's layout — the list's column grid, the grid's column. The
     *  states above append to it. */
    buttonClass,
    /** Where this view parks its checkbox. Both are `bare` (the row around it
     *  is the hit context) and both are permanently visible: a selection
     *  control that hides until hunted for is no affordance. */
    checkboxClass,
    /** The grid names the kind on hover; the list already has a Kind column. */
    title,
    children,
  }: RowInteraction
    & { buttonClass: string; checkboxClass: string; title?: string; children: Snippet } = $props()
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
    {title}
    class={cn(
      buttonClass,
      'transition-colors',
      'focus-visible:outline-2 focus-visible:outline-accent focus-visible:outline-offset-1',
      selected ? 'bg-raised' : 'dither-fill',
      active && !selected && 'bg-card',
      cut && 'opacity-50',
      dropTarget && 'ring-1 ring-accent/60',
    )}
  >
    {@render children()}
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
    class={checkboxClass}
  />
</div>