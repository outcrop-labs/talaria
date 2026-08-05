<script lang="ts">
  import type { Snippet } from 'svelte'

  /** Rail section label ("CHANNELS", "AGENTS") with optional icon action. */
  let {
    label,
    action,
    children,
  }: { label: string; action?: Snippet; children: Snippet } = $props()
</script>

<div class="mb-4">
  <div class="mb-1 flex items-center px-2">
    <span class="flex-1 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">{label}</span>
    {@render action?.()}
  </div>
  <ul class="space-y-0.5">{@render children()}</ul>
</div>
