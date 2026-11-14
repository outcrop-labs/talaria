<script lang="ts">
  import { KeyRound, SlidersHorizontal } from '@lucide/svelte'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import SettingsRowFix from '@/components/settings/SettingsRowFix.svelte'
  import SettingsRowMenu from '@/components/settings/SettingsRowMenu.svelte'
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import type { MenuIcon } from '@/components/ui/context-menu.svelte'
  import ProviderMark from '@/components/fleet/ProviderMark.svelte'
  import type { LlmEndpoint } from '@/lib/fleet-defs'
  import ModelsEndpointModal from './ModelsEndpointModal.svelte'

  let { ep }: { ep: LlmEndpoint } = $props()

  // 'key' opens the dialog with the cursor in the key field.
  let managing = $state<false | 'manage' | 'key'>(false)

  // What is known about the key without a request: a stored one, an env-var
  // fallback, or neither. A cloud provider cannot answer without one, which
  // is the state that needs a person. A self-hosted server usually asks for
  // none, and that is a settled state, not a missing one.
  const needsKey = $derived(!ep.hasKey && !ep.apiKeyEnv && ep.class === 'cloud')
  const key = $derived.by((): { status: 'ok' | 'idle' | 'warn'; text: string } => {
    if (ep.hasKey) return { status: 'ok', text: 'Key stored' }
    if (ep.apiKeyEnv) return { status: 'idle', text: 'Key from environment' }
    return needsKey ? { status: 'warn', text: 'Needs a key' } : { status: 'idle', text: 'No key required' }
  })
  // A known cloud provider's address says nothing its name does not. A server
  // of your own is told apart by where it is.
  const showsUrl = $derived(!!ep.baseUrl && (ep.class === 'local' || ep.provider === 'custom'))
</script>

<!-- One provider as a row of the providers card. The row opens its dialog, and
     Fix shows only when the provider cannot answer without a key. -->
<div>
  <SettingsRow label={ep.name} onOpen={() => (managing = 'manage')} openLabel={`Manage ${ep.name}`}>
    {#snippet icon()}<ProviderMark provider={ep.provider} name={ep.name} class="border-0 bg-transparent" />{/snippet}
    {#snippet hint()}
      {ep.models.length} model{ep.models.length === 1 ? '' : 's'}
      {#if showsUrl}· <span class="font-mono text-xs">{ep.baseUrl}</span>{/if}
    {/snippet}
    {#snippet status()}<StatusDot status={key.status} />{key.text}{/snippet}
    {#if needsKey}<SettingsRowFix label={`Fix ${ep.name}: add a key`} onclick={() => (managing = 'key')} />{/if}
    <SettingsRowMenu
      label={`Actions for ${ep.name}`}
      items={[
        { label: 'Manage', icon: [SlidersHorizontal, { size: 14 }], onSelect: () => (managing = 'manage') },
        ...(needsKey ? [{ label: 'Add key', icon: [KeyRound, { size: 14 }] as MenuIcon, onSelect: () => (managing = 'key') }] : []),
      ]}
    />
  </SettingsRow>
  {#if managing}
    <ModelsEndpointModal {ep} focusKey={managing === 'key'} onClose={() => (managing = false)} />
  {/if}
</div>
