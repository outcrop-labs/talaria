<script lang="ts">
  import Markdown from '@/components/ui/Markdown.svelte'
  import { formatTokens } from '@/lib/format'
  import type { AgentDef } from '@/lib/fleet-defs'

  let { def }: { def: AgentDef } = $props()

  const cfg = $derived(def.latest?.config)

  // Compaction, in the same one-line voice as the rows above it. Blank fields
  // are not zero: the honest reading of a def with no compression block is
  // "whatever Hermes does by default", so that is what it says rather than
  // printing a number this agent never authored.
  const compaction = $derived.by(() => {
    const c = def.compression
    const parts: string[] = []
    if (c?.threshold != null) parts.push(`${c.threshold} of context`)
    if (c?.thresholdTokens != null) parts.push(`${formatTokens(c.thresholdTokens)} tokens`)
    return parts.length > 0 ? parts.join(' · ') : 'Hermes default'
  })
</script>

{#snippet field(label: string, value: string)}
  <div class="flex gap-3">
    <span class="w-24 shrink-0 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">{label}</span>
    <span class="min-w-0 flex-1 text-fg">{value}</span>
  </div>
{/snippet}

<div class="space-y-3 text-sm">
  {@render field('Main model', cfg?.main ? `${cfg.main.endpoint} · ${cfg.main.model}` : '—')}
  {@render field('Tiers', cfg?.aliases?.map((a) => a.name).join(', ') || '—')}
  {@render field('Fallbacks', cfg?.fallbacks?.length ? String(cfg.fallbacks.length) : 'none')}
  {@render field('Compaction', compaction)}
  {#if def.latest?.soul}
    <div>
      <div class="mb-1 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Soul</div>
      <div class="max-h-64 overflow-y-auto rounded-lg border border-line p-3 text-xs">
        <Markdown children={def.latest.soul} />
      </div>
    </div>
  {/if}
</div>
