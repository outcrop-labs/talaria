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

<!-- A fixed cell, so down a list the kebabs share one column. A row with
     nothing in its menu passes an empty list and still holds the cell. -->
<div data-row-menu class="grid w-8 shrink-0 place-items-center">
  {#if typeof items === 'function' || items.length > 0}
    <DropdownMenu {items} {align} {up}>
      {#snippet trigger(open)}
        <IconButton title={label} active={open}><Ellipsis size={16} /></IconButton>
      {/snippet}
    </DropdownMenu>
  {/if}
</div>
