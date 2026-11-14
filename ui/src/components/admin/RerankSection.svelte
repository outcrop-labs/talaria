<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import { useAutosave } from '@/components/settings/autosave.svelte'
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import SettingsSection from '@/components/settings/SettingsSection.svelte'
  import Button from '@/components/ui/Button.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Select from '@/components/ui/Select.svelte'
  import { submitOnEnter } from '@/components/ui/control'
  import ModelIdPicker from '@/components/fleet/ModelIdPicker.svelte'
  import { postJson, putJson } from '@/lib/fetch-json'
  import { toastError } from '@/lib/toast.svelte'
  import SaveAction from './SaveAction.svelte'
  import type { RagAdmin } from './retrieval'
  import { secretHint } from './secret-hint'

  // ── Reranker provider (the precision stage after vector recall) ─────────────
  let { rag }: { rag: RagAdmin } = $props()

  const qc = useQueryClient()
  const autosave = useAutosave()
  const cfg = $derived(rag.rerank.config)
  let key = $state('')
  let models = $state<string[] | null>(null)
  let savingKey = $state(false)
  const meta = $derived(rag.rerank.providers.find((p) => p.id === cfg.provider))

  // Autosave: provider / url / model apply on change. Only the API key (a
  // secret being committed) keeps an explicit save affordance.
  const apply = async (patch: Record<string, unknown>) => {
    try {
      await putJson('/api/admin/rag', { reranker: patch })
    } catch (e) {
      toastError('Apply failed', e)
      return
    }
    await qc.invalidateQueries({ queryKey: ['rag-admin'] })
    if (!('apiKey' in patch)) autosave.flash()
  }
  const loadModels = async () => {
    // The candidate key goes in the body — never a query string (logs).
    try {
      models = (await postJson<{ models: string[] }>('/api/admin/rag', { models: cfg.provider, ...(key ? { key } : {}) })).models
    } catch (e) {
      toastError('Load models failed', e)
    }
  }
  const saveKey = async () => {
    savingKey = true
    try {
      await apply({ apiKey: key })
      key = ''
    } finally {
      savingKey = false
    }
  }
</script>

<SettingsSection
  title="Reranker"
  description="A second pass that rescores search results so the best sources come first."
  info="The precision stage after vector recall: a cross-encoder rescores the merged candidates. Off keeps plain vector order. Run one yourself, or connect a provider. Changes apply immediately."
>
  <SettingsGroup>
    <SettingsRow
      label="Provider"
      hint={cfg.provider === 'off' ? 'Results stay in vector order.' : `Rescores the top ${cfg.candidates ?? 30} results.`}
      as="label"
    >
      <Select
        size="sm"
        value={cfg.provider}
        onchange={(e) => {
          models = null
          void apply({ provider: e.currentTarget.value, model: null })
        }}
        class="w-[280px]"
      >
        <option value="off">Off</option>
        {#each rag.rerank.providers as p (p.id)}
          <option value={p.id}>{p.label} ({p.country})</option>
        {/each}
      </Select>
    </SettingsRow>
    {#if meta?.needsUrl}
      <SettingsRow label="Endpoint URL" hint="A TEI server's rerank endpoint." as="label">
        <Input
          size="sm"
          value={cfg.url ?? ''}
          onblur={(e) => e.currentTarget.value !== (cfg.url ?? '') && void apply({ url: e.currentTarget.value || null })}
          placeholder="http://localhost:8056"
          class="w-[280px] font-mono"
        />
      </SettingsRow>
    {/if}
    {#if meta?.needsKey}
      <SettingsRow
        label="API key"
        hint={secretHint(cfg.hasKey)}
      >
        <Input
          size="sm"
          type="password"
          autocomplete="off"
          aria-label="Reranker API key"
          bind:value={key}
          onkeydown={submitOnEnter(() => key && !savingKey && void saveKey())}
          placeholder={cfg.hasKey ? 'Paste a new key' : 'Paste the provider key'}
          class="w-[280px]"
        />
        <SaveAction size="md" dirty={!!key} busy={savingKey} onclick={() => void saveKey()}>Save key</SaveAction>
      </SettingsRow>
    {/if}
    <!-- A model control only where there IS a model to pick. Derived from the
         provider's own catalogue rather than an id blocklist: the self-hosted
         sidecar serves whatever it was started with, and the decision-model
         provider delegates to talaria-decide's own config, so neither has a
         model of its own to choose here. -->
    {#if meta && (meta.liveCatalog || meta.fallbackModels.length > 0)}
      <SettingsRow label="Model">
        {#snippet hint()}
          {#if cfg.model}<span class="font-mono text-xs">{cfg.model}</span>{:else}The provider's default model.{/if}
        {/snippet}
        {#if models}
          <ModelIdPicker {models} value={cfg.model ?? null} onChange={(model) => void apply({ model })} emptyLabel="Default model" class="w-[280px]" />
        {:else}
          <Button size="sm" variant="outline" onclick={() => void loadModels()}>Change model</Button>
        {/if}
      </SettingsRow>
    {/if}
  </SettingsGroup>
</SettingsSection>
