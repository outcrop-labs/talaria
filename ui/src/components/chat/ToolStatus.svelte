<script lang="ts">
  import WaitingMark from '@/components/ui/WaitingMark.svelte'

  // Tool in flight = the tool rung (spec §9), sized down to the 11px mono
  // row; done = the quiet green check.
  let { status }: { status: 'running' | 'completed' } = $props()
</script>

{#if status === 'completed'}
  <span class="text-[color:var(--theme-success)]">✓</span>
{:else}
  <WaitingMark site="chat/tool" size={11} class="text-accent" />
{/if}
