<script lang="ts">
  import { createRawSnippet, mount, unmount, type Snippet } from 'svelte'
  import { useQueryClient } from '@tanstack/svelte-query'
  import Button from '@/components/ui/Button.svelte'
  import Combobox from '@/components/ui/Combobox.svelte'
  import SettingsField from '@/components/settings/SettingsField.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Modal from '@/components/ui/Modal.svelte'
  import ProviderMark from '@/components/fleet/ProviderMark.svelte'
  import { addEndpoint, inferClass, PROVIDER_PRESETS } from '@/lib/models'

  let { open, onClose, onAdded }: { open: boolean; onClose: () => void; onAdded: (id: string) => void } = $props()

  // ComboOption.icon is a zero-arg snippet; bridge a <ProviderMark> into one
  // (createRawSnippet renders a shell, the component mounts inside it).
  const markIcon = (provider: string, name?: string): Snippet =>
    createRawSnippet(() => ({
      render: () => '<span style="display:contents"></span>',
      setup(target) {
        const mark = mount(ProviderMark, { target, props: { provider, name } })
        return () => unmount(mark)
      },
    }))

  const qc = useQueryClient()
  let presetKey = $state(PROVIDER_PRESETS[0]!.key)
  const preset = $derived(PROVIDER_PRESETS.find((p) => p.key === presetKey)!)
  let name = $state('')
  let baseUrl = $state('')
  let apiKeyEnv = $state('')
  let apiKey = $state('')
  let busy = $state(false)
  let err = $state<string | null>(null)

  const create = async () => {
    err = null
    busy = true
    try {
      const url = (preset.configurableUrl ? baseUrl.trim() || preset.baseUrl : preset.baseUrl) ?? null
      // No seed models — the endpoint starts empty and the manage modal opens
      // next, where models come from the provider's LIVE catalog.
      const r = await addEndpoint({
        name: name.trim() || preset.key,
        provider: preset.provider,
        baseUrl: url,
        // Users never pick local/cloud — known providers carry it, custom infers from the URL.
        class: preset.configurableUrl ? inferClass(url) : preset.class,
        apiKeyEnv: (apiKeyEnv.trim() || preset.apiKeyEnv) ?? null,
        apiKey: apiKey.trim() || null,
      })
      if (r.error) {
        err = r.error
        return
      }
      await qc.invalidateQueries({ queryKey: ['fleet-endpoints'] })
      onClose()
      if (r.id) onAdded(r.id)
    } finally {
      busy = false
    }
  }
</script>

<Modal {open} {onClose} title="Add provider">
  <div class="space-y-5">
    <SettingsField label="Provider">
      <Combobox
        size="sm"
        options={PROVIDER_PRESETS.map((p) => ({
          value: p.key,
          label: p.label,
          sub: p.configurableUrl ? undefined : 'Preconfigured',
          icon: markIcon(p.provider, p.key),
        }))}
        selected={[presetKey]}
        onChange={([k]) => k && (presetKey = k)}
        placeholder="Pick a provider"
      />
    </SettingsField>
    <SettingsField label="Name" for="models-name">
      <Input id="models-name" size="sm" autofocus bind:value={name} placeholder={preset.key} />
    </SettingsField>
    {#if preset.configurableUrl}
      <SettingsField label="Base URL" for="models-base-url" hint="A host on your own network counts as self-hosted.">
        <Input id="models-base-url" size="sm" class="font-[var(--font-mono)]" bind:value={baseUrl} placeholder={preset.baseUrl ?? 'https://host/v1'} />
      </SettingsField>
    {/if}
    <SettingsField label="API key" for="models-api-key" hint="Stored encrypted. It is never shown again.">
      <Input id="models-api-key" size="sm" type="password" bind:value={apiKey} placeholder="Paste the provider key" autocomplete="off" />
    </SettingsField>
    <details class="group">
      <summary class="cursor-pointer list-none font-sans text-[13px] font-medium leading-5 text-muted transition-colors hover:text-fg">
        <span class="mr-1 inline-block transition-transform group-open:rotate-90">›</span>Read the key from an environment variable
      </summary>
      <SettingsField class="mt-3" label="Variable name" for="models-api-key-env" hint="Used only when no key is stored above.">
        <Input id="models-api-key-env" size="sm" class="font-[var(--font-mono)]" bind:value={apiKeyEnv} placeholder={preset.apiKeyEnv ?? 'MY_PROVIDER_KEY'} />
      </SettingsField>
    </details>
    {#if err}<div class="font-sans text-[13px] leading-5 text-danger" role="alert">{err}</div>{/if}
  </div>
  {#snippet footer()}
    <div class="flex justify-end gap-2">
      <Button variant="ghost" size="sm" onclick={onClose}>Cancel</Button>
      <Button size="sm" onclick={() => void create()} disabled={busy}>
        {busy ? 'Adding' : 'Add provider'}
      </Button>
    </div>
  {/snippet}
</Modal>
