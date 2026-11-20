<script lang="ts">
  import Chip from '@/components/ui/Chip.svelte'
  import { remoteStateLabel } from '@/lib/board-sync'

  // A synced ticket whose ClickUp task is no longer active there — archived,
  // deleted, moved to another list, or out of the sync token's sight. The sync
  // MARKS such a ticket and never deletes or moves it (docs/BOARD-SYNC.md), so
  // this chip is how a card, a list row and the ticket itself say so. Renders
  // nothing for an active task or a ticket that is not synced.
  let { state, class: className }: { state: string | null | undefined; class?: string } = $props()
  const label = $derived(remoteStateLabel(state))
</script>

{#if label}
  <Chip tone="warn" class={className} title={`${label}. The sync marks this ticket and never deletes it.`}>{label}</Chip>
{/if}
