<script lang="ts">
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
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

<SettingsRow
  label="No-train routing"
  hint={ep.provider === 'openrouter'
    ? 'Only US providers that do not store requests, with data collection denied.'
    : 'Asks the provider not to collect data, where it honors the request.'}
>
  <Toggle checked={on} onChange={toggle} title="No-train routing" />
</SettingsRow>
