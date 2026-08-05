<script lang="ts">
  import { Flag } from '@lucide/svelte'
  import DropdownMenu from '@/components/ui/DropdownMenu.svelte'
  import FieldPill from '@/components/ui/FieldPill.svelte'
  import type { MenuIcon } from '@/components/ui/context-menu.svelte'
  import { cn } from '@/lib/cn'
  import { PRIORITIES, PRIORITY_COLOR, type Priority, type Task } from '@/lib/task-const'
  import type { PillCtx } from './field-pills'

  let { t, ctx, class: className }: { t: Task; ctx: PillCtx; class?: string } = $props()

  const flagIcon = (p: Priority): MenuIcon => [Flag, { size: 12, style: `color: ${PRIORITY_COLOR[p]}` }]
</script>

{#snippet flag()}
  <Flag size={11} style="color: {PRIORITY_COLOR[t.priority]}" />
{/snippet}

{#if !ctx.canEdit}
  <span class={cn('inline-flex items-center gap-1.5 font-mono text-[10px] uppercase tracking-[0.05em] text-muted', className)}>
    {@render flag()}
    {t.priority}
  </span>
{:else}
  <DropdownMenu
    align="left"
    class={className}
    items={[...PRIORITIES].reverse().map((p) => ({
      label: p,
      icon: flagIcon(p),
      checked: t.priority === p,
      onSelect: () => ctx.onPatch({ priority: p }),
    }))}
  >
    {#snippet trigger(open)}
      <FieldPill icon={flag} active={open} title="Change priority">
        {t.priority}
      </FieldPill>
    {/snippet}
  </DropdownMenu>
{/if}
