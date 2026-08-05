<script lang="ts">
  import type { Snippet } from 'svelte'
  import DropdownMenu from '@/components/ui/DropdownMenu.svelte'
  import FieldPill from '@/components/ui/FieldPill.svelte'
  import type { ContextMenuEntry } from '@/components/ui/context-menu.svelte'

  let {
    icon,
    label,
    count,
    items,
  }: {
    icon: Snippet
    label: string
    count: number
    items: ContextMenuEntry[] | (() => ContextMenuEntry[])
  } = $props()
</script>

<DropdownMenu align="left" {items}>
  {#snippet trigger(open)}
    <FieldPill {icon} active={open || count > 0} class="h-9 rounded-md px-2">
      {label}{count > 0 ? ` · ${count}` : ''}
    </FieldPill>
  {/snippet}
</DropdownMenu>
