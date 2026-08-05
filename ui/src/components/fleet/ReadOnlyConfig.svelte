<script lang="ts">
  import Markdown from '@/components/ui/Markdown.svelte'
  import type { AgentDef } from '@/lib/fleet-defs'

  let { def }: { def: AgentDef } = $props()

  const cfg = $derived(def.latest?.config)
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
  {#if def.latest?.soul}
    <div>
      <div class="mb-1 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Soul</div>
      <div class="max-h-64 overflow-y-auto rounded-lg border border-line p-3 text-xs">
        <Markdown children={def.latest.soul} />
      </div>
    </div>
  {/if}
</div>
