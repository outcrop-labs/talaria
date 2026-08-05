<script lang="ts">
  import { MoreHorizontal, type LucideIcon as IconType } from '@lucide/svelte'
  import Button from '@/components/ui/Button.svelte'
  import DropdownMenu from '@/components/ui/DropdownMenu.svelte'
  import type { ContextMenuEntry } from '@/components/ui/context-menu.svelte'

  // A small kebab (⋯) menu that houses secondary controls (delete, etc.) — the
  // per-item settings area, so destructive actions aren't loose in the sidebar.
  interface MenuItem {
    label: string
    icon?: IconType
    danger?: boolean
    onClick: () => void
  }

  let { items }: { items: MenuItem[] } = $props()

  const entries = $derived(
    items.map(
      (it): ContextMenuEntry => ({
        label: it.label,
        icon: it.icon ? [it.icon, { size: 13 }] : undefined,
        danger: it.danger,
        onSelect: it.onClick,
      }),
    ),
  )
</script>

<DropdownMenu align="right" class="shrink-0" items={entries}>
  {#snippet trigger(_open: boolean)}
    <Button variant="ghost" size="sm" title="More">
      <MoreHorizontal size={14} />
    </Button>
  {/snippet}
</DropdownMenu>
