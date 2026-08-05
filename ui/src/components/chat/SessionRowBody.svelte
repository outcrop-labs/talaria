<script lang="ts">
  import { cn } from '@/lib/cn'
  import { relativeTime } from '@/lib/fleet'
  import type { Conversation } from '@/lib/conversations.svelte'

  /** §10 session row content: [status dot] [13px title] [mono owner] [mono time].
   *  Shared: the Plan rail and the Comms agent-thread rows are both session
   *  lists — one anatomy, one component. */
  let { conv, active }: { conv: Conversation; active: boolean } = $props()

  // Semantic first (spec §1): failure orange, working green; the active row's
  // dot reads gold, idle rows stay dim ink.
  const dot = $derived(
    conv.failed
      ? 'var(--theme-danger)'
      : conv.working
        ? 'var(--theme-success)'
        : active
          ? 'var(--theme-accent)'
          : 'var(--theme-ink-dim)',
  )
</script>

<span
  aria-hidden="true"
  class={cn('h-[6px] w-[6px] shrink-0 rounded-full', conv.working && !conv.failed && 'gd-breathe')}
  style:background-color={dot}
></span>
<!-- A working session is live background state — MONITOR BREATHE on the
     ambient budget (spec §9); idle/failed dots stay still. -->
<span class="min-w-0 flex-1 truncate font-sans text-[13px]">{conv.title || 'Untitled'}</span>
{#if conv.ownerLabel}
  <span class="max-w-16 shrink-0 truncate font-mono text-[10px] tracking-[0.05em] text-muted">
    {conv.ownerLabel}
  </span>
{/if}
<span class="shrink-0 font-mono text-[10px] tracking-[0.05em] text-ink-dim">
  {relativeTime(conv.updatedAt)}
</span>
