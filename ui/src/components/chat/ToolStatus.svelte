<script lang="ts">
  import GeneratingBars from '@/components/ui/GeneratingBars.svelte'

  // Tool in flight = STAGE SCAN on the standard budget (spec §9), scaled to
  // the 11px mono row; done = the quiet green check.
  let { status }: { status: 'running' | 'completed' } = $props()
</script>

{#if status === 'completed'}
  <span class="text-[color:var(--theme-success)]">✓</span>
{:else}
  <GeneratingBars bars={4} variant="scan" step={0.18} class="text-accent [&>span]:h-[9px]" />
{/if}
