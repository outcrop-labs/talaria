<script lang="ts">
  import DitherLayer from '@/components/ui/DitherLayer.svelte'
  import Markdown from '@/components/ui/Markdown.svelte'
  import type { DitherSource } from '@/lib/dither'
  import { dateLabel, clockLabel, type BriefView } from './daily-brief.svelte'

  /**
   * The head of the document: the day, the lede, and who wrote it.
   *
   * THE FIELD IS DITHER, NOT A GRADIENT. The playground sketch put a wave
   * canvas behind this heading; Mercury forbids glow and blur, which leaves a
   * flat fill and nothing else — so the house substitute is the Bayer field
   * (`lib/dither.ts`), where the gradient exists statistically and no light is
   * ever painted. A horizontal crest under a top edge reads as depth.
   *
   * THE CREST HOLDS STILL (speed 0). It drifted at 9 px/s, which kept the
   * engine re-evaluating the whole field about twelve times a second for as
   * long as anyone sat on Home: measured in Chromium at 1440×900, 21% of the
   * main thread and 63k dots rewritten a second, on a document that is never
   * rewritten. A still wave is the same bands, painted once (N11).
   *
   * THE LEDE IS NEVER REWRITTEN, which is why it is rendered plainly and
   * without a regenerating affordance. There is deliberately no "refresh" here:
   * the control would imply the document can be re-asked, and the entire
   * premise is that it cannot.
   */
  let { brief }: { brief: BriefView } = $props()

  // Ambient, not decorative-loud: one drifting crest plus an edge, both well
  // under half strength, so the words stay the brightest thing in the box.
  const FIELD: DitherSource[] = [
    // Strengths and alphas tuned to the median look this still field had once
    // its entrance had built it up (N11): see LIVE_FIELD_TUNING in
    // lib/dither-engine.ts.
    { id: 'crest', kind: 'wave', axis: 'x', wavelength: 320, speed: 0, strength: 0.34, tone: 'accent' },
    { id: 'top', kind: 'edge', side: 'top', depth: 120, strength: 0.225 },
    { id: 'grain', kind: 'uniform', strength: 0.045 },
  ]
</script>

<header class="relative overflow-hidden rounded-lg border border-line bg-surface px-7 py-7">
  <DitherLayer sources={FIELD} pitch={4} dot={1.6} organic={0.6} alphaFloor={0.0845} maxAlpha={0.676} />

  <div class="relative">
    <div class="flex flex-wrap items-baseline gap-x-3 gap-y-1">
      <h1 class="font-sans text-[22px] font-semibold leading-tight text-fg">
        {dateLabel(brief.date, brief.zone)}
      </h1>
      <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">
        Opened {clockLabel(brief.openedAt, brief.zone)}
      </span>
    </div>

    {#if brief.lede}
      <!-- Sans, because this is the one paragraph on the page somebody READS
           rather than scans. Markdown because the assistant writes bold leads
           into it and a literal `**` would be the model's formatting leaking. -->
      <div class="mt-3 max-w-[62ch] font-sans text-[14px] leading-6 text-fg">
        <Markdown children={brief.lede} />
      </div>
    {/if}

    <div class="mt-4 font-mono text-[10px] uppercase tracking-[0.08em] text-muted">
      Written by {brief.agent.name ?? 'your assistant'} · appended to through the day, never rewritten
    </div>
  </div>
</header>
