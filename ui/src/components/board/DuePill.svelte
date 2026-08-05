<script lang="ts">
  import { CalendarDays } from '@lucide/svelte'
  import DropdownMenu from '@/components/ui/DropdownMenu.svelte'
  import FieldPill from '@/components/ui/FieldPill.svelte'
  import type { ContextMenuEntry } from '@/components/ui/context-menu.svelte'
  import { cn } from '@/lib/cn'
  import { dateInputValue, dueIsoFromDateInput, dueIsoInDays } from '@/lib/dates'
  import type { Task } from '@/lib/task-const'
  import { isOverdueTask, type PillCtx } from './field-pills'

  let {
    t,
    ctx,
    class: className,
    ghost,
    persistent,
  }: { t: Task; ctx: PillCtx; class?: string; ghost?: boolean; persistent?: boolean } = $props()

  const fmtDue = (iso: string) => {
    const d = new Date(iso)
    const now = new Date()
    const sameYear = d.getFullYear() === now.getFullYear()
    return d.toLocaleDateString(undefined, { month: 'short', day: 'numeric', ...(sameYear ? {} : { year: 'numeric' }) })
  }

  const late = $derived(isOverdueTask(t, ctx.statuses))
  const label = $derived(t.dueDate ? fmtDue(t.dueDate) : 'Due')
  const items: ContextMenuEntry[] = $derived([
    { label: 'Today', onSelect: () => ctx.onPatch({ dueDate: dueIsoInDays(0) }) },
    { label: 'Tomorrow', onSelect: () => ctx.onPatch({ dueDate: dueIsoInDays(1) }) },
    { label: 'Next week', onSelect: () => ctx.onPatch({ dueDate: dueIsoInDays(7) }) },
    ...(t.dueDate ? (['sep', { label: 'Clear', danger: true, onSelect: () => ctx.onPatch({ dueDate: null }) }] as ContextMenuEntry[]) : []),
  ])
</script>

{#snippet calendar()}
  <CalendarDays size={11} />
{/snippet}

{#if !ctx.canEdit}
  {#if t.dueDate}
    <span class={cn('font-mono text-[10px] uppercase tracking-[0.05em]', late ? 'font-medium text-danger' : 'text-muted', className)}>
      {label}
    </span>
  {/if}
{:else}
  <DropdownMenu align="left" class={className} {items}>
    {#snippet trigger(open)}
      <FieldPill
        {persistent}
        icon={calendar}
        active={open}
        empty={!t.dueDate}
        title="Set due date"
        class={cn(
          late && 'font-medium !text-danger',
          // Ghost: an unset property stays invisible until the card is
          // hovered (or its picker is open) — quiet cards, one-click set.
          ghost && !persistent && !t.dueDate && !open && 'opacity-0 transition-opacity group-hover:opacity-100',
        )}
      >
        {ghost && !persistent && !t.dueDate ? '' : label}
      </FieldPill>
    {/snippet}
    {#snippet footer(close)}
      <input
        type="date"
        value={dateInputValue(t.dueDate)}
        oninput={(e) => {
          const iso = dueIsoFromDateInput(e.currentTarget.value)
          if (iso) {
            ctx.onPatch({ dueDate: iso })
            close()
          }
        }}
        class="w-full cursor-pointer bg-transparent text-xs text-fg focus:outline-none"
      />
    {/snippet}
  </DropdownMenu>
{/if}
