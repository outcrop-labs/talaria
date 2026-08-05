<script lang="ts">
  import { UserRound } from '@lucide/svelte'
  import Avatar from '@/components/ui/Avatar.svelte'
  import DropdownMenu from '@/components/ui/DropdownMenu.svelte'
  import FieldPill from '@/components/ui/FieldPill.svelte'
  import type { ContextMenuEntry } from '@/components/ui/context-menu.svelte'
  import { cn } from '@/lib/cn'
  import { assigneeInfo, userAssignee } from '@/lib/assignees'
  import type { Task } from '@/lib/task-const'
  import type { PillCtx } from './field-pills'

  let {
    t,
    ctx,
    class: className,
    ghost,
    persistent,
  }: { t: Task; ctx: PillCtx; class?: string; ghost?: boolean; persistent?: boolean } = $props()

  const infos = $derived(t.assignees.map((a) => assigneeInfo(a, ctx.agents, ctx.members)))
  const summary = $derived(
    infos.length === 0 ? 'Assign' : infos.length === 1 ? infos[0]!.label : `${infos.length} assignees`,
  )

  const toggle = (key: string) =>
    ctx.onPatch({ assignees: t.assignees.includes(key) ? t.assignees.filter((a) => a !== key) : [...t.assignees, key] })

  const items = (): ContextMenuEntry[] => [
    ...ctx.members.map((m) => {
      const key = userAssignee(m.userId)
      return {
        label: (m.name ?? m.email ?? 'teammate') + (m.userId === ctx.meId ? ' (me)' : ''),
        checked: t.assignees.includes(key),
        keepOpen: true,
        onSelect: () => toggle(key),
      }
    }),
    ...(ctx.members.length && ctx.agents.length ? (['sep'] as ContextMenuEntry[]) : []),
    ...ctx.agents.map((a) => ({
      label: a.label,
      checked: t.assignees.includes(a.id),
      keepOpen: true,
      onSelect: () => toggle(a.id),
    })),
  ]
</script>

{#snippet avatars()}
  <span class="flex items-center gap-1.5">
    {#if infos.length > 0}
      <span class="flex -space-x-1.5">
        {#each infos.slice(0, 3) as a (a.key)}
          <Avatar name={a.label} class="h-4.5 w-4.5 ring-2 ring-[color:var(--theme-panel)]" />
        {/each}
      </span>
    {/if}
    <span class="min-w-0 truncate">{summary}</span>
  </span>
{/snippet}

{#snippet user()}
  <UserRound size={11} />
{/snippet}

{#if !ctx.canEdit}
  {#if infos.length > 0}
    <span class={cn('inline-flex items-center font-mono text-[10px] uppercase tracking-[0.05em] text-muted', className)}>
      {@render avatars()}
    </span>
  {/if}
{:else}
  <DropdownMenu align="left" class={className} {items}>
    {#snippet trigger(open)}
      <FieldPill
        {persistent}
        active={open}
        empty={infos.length === 0}
        icon={infos.length === 0 ? user : undefined}
        title="Assign teammates or agents"
        class={cn(ghost && !persistent && infos.length === 0 && !open && 'opacity-0 transition-opacity group-hover:opacity-100')}
      >
        {#if !(ghost && !persistent && infos.length === 0)}{@render avatars()}{/if}
      </FieldPill>
    {/snippet}
  </DropdownMenu>
{/if}
