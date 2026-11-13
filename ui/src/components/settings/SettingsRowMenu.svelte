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
  }: {
    /** Names the thing the menu acts on, e.g. "Actions for Anthropic". */
    label: string
    items: ContextMenuEntry[] | (() => ContextMenuEntry[])
  } = $props()
</script>

<DropdownMenu {items}>
  {#snippet trigger(open)}
    <IconButton title={label} active={open}><Ellipsis size={16} /></IconButton>
  {/snippet}
</DropdownMenu>
