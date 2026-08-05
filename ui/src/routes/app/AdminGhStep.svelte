<script lang="ts">
  import type { Snippet } from 'svelte'

  /** One numbered setup step — a small circle, a title, then its content. */
  let { n, title, children }: { n: number; title: string; children: Snippet } = $props()
</script>

<div class="flex gap-3">
  <span class="mt-0.5 flex h-5 w-5 shrink-0 items-center justify-center rounded-full border border-[var(--theme-accent-border)] font-mono text-[10px] text-accent">{n}</span>
  <div class="min-w-0 flex-1 space-y-2">
    <div class="text-sm font-medium text-fg">{title}</div>
    {@render children()}
  </div>
</div>
