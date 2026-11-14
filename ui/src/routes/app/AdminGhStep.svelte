<script lang="ts">
  import type { Snippet } from 'svelte'

  /** One numbered setup step — a small circle, a title, then its content. */
  let { n, title, children }: { n: number; title: string; children: Snippet } = $props()
</script>

<div class="flex gap-3">
  <span class="grid h-5 w-5 shrink-0 place-items-center rounded-full border border-line font-sans text-xs font-medium tabular-nums text-muted">{n}</span>
  <div class="min-w-0 flex-1 space-y-3">
    <div class="font-sans text-sm font-medium leading-5 text-fg">{title}</div>
    {@render children()}
  </div>
</div>
