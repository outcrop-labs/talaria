<script lang="ts">
  import type { Snippet } from 'svelte'
  import { cn } from '@/lib/cn'
  import Panel from '@/components/ui/Panel.svelte'
  import { focusRing } from './control'

  // The §8 stat block: big sans numeral in readout cream + 10px mono uppercase
  // ink-dim label + optional mono sub line. Reuse for fleet/cost/activity
  // summaries.
  //
  // `href` is the tile that is also a doorway — a stat summarizing a section
  // the reader can jump into (Overview's grid pointing at the observability
  // tabs). Those sites hand-rolled the card as a click target because the
  // primitive could only be a div; as an anchor the whole card is the link,
  // no underline, no hover theatrics — the router's interceptor keeps the
  // trip client-side, and middle-click/open-in-new-tab finally work.
  let {
    label,
    value,
    sub,
    href,
    class: className,
  }: { label: string; value: string | number | Snippet; sub?: string; href?: string; class?: string } = $props()
</script>

<!-- A stat card is a headline number on a page that has three or four of
     them; the corridor gives it material without competing with the value. -->
<Panel
  field="ambient"
  as={href ? 'a' : 'div'}
  {href}
  class={cn('p-5', href && 'block', href && focusRing, className)}
>
  <div class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">{label}</div>
  <div class="mt-1.5 font-sans text-2xl font-semibold text-fg">
    {#if typeof value === 'function'}{@render value()}{:else}{value}{/if}
  </div>
  {#if sub}<div class="mt-1 font-mono text-[11px] text-muted">{sub}</div>{/if}
</Panel>
