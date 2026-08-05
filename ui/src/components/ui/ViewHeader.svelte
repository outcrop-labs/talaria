<script lang="ts">
  import type { Snippet } from 'svelte'
  import { cn } from '@/lib/cn'
  import InfoTip from './InfoTip.svelte'

  // The one view header. Every full-page view opens with this — title voice,
  // spacing, and control placement are decided HERE, not re-derived per view
  // (the audit found three different gap scales and two different ways of
  // right-aligning the same buttons).
  //
  //   <ViewHeader title="MCP" info="Model Context Protocol servers…">
  //     {#snippet status()}<span>3/5 online</span>{/snippet}   ← mono readout beside the title
  //     {#snippet actions()}<Button …>New</Button>{/snippet}   ← right-aligned controls
  //   </ViewHeader>
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
    /** InfoTip copy shown beside the title. */
    info?: string
    /** One muted sentence under the title row. */
    blurb?: string | Snippet
    /** Mono metadata beside the title (counts, health) — the caller keeps a
     *  stable slot (skeleton/error state) so the row never jogs. */
    status?: Snippet
    /** Right-aligned controls: buttons, pickers, search. */
    actions?: Snippet
    class?: string
  } = $props()
</script>

<header class={cn('space-y-1', className)}>
  <div class="flex flex-wrap items-center gap-3">
    <div class="flex items-center gap-1.5">
      <h1 class="font-sans text-2xl font-semibold tracking-tight text-fg">{title}</h1>
      {#if info}<InfoTip text={info} />{/if}
    </div>
    {#if status}
      <div class="font-mono text-[11px] tracking-[0.05em] text-muted">{@render status()}</div>
    {/if}
    {#if actions}
      <div class="ml-auto flex flex-wrap items-center gap-2">{@render actions()}</div>
    {/if}
  </div>
  {#if blurb}
    <p class="font-sans text-xs text-muted">
      {#if typeof blurb === 'string'}{blurb}{:else}{@render blurb()}{/if}
    </p>
  {/if}
</header>
