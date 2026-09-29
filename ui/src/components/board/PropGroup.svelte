<script lang="ts">
  import type { Snippet } from 'svelte'
  import { cn } from '@/lib/cn'

  // A titled group of fields in the ticket's properties rail.
  //
  // The rail used to be a flat run of fifteen `Prop` blocks — status, priority,
  // color, assignees, effort, estimate, time, tokens, dates, sub-tasks,
  // dependencies, labels, watchers, timestamps — all at one visual level, in
  // one scroll, with nothing to tell a reader that "due date" and "blocked by"
  // answer different questions. Finding a field meant reading every label.
  //
  // Groups give the rail a shape you can learn: the ticket's identity at the
  // top with no heading (you are looking at it), then Planning, Relationships,
  // Classification and the rest, each behind a hairline and a micro-label. The
  // heading is the house micro-label (UI-CONVENTIONS "Rows, chips, dots"), one
  // size for every section in the app.
  let {
    label,
    children,
    class: className,
  }: { label?: string; children: Snippet; class?: string } = $props()
</script>

<section class={cn('space-y-3 border-t border-line-subtle pt-4 first:border-t-0 first:pt-0', className)}>
  {#if label}
    <h3 class="font-mono text-[10px] font-semibold uppercase tracking-wide text-muted">{label}</h3>
  {/if}
  {@render children()}
</section>
