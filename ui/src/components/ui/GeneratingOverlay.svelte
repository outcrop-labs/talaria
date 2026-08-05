<script lang="ts">
  import Generating from './Generating.svelte'

  // A veil over content being rewritten in place (e.g. the plan document while
  // the agent resyncs it). Parent must be `relative`.
  let { label }: { label: string } = $props()
</script>

<div class="absolute inset-0 z-10 grid place-items-center bg-surface/60 backdrop-blur-[2px]">
  <Generating {label} lines={4} class="w-72 bg-panel shadow-[var(--theme-shadow-2)]" />
</div>
