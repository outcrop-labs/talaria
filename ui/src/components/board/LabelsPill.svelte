<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import { Tag } from '@lucide/svelte'
  import DropdownMenu from '@/components/ui/DropdownMenu.svelte'
  import FieldPill from '@/components/ui/FieldPill.svelte'
  import type { ContextMenuEntry } from '@/components/ui/context-menu.svelte'
  import { cn } from '@/lib/cn'
  import { createBoardLabel } from '@/lib/boards.svelte'
  import type { Task } from '@/lib/task-const'
  import LabelChip from './LabelChip.svelte'
  import { LABEL_CSS, dotIcon, type PillCtx } from './field-pills'

  let {
    t,
    ctx,
    class: className,
    ghost,
    persistent,
  }: { t: Task; ctx: PillCtx; class?: string; ghost?: boolean; persistent?: boolean } = $props()

  const qc = useQueryClient()
  const labels = $derived(ctx.labels ?? [])
  const shown = $derived(t.tags.slice(0, 2))

  const toggle = (name: string) =>
    ctx.onPatch({ tags: t.tags.includes(name) ? t.tags.filter((x) => x !== name) : [...t.tags, name] })

  const items = (): ContextMenuEntry[] =>
    labels.map((l) => ({
      label: l.name,
      icon: dotIcon(LABEL_CSS[l.color]),
      checked: t.tags.includes(l.name),
      keepOpen: true,
      onSelect: () => toggle(l.name),
    }))
</script>

{#snippet tag()}
  <Tag size={11} />
{/snippet}

{#if !ctx.canEdit}
  {#if t.tags.length > 0}
    <span class={cn('inline-flex items-center gap-1', className)}>
      {#each shown as n (n)}
        <LabelChip name={n} {labels} />
      {/each}
      {#if t.tags.length > 2}<span class="text-[10px] text-muted">+{t.tags.length - 2}</span>{/if}
    </span>
  {/if}
{:else}
  <DropdownMenu align="left" class={className} {items}>
    {#snippet trigger(open)}
      <FieldPill
        {persistent}
        active={open}
        empty={t.tags.length === 0}
        icon={t.tags.length === 0 ? tag : undefined}
        title="Labels"
        class={cn(ghost && !persistent && t.tags.length === 0 && !open && 'opacity-0 transition-opacity group-hover:opacity-100')}
      >
        {#if t.tags.length === 0}
          {ghost && !persistent ? '' : 'Label'}
        {:else}
          <span class="flex items-center gap-1">
            {#each shown as n (n)}
              <LabelChip name={n} {labels} />
            {/each}
            {#if t.tags.length > 2}<span class="text-[10px] text-muted">+{t.tags.length - 2}</span>{/if}
          </span>
        {/if}
      </FieldPill>
    {/snippet}
    {#snippet footer(close)}
      <input
        placeholder="New label; Enter creates"
        onkeydown={(e) => {
          if (e.key !== 'Enter') return
          const name = (e.target as HTMLInputElement).value.trim()
          if (!name) return
          void (async () => {
            if (ctx.boardId) {
              await createBoardLabel(ctx.boardId, name)
              void qc.invalidateQueries({ queryKey: ['board-labels', ctx.boardId] })
            }
            ctx.onPatch({ tags: t.tags.includes(name) ? t.tags : [...t.tags, name] })
            close()
          })()
        }}
        class="w-full bg-transparent text-xs text-fg placeholder:text-muted focus:outline-none"
      />
    {/snippet}
  </DropdownMenu>
{/if}
