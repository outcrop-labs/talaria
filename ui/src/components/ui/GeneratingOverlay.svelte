<script lang="ts">
  import { fade, QUICK } from '@/lib/motion'
  import Generating from './Generating.svelte'

  // A veil over content being rewritten in place (e.g. the plan document while
  // the agent resyncs it). Parent must be `relative`.
  let { label }: { label: string } = $props()
</script>

<!-- |global: the overlay IS the component root — call sites render
     {#if busy}<GeneratingOverlay/>, so local legs never play (ANIMATIONS.md). -->
<div in:fade|global={{ duration: 150 }} out:fade|global={QUICK} class="absolute inset-0 z-10 grid place-items-center bg-surface/60 backdrop-blur-[2px]">
  <Generating {label} lines={4} class="w-72 bg-panel shadow-[var(--theme-shadow-2)]" />
</div>
