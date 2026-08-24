<script lang="ts">
  import type { Snippet } from 'svelte'
  import { cn } from '@/lib/cn'
  import { claimViewTitle } from '@/lib/view-title.svelte'

  // The one view header. The TITLE (and its InfoTip) live in the top strip
  // now — this component claims them there on mount, so title voice is still
  // decided HERE, not re-derived per view — and renders what stays in the
  // body: the status readout, the right-aligned controls, and the blurb.
  //
  //   <ViewHeader title="MCP" info="Model Context Protocol servers…">
  //     {#snippet status()}<span>3/5 online</span>{/snippet}   ← mono readout row
  //     {#snippet actions()}<Button …>New</Button>{/snippet}   ← right-aligned controls
  //   </ViewHeader>
  //
  // A view whose header was ONLY a title claims it directly with
  // claimViewTitle() instead — rendering an empty header element buys
  // nothing, and an invisible node inside a `space-y` still takes its turn
  // in the rhythm.
  //
  // Tabs are NOT part of the header — they render as the next sibling section
  // (they're their own grammar row for the stagger cascade). As a child of a
  // `use:staggerIn` container the header rises as one block.
  let {
    title,
    info,
    blurb,
    status,
    actions,
    class: className,
  }: {
    title: string
    /** InfoTip copy shown beside the title in the top strip. */
    info?: string
    /** One muted sentence under the controls row. */
    blurb?: string | Snippet
    /** Mono metadata (counts, health) — the caller keeps a stable slot
     *  (skeleton/error state) so the row never jogs. */
    status?: Snippet
    /** Right-aligned controls: buttons, pickers, search. */
    actions?: Snippet
    class?: string
  } = $props()

  claimViewTitle(title, { info })
</script>

{#if status || actions || blurb}
  <header class={cn('space-y-1', className)}>
    {#if status || actions}
      <div class="flex flex-wrap items-center gap-3">
        {#if status}
          <div class="font-mono text-[11px] tracking-[0.05em] text-muted">{@render status()}</div>
        {/if}
        {#if actions}
          <div class="ml-auto flex flex-wrap items-center gap-2">{@render actions()}</div>
        {/if}
      </div>
    {/if}
    {#if blurb}
      <p class="font-sans text-xs text-muted">
        {#if typeof blurb === 'string'}{blurb}{:else}{@render blurb()}{/if}
      </p>
    {/if}
  </header>
{/if}
