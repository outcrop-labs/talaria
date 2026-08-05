<script lang="ts">
  import type { Snippet } from 'svelte'
  import { cn } from '@/lib/cn'

  // One agent-reported result field (Outcome / Resolution / Error) on the ticket.
  let { title, danger, children }: { title: string; danger?: boolean; children: Snippet } = $props()
</script>

<div class={cn('mb-2 rounded-lg border bg-card p-2', danger ? 'border-danger/40' : 'border-line')}>
  <div class={cn('mb-0.5 font-mono text-[10px] uppercase tracking-[0.08em]', danger ? 'text-danger' : 'text-ink-dim')}>{title}</div>
  <div class="whitespace-pre-wrap font-sans text-sm text-fg">{@render children()}</div>
</div>
