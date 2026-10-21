<script lang="ts">
  import { cn } from '@/lib/cn'
  import { compactedSentence, compactedTitle, showCompactedRow } from './compacted-row'
  import type { SessionOverview } from '@/lib/conversations.svelte'

  // Where a pinned session was compacted: the transcript's own divider form (a
  // hairline with a raised pill, the DayDivider shape) in the chrome voice,
  // because this is a fact about the session rather than something anyone
  // said. It sits above the first surviving message as the top of the thread.
  //
  // It takes the whole overview and decides for itself whether it exists — the
  // gate is `showCompactedRow`, tested beside this file, so the host template
  // cannot drift into its own version of "does this apply". The `overview &&`
  // below is the type narrowing, not a second opinion.
  let { overview, class: className }: { overview: SessionOverview | null; class?: string } = $props()
</script>

{#if overview && showCompactedRow(overview)}
  {@const sentence = compactedSentence(overview)}
  <div
    role="separator"
    aria-label={sentence}
    title={compactedTitle(overview)}
    class={cn('relative flex items-center py-1', className)}
  >
    <div class="h-px flex-1 bg-line"></div>
    <!-- Mono, not the day pill's sans: a day label is a date someone reads,
         this is instrumentation. Sentence case stays — the micro-label's
         uppercase would shout a whole sentence. -->
    <span
      class="mx-2 min-w-0 shrink rounded-full border border-line bg-raised px-3 py-0.5 text-center font-mono text-[10px] tracking-[0.02em] text-muted"
    >
      {sentence}
    </span>
    <div class="h-px flex-1 bg-line"></div>
  </div>
{/if}
