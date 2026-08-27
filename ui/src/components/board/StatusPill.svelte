<script lang="ts">
  import DropdownMenu from '@/components/ui/DropdownMenu.svelte'
  import FieldPill from '@/components/ui/FieldPill.svelte'
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import { cn } from '@/lib/cn'
  import { statusColorOf, statusLabelOf } from '@/lib/statuses'
  import { STATUS_LABEL, TASK_STATUSES, type Task } from '@/lib/task-const'
  import { STATUS_COLOR, dotIcon, type PillCtx } from './field-pills'

  let { t, ctx, class: className }: { t: Task; ctx: PillCtx; class?: string } = $props()

  const sts = $derived(ctx.statuses ?? [])
  const label = $derived(statusLabelOf(t.status, sts))
  const dot = $derived(sts.length ? statusColorOf(t.status, sts) : STATUS_COLOR[t.status])
  const options = $derived(
    sts.length
      ? sts.map((st) => ({ key: st.key, label: st.label, color: statusColorOf(st.key, sts) }))
      : TASK_STATUSES.map((k) => ({ key: k as string, label: STATUS_LABEL[k] ?? k, color: STATUS_COLOR[k] ?? 'var(--theme-muted)' })),
  )
</script>

{#if !ctx.canEdit}
  <span class={cn('inline-flex items-center gap-1.5 font-mono text-[10px] uppercase tracking-[0.05em] text-muted', className)}>
    <StatusDot color={dot} />
    {label}
  </span>
{:else}
  <DropdownMenu
    align="left"
    class={className}
    items={options.map((o) => ({
      label: o.label,
      icon: dotIcon(o.color),
      checked: t.status === o.key,
      onSelect: () => ctx.onPatch({ status: o.key as Task['status'] }),
    }))}
  >
    {#snippet trigger(open)}
      <FieldPill {dot} active={open} title="Change status">
        {label}
      </FieldPill>
    {/snippet}
  </DropdownMenu>
{/if}
