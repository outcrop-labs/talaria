<script lang="ts">
  import type { Snippet } from 'svelte'
  import Brand from '@/components/Brand.svelte'

  // Chrome for publicly shared KB pages (docs + folders). No app nav, no auth —
  // just a clean, centered document with the Talaria mark (spec §4). The content
  // column uses the same comfortable measure as the in-app read view.
  let { children, meta }: { children: Snippet; meta?: string } = $props()
</script>

<div class="min-h-screen bg-surface text-fg">
  <header class="border-b border-line-subtle">
    <div class="mx-auto flex max-w-[46rem] items-center gap-2 px-6 py-3">
      <Brand />
      {#if meta}<span class="ml-auto font-mono text-[10px] uppercase tracking-[0.08em] text-muted">{meta}</span>{/if}
    </div>
  </header>
  <main class="mx-auto max-w-[46rem] px-6 py-12 font-sans text-[0.95rem] leading-[1.7]">{@render children()}</main>
  <footer class="mx-auto max-w-[46rem] px-6 pb-12 pt-6 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">
    Shared from a Talaria knowledgebase
  </footer>
</div>
