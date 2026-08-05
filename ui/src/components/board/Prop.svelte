<script lang="ts">
  import type { Snippet } from 'svelte'

  // Labeled field in the ticket's properties rail (module-private helper of
  // task-detail in React; its own file here).
  let { label, children }: { label: string; children: Snippet } = $props()
</script>

<div>
  <div class="mb-1 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">{label}</div>
  {@render children()}
</div>
