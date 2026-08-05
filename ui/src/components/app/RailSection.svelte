<script lang="ts">
  import type { Snippet } from 'svelte'
  import { listStagger } from '@/lib/motion'

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
  <!-- div, not ul: RailRow renders div rows (Materialize-friendly geometry). -->
  <div class="space-y-0.5" use:listStagger>{@render children()}</div>
</div>
