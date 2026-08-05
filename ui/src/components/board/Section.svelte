<script lang="ts">
  import type { Snippet } from 'svelte'

  // Labeled block in the ticket's content column (module-private helper of
  // task-detail in React; its own file here).
  let { label, children }: { label: string; children: Snippet } = $props()
</script>

<div>
  <div class="mb-2 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">{label}</div>
  {@render children()}
</div>
