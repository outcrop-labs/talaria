<script lang="ts">
  import type { Snippet } from 'svelte'
  import type { InboxGlyph } from '@/lib/inbox-board'

  let { glyph }: { glyph: InboxGlyph } = $props()

  const GLYPHS: Record<InboxGlyph, { tone: string; draw: Snippet }> = {
    running: { tone: 'text-[color:var(--theme-chart-1)]', draw: running },
    done: { tone: 'text-[color:var(--theme-success)]', draw: done },
    failed: { tone: 'text-[color:var(--theme-danger)]', draw: failed },
    blocked: { tone: 'text-[color:var(--theme-danger)]', draw: blocked },
    signoff: { tone: 'text-[color:var(--theme-warning)]', draw: signoff },
    triage: { tone: 'text-muted', draw: triage },
    waiting: { tone: 'text-[color:var(--theme-warning)]', draw: waiting },
  }
  const mark = $derived(GLYPHS[glyph])
</script>

{#snippet ring()}<circle cx="8" cy="8" r="6.2" />{/snippet}

{#snippet running()}<path d="M8 1.8A6.2 6.2 0 1 1 1.8 8" />{/snippet}

{#snippet done()}
  {@render ring()}
  <path d="m5.2 8.3 1.9 1.9 3.7-4" />
{/snippet}

{#snippet failed()}
  {@render ring()}
  <path d="M8 4.8v3.6M8 11v.1" />
{/snippet}

{#snippet blocked()}
  {@render ring()}
  <path d="M5.2 8h5.6" />
{/snippet}

{#snippet signoff()}
  {@render ring()}
  <path d="M8 4.7V8l2.3 1.5" />
{/snippet}

{#snippet triage()}<path d="M8 1.5 14.5 8 8 14.5 1.5 8Z" />{/snippet}

{#snippet waiting()}
  {@render ring()}
  <path d="M8 8v.1" />
{/snippet}

<svg
  width="16"
  height="16"
  viewBox="0 0 16 16"
  aria-hidden="true"
  fill="none"
  stroke="currentColor"
  stroke-width="1.6"
  stroke-linecap="round"
  stroke-linejoin="round"
  class="shrink-0 {mark.tone}"
>
  {@render mark.draw()}
</svg>
