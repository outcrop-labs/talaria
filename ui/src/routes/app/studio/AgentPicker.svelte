<script lang="ts">
  import { Lock, Users } from '@lucide/svelte'
  import Avatar from '@/components/ui/Avatar.svelte'
  import { focusRing } from '@/components/ui/control'
  import { cn } from '@/lib/cn'
  import { sansFill } from '@/lib/ui-voice'
  import type { SkillOwner } from '@/lib/skills'

  // Who the Studio is building for: one agent, or every agent at once.
  let {
    agents,
    shared,
    selectedKey,
    roleOf,
    openCount,
    onSelect,
  }: {
    agents: SkillOwner[]
    shared: SkillOwner | undefined
    selectedKey: string
    roleOf: (o: SkillOwner) => string
    /** Open requests for help from this owner. */
    openCount: (o: SkillOwner) => number
    onSelect: (owner: string) => void
  } = $props()
</script>

{#snippet row(owner: string, name: string, sub: string, count: number, locked: boolean)}
  {@const active = selectedKey === owner}
  <button
    type="button"
    onclick={() => onSelect(owner)}
    aria-current={active ? 'true' : undefined}
    class={cn(
      'flex w-full items-center gap-2.5 rounded-md px-2 py-2 text-left transition-colors',
      focusRing,
      active ? sansFill : 'hover:bg-hover',
    )}
  >
    {#if owner === 'shared'}
      <span class="grid h-8 w-8 shrink-0 place-items-center rounded-full border border-line-strong bg-raised text-muted">
        <Users size={16} />
      </span>
    {:else}
      <Avatar {name} class="h-8 w-8" />
    {/if}
    <!-- A name gets the width it needs: it wraps instead of ending in dots. -->
    <span class="min-w-0 flex-1">
      <span class="block break-words font-sans text-sm font-medium leading-5 text-fg">{name}</span>
      {#if sub}<span class="line-clamp-2 font-sans text-[13px] leading-5 text-muted" title={sub}>{sub}</span>{/if}
    </span>
    {#if count > 0}
      <span class="shrink-0 font-sans text-xs tabular-nums text-muted" title={`${count} open ${count === 1 ? 'request' : 'requests'}`}>
        {count}
      </span>
    {/if}
    {#if locked}
      <span class="shrink-0 text-muted" title="View only"><Lock size={14} /></span>
    {/if}
  </button>
{/snippet}

<aside class="space-y-0.5">
  <div class="px-2 pb-1.5 font-sans text-xs leading-4 text-muted">Agents</div>
  {#each agents as o (o.owner)}
    {@render row(o.owner, o.label, roleOf(o), openCount(o), !o.canEdit)}
  {/each}
  <div class="py-2"><div class="border-t border-line-subtle"></div></div>
  {@render row('shared', 'Every agent', 'Skills all agents share', 0, !!shared && !shared.canEdit)}
</aside>
