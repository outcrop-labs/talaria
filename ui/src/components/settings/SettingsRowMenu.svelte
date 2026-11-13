<script lang="ts">
  import { Ellipsis } from '@lucide/svelte'
  import DropdownMenu from '@/components/ui/DropdownMenu.svelte'
  import IconButton from '@/components/ui/IconButton.svelte'
  import type { ContextMenuEntry } from '@/components/ui/context-menu.svelte'

  // The kebab at the end of an entity row: every action that is not the row's
  // one visible button. Destructive items go last, marked `danger`.
  let {
    label,
    items,
    align,
    up,
  }: {
    /** Names the thing the menu acts on, e.g. "Actions for Anthropic". */
    label: string
    items: ContextMenuEntry[] | (() => ContextMenuEntry[])
    /** Which edge of the kebab the menu hangs from. Right by default. */
    align?: 'left' | 'right'
    /** Open upward, for a kebab at the foot of a dialog or the window. */
    up?: boolean
  } = $props()
</script>

<DropdownMenu {items} {align} {up}>
  {#snippet trigger(open)}
    <IconButton title={label} active={open}><Ellipsis size={16} /></IconButton>
  {/snippet}
</DropdownMenu>
