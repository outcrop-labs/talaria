<script lang="ts">
  import WaitingMark from '@/components/ui/WaitingMark.svelte'
  import type { ToolStatus } from '@/lib/sse-parse'

  // Tool in flight = the tool rung (spec §9), sized down to the 11px mono
  // row; done = the quiet green check; failed = the danger ✕. A failure is
  // terminal, so it must never keep the waiting mark — a row still spinning
  // after `tool.failed` promises a result nothing is coming with. The mark is
  // a bare glyph, so it carries the word in the accessibility tree.
  let { status }: { status: ToolStatus } = $props()
</script>

{#if status === 'completed'}
  <span class="text-[color:var(--theme-success)]" role="img" aria-label="done">✓</span>
{:else if status === 'error'}
  <span class="text-[color:var(--theme-danger)]" role="img" aria-label="failed">✕</span>
{:else}
  <WaitingMark site="chat/tool" size={11} class="text-accent" />
{/if}
