<script lang="ts">
  import Toggle from '@/components/ui/Toggle.svelte'
  import { patchEndpoint } from '@/lib/models'
  import type { LlmEndpoint } from '@/lib/fleet-defs'
  import { GENERIC_NO_TRAIN, OPENROUTER_NO_TRAIN } from './models'

  // Privacy routing as a SETTING (not a forced default): admins opt an endpoint
  // into no-train routing, which Talaria merges into every call to that backend.
  let { ep, run }: { ep: LlmEndpoint; run: (p: Promise<{ error?: string }>) => Promise<void> } = $props()

  const on = $derived(
    Boolean((ep.requestDefaults as { provider?: { data_collection?: string } } | undefined)?.provider?.data_collection === 'deny'),
  )
  const toggle = () => {
    const next = on ? {} : ep.provider === 'openrouter' ? OPENROUTER_NO_TRAIN : GENERIC_NO_TRAIN
    void run(patchEndpoint(ep.id, { requestDefaults: next }))
  }
</script>

<div class="mt-4 flex items-start gap-2.5 border-t border-line pt-3">
  <!-- The §8 toggle primitive: gold knob on a warm track when on. -->
  <Toggle checked={on} onChange={toggle} class="mt-0.5 shrink-0" />
  <div class="min-w-0">
    <div class="text-xs font-medium text-fg">No-train routing {#if on}<span class="font-mono text-[10px] uppercase tracking-[0.05em] text-success">· on</span>{/if}</div>
    <div class="font-sans text-[11px] text-muted">
      {ep.provider === 'openrouter'
        ? 'Restrict to US, no-store provider pools and deny data collection on every request.'
        : 'Send data_collection: deny with every request (honored where the provider supports it).'}
    </div>
  </div>
</div>
