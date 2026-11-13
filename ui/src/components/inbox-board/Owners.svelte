<script lang="ts">
  import { Bot } from '@lucide/svelte'
  import type { InboxPerson } from '@/lib/inbox-board'
  import PersonMark from './PersonMark.svelte'

  let { people, size = 'sm' }: { people: readonly InboxPerson[]; size?: 'sm' | 'md' } = $props()
</script>

{#each people as person, i (person.name)}
  {#if i > 0}<span aria-hidden="true">,&nbsp;</span>{/if}
  <span class="inline-flex min-w-0 items-center gap-1 align-bottom">
    {#if person.agent}
      <span class="shrink-0 text-muted" title="Agent"><Bot size={16} aria-hidden="true" /></span>
    {:else}
      <PersonMark name={person.name} {size} />
    {/if}
    <span class="truncate">{person.name}</span>
  </span>
{:else}
  <span class="truncate text-muted">No owner</span>
{/each}
