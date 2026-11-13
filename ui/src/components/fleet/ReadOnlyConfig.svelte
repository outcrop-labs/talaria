<script lang="ts">
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsSection from '@/components/settings/SettingsSection.svelte'
  import Markdown from '@/components/ui/Markdown.svelte'
  import { formatTokens } from '@/lib/format'
  import type { AgentDef } from '@/lib/fleet-defs'
  import { useModelNames } from '@/components/models/model-names.svelte'

  let { def }: { def: AgentDef } = $props()

  const cfg = $derived(def.latest?.config)
  const modelName = useModelNames()
  const mainName = $derived(modelName(cfg?.main?.model))

  // Compaction, in the same one-line voice as the rows above it. Blank fields
  // are not zero: the honest reading of a def with no compression block is
  // "whatever Hermes does by default", so that is what it says rather than
  // printing a number this agent never authored.
  const compaction = $derived.by(() => {
    const c = def.compression
    const parts: string[] = []
    if (c?.threshold != null) parts.push(`${c.threshold} of context`)
    if (c?.thresholdTokens != null) parts.push(`${formatTokens(c.thresholdTokens)} tokens`)
    return parts.length > 0 ? parts.join(' · ') : 'Default'
  })
</script>

{#snippet field(label: string, value: string, mono = false)}
  <dt class="font-sans text-[13px] leading-5 text-muted">{label}</dt>
  <dd class={mono ? 'min-w-0 font-mono text-xs leading-5 text-fg' : 'min-w-0 font-sans text-[13px] leading-5 text-fg'}>{value}</dd>
{/snippet}

<div class="space-y-8">
  <SettingsSection title="Models" description="Only this agent's managers can change its configuration.">
    <SettingsGroup>
      <dl class="grid grid-cols-[10rem_minmax(0,1fr)] gap-x-4 gap-y-2 px-5 py-4">
        {@render field('Main model', cfg?.main ? `${mainName ?? cfg.main.model} · ${cfg.main.endpoint}` : 'Not configured yet', !!cfg?.main && !mainName)}
        {@render field('Model tiers', cfg?.aliases?.map((a) => a.name).join(', ') || 'None')}
        {@render field('Fallbacks', cfg?.fallbacks?.length ? String(cfg.fallbacks.length) : 'None')}
        {@render field('Compaction', compaction)}
      </dl>
    </SettingsGroup>
  </SettingsSection>
  {#if def.latest?.soul}
    <SettingsSection title="Instructions">
      <SettingsGroup>
        <div class="max-h-72 overflow-y-auto px-5 py-4 font-sans text-[13px] leading-5">
          <Markdown children={def.latest.soul} />
        </div>
      </SettingsGroup>
    </SettingsSection>
  {/if}
</div>
