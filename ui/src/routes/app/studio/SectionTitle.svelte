<script lang="ts">
  import type { Snippet } from 'svelte'

  // §8 section header: 10px mono uppercase ink-dim label, prose hint beside it.
  let { title, hint, action }: { title: string; hint: string; action?: Snippet } = $props()
</script>

<div class="mb-2 flex min-h-6 items-center gap-2">
  <h3 class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">{title}</h3>
  <span class="min-w-0 truncate font-sans text-xs text-muted">{hint}</span>
  {#if action}<span class="ml-auto">{@render action()}</span>{/if}
</div>
