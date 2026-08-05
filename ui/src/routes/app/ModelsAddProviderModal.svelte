<script lang="ts">
  import { createRawSnippet, mount, unmount, type Snippet } from 'svelte'
  import { useQueryClient } from '@tanstack/svelte-query'
  import Button from '@/components/ui/Button.svelte'
  import Combobox from '@/components/ui/Combobox.svelte'
  import InfoTip from '@/components/ui/InfoTip.svelte'
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
  <div class="space-y-4">
    <div>
      <label class="mb-1 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Provider</label>
      <Combobox
        options={PROVIDER_PRESETS.map((p) => ({
          value: p.key,
          label: p.label,
          sub: p.configurableUrl ? undefined : 'preconfigured',
          icon: markIcon(p.provider, p.key),
        }))}
        selected={[presetKey]}
        onChange={([k]) => k && (presetKey = k)}
        placeholder="Pick a provider"
      />
    </div>
    <div>
      <label class="mb-1 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Name</label>
      <Input autofocus bind:value={name} placeholder={preset.key} />
    </div>
    {#if preset.configurableUrl}
      <div>
        <label class="mb-1 flex items-center gap-1.5 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">
          Base URL
          <InfoTip text="LAN and loopback hosts count as self-hosted in the cost split, inferred automatically." />
        </label>
        <Input bind:value={baseUrl} placeholder={preset.baseUrl ?? 'https://host/v1'} />
      </div>
    {/if}
    <div>
      <label class="mb-1 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">API key</label>
      <Input type="password" bind:value={apiKey} placeholder="paste the provider key" autocomplete="off" />
      <InfoTip class="mt-1" text="Stored encrypted at rest (AES-256-GCM), never written to a config file or shown again." />
    </div>
    <details>
      <summary class="cursor-pointer font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim transition-colors hover:text-muted">Advanced: env-var fallback</summary>
      <div class="mt-2">
        <Input bind:value={apiKeyEnv} placeholder={preset.apiKeyEnv ?? 'MY_PROVIDER_KEY'} />
        <InfoTip class="mt-1" text="Optional: an env-var name to read the key from if none is stored above (ops override)." />
      </div>
    </details>
    {#if err}<div class="text-sm text-danger">{err}</div>{/if}
    <div class="flex justify-end gap-2 border-t border-line pt-3">
      <Button variant="ghost" size="sm" onclick={onClose}>
        Cancel
      </Button>
      <Button size="sm" onclick={() => void create()} disabled={busy}>
        {busy ? 'Adding' : 'Add provider'}
      </Button>
    </div>
  </div>
</Modal>
