<script lang="ts">
  import type { JudgeReview } from '@/lib/boards.svelte'

  // The QA judge's advisory verdict, shown above the human approval gate.
  let { review }: { review: JudgeReview } = $props()

  const tone = $derived(
    review.verdict === 'pass'
      ? { color: 'var(--theme-success)', label: 'Pass' }
      : review.verdict === 'revise'
        ? { color: 'var(--theme-warning)', label: 'Revise' }
        : { color: 'var(--theme-danger)', label: 'Escalate' },
  )
</script>

<div class="mb-2 rounded-lg border border-line bg-card p-2.5 text-sm">
  <div class="mb-1 flex items-center gap-2">
    <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">QA judge</span>
    <span
      class="rounded px-1.5 py-0.5 font-mono text-[10px] font-medium uppercase tracking-[0.05em]"
      style:color={tone.color}
      style:border={`1px solid ${tone.color}`}>{tone.label}</span
    >
    {#if review.model}<span class="font-mono text-[10px] tracking-[0.05em] text-muted">{review.model}</span>{/if}
    <span class="ml-auto font-mono text-[10px] uppercase tracking-[0.05em] text-muted">advisory</span>
  </div>
  {#if review.summary}<div class="font-sans text-fg">{review.summary}</div>{/if}
  {#if review.issues.length > 0}
    <ul class="mt-1.5 list-disc space-y-0.5 pl-4 font-sans text-[13px] text-muted">
      {#each review.issues as i, n (n)}
        <li>{i}</li>
      {/each}
    </ul>
  {/if}
</div>
