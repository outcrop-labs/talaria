<script lang="ts">
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import Button from '@/components/ui/Button.svelte'
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import ProviderMark from '@/components/fleet/ProviderMark.svelte'
  import type { LlmEndpoint } from '@/lib/fleet-defs'
  import ModelsEndpointModal from './ModelsEndpointModal.svelte'

  let { ep }: { ep: LlmEndpoint } = $props()

  let managing = $state(false)

  // What is known about the key without a request: a stored one, an env-var
  // fallback, or neither. A cloud provider cannot answer without one; a
  // self-hosted server usually asks for none.
  const key = $derived.by((): { status: 'ok' | 'idle' | 'warn'; text: string } => {
    if (ep.hasKey) return { status: 'ok', text: 'Key stored' }
    if (ep.apiKeyEnv) return { status: 'idle', text: 'Key from environment' }
    return ep.class === 'cloud' ? { status: 'warn', text: 'Needs a key' } : { status: 'idle', text: 'No key' }
  })
</script>

<!-- One provider as a row of the providers card. -->
<div>
  <SettingsRow label={ep.name}>
    {#snippet icon()}<ProviderMark provider={ep.provider} name={ep.name} class="border-0 bg-transparent" />{/snippet}
    {#snippet hint()}
      {ep.models.length} model{ep.models.length === 1 ? '' : 's'}
      {#if ep.baseUrl}· <span class="font-mono text-xs">{ep.baseUrl}</span>{/if}
    {/snippet}
    <span class="flex items-center gap-2 font-sans text-[13px] leading-5 text-muted">
      <StatusDot status={key.status} />{key.text}
    </span>
    <Button variant="outline" size="sm" onclick={() => (managing = true)}>Manage</Button>
  </SettingsRow>
  {#if managing}
    <ModelsEndpointModal {ep} onClose={() => (managing = false)} />
  {/if}
</div>
