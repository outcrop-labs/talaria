<script lang="ts">
  import { Timer } from '@lucide/svelte'
  import DropdownMenu from '@/components/ui/DropdownMenu.svelte'
  import FieldPill from '@/components/ui/FieldPill.svelte'
  import { cn } from '@/lib/cn'
  import type { Task } from '@/lib/task-const'
  import type { PillCtx } from './field-pills'

  let {
    t,
    ctx,
    class: className,
    ghost,
    persistent,
  }: { t: Task; ctx: PillCtx; class?: string; ghost?: boolean; persistent?: boolean } = $props()

  const label = $derived(t.estimatedHours != null ? `${t.estimatedHours}h` : 'Estimate')
</script>

{#snippet timer()}
  <Timer size={11} />
{/snippet}

{#if !ctx.canEdit}
  {#if t.estimatedHours != null}
    <span class={cn('font-mono text-[10px] uppercase tracking-[0.05em] text-muted', className)}>{label}</span>
  {/if}
{:else}
  <DropdownMenu
    align="left"
    class={className}
    items={[0.5, 1, 2, 4, 8].map((h) => ({
      label: `${h}h`,
      checked: t.estimatedHours === h,
      onSelect: () => ctx.onPatch({ estimatedHours: h }),
    }))}
  >
    {#snippet trigger(open)}
      <FieldPill
        {persistent}
        icon={timer}
        active={open}
        empty={t.estimatedHours == null}
        title="Set estimate (hours)"
        class={cn(ghost && !persistent && t.estimatedHours == null && !open && 'opacity-0 transition-opacity group-hover:opacity-100')}
      >
        {ghost && !persistent && t.estimatedHours == null ? '' : label}
      </FieldPill>
    {/snippet}
    {#snippet footer(close)}
      <input
        type="number"
        min={0}
        max={999}
        step={0.5}
        placeholder="Custom hours"
        value={t.estimatedHours ?? ''}
        onkeydown={(e) => {
          if (e.key !== 'Enter') return
          const v = (e.target as HTMLInputElement).value.trim()
          const n = v === '' ? null : Number(v)
          if (n === null || (!Number.isNaN(n) && n >= 0)) {
            ctx.onPatch({ estimatedHours: n })
            close()
          }
        }}
        class="w-full bg-transparent text-xs text-fg [appearance:textfield] focus:outline-none"
      />
    {/snippet}
  </DropdownMenu>
{/if}
