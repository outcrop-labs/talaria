<script lang="ts" module>
  import ColorDot from '@/components/board/ColorDot.svelte'
  import { LABEL_CSS } from '@/components/board/field-pills'
  import type { ContextMenuItem } from '@/components/ui/context-menu.svelte'
  import type { LabelColor } from '@/lib/boards.svelte'

  /** The palette in `LABEL_CSS` key order — the order every swatch list shows. */
  const COLOR_NAMES = Object.keys(LABEL_CSS) as LabelColor[]

  /** THE SWATCH LIST: one ticked entry per colour, built from the palette.
   *
   *  Takes which colours to offer — the board rows show the whole palette, the
   *  ticket menu its own `TICKET_COLORS` order — the value to tick, and what to
   *  do on pick. The builder owns no write, so a caller routes the pick through
   *  whatever error surface its own mutations run through. */
  export const colorEntries = (
    colors: readonly LabelColor[],
    value: string | null | undefined,
    onPick: (color: LabelColor) => void,
  ): ContextMenuItem[] =>
    colors.map((c) => ({
      label: c,
      icon: [ColorDot, { class: 'h-2.5 w-2.5 rounded-full', color: LABEL_CSS[c] }],
      checked: value === c,
      onSelect: () => onPick(c),
    }))
</script>

<script lang="ts">
  import DropdownMenu from './DropdownMenu.svelte'
  import { cn } from '@/lib/cn'

  // The colour swatch: the dot that shows a row's colour and opens the palette.
  // The board's status and label rows both use it; the ticket menu reuses the
  // entry builder alone, because its colours live in a submenu.
  let {
    value,
    onPick,
    disabled = false,
  }: {
    /** The current colour, ticked in the list. */
    value?: string | null
    onPick: (color: LabelColor) => void
    disabled?: boolean
  } = $props()
</script>

<DropdownMenu align="left" items={colorEntries(COLOR_NAMES, value, onPick)}>
  {#snippet trigger(open)}
    <button
      title="Color"
      {disabled}
      class={cn('h-4 w-4 shrink-0 rounded-full ring-2 transition-shadow', open ? 'ring-[var(--theme-accent-border)]' : 'ring-transparent')}
      style:background={LABEL_CSS[value as LabelColor] ?? 'var(--theme-muted)'}
    ></button>
  {/snippet}
</DropdownMenu>