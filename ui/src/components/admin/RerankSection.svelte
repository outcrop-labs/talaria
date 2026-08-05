<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import Button from '@/components/ui/Button.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Select from '@/components/ui/Select.svelte'
  import SectionHeader from '@/components/ui/SectionHeader.svelte'
  import { submitOnEnter } from '@/components/ui/control'
  import type { RagAdmin } from './retrieval'

  // ── Reranker provider (the precision stage after vector recall) ─────────────
  let { rag }: { rag: RagAdmin } = $props()

  const qc = useQueryClient()
  const cfg = $derived(rag.rerank.config)
  let key = $state('')
  let models = $state<string[] | null>(null)
  let savingKey = $state(false)
  const meta = $derived(rag.rerank.providers.find((p) => p.id === cfg.provider))

  // Autosave: provider / url / model apply on change. Only the API key (a
  // secret being committed) keeps an explicit save affordance.
  const apply = async (patch: Record<string, unknown>) => {
    await fetch('/api/admin/rag', {
      method: 'PUT',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ reranker: patch }),
    })
    await qc.invalidateQueries({ queryKey: ['rag-admin'] })
  }
  const loadModels = async () => {
    // The candidate key goes in the body — never a query string (logs).
    const r = await fetch('/api/admin/rag', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ models: cfg.provider, ...(key ? { key } : {}) }),
    })
    if (r.ok) models = ((await r.json()) as { models: string[] }).models
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

<div class="mt-5 border-t border-line-subtle pt-4">
  <SectionHeader class="mb-1" title="Reranker" />
  <p class="mb-3 text-xs text-muted">
    The precision stage after vector recall: a cross-encoder rescores the merged candidates so the best
    sources win. Off = plain vector order. Self-host it, or hook up a provider and set your default.
    Changes apply immediately.
  </p>
  <div class="flex flex-wrap items-center gap-2">
    <Select size="sm" value={cfg.provider} onchange={(e) => { models = null; void apply({ provider: e.currentTarget.value, model: null }) }} class="w-52">
      <option value="off">Off (vector order)</option>
      {#each rag.rerank.providers as p (p.id)}
        <option value={p.id}>
          {p.label} ({p.country})
        </option>
      {/each}
    </Select>
    {#if meta?.needsUrl}
      <Input
        size="sm"
        value={cfg.url ?? ''}
        onblur={(e) => e.currentTarget.value !== (cfg.url ?? '') && void apply({ url: e.currentTarget.value || null })}
        placeholder="http://localhost:8056 (TEI /rerank)"
        class="w-64"
      />
    {/if}
    {#if meta?.needsKey}
      <Input size="sm" type="password" bind:value={key} onkeydown={submitOnEnter(() => key && !savingKey && void saveKey())} placeholder={cfg.hasKey ? 'replace saved key' : 'API key'} class="w-52" />
      {#if key}
        <Button size="sm" onclick={() => void saveKey()} disabled={savingKey}>
          Save key
        </Button>
      {/if}
    {/if}
    {#if meta && meta.id !== 'tei'}
      {#if models}
        <Select size="sm" value={cfg.model ?? ''} onchange={(e) => void apply({ model: e.currentTarget.value || null })} class="w-64">
          <option value="">default model</option>
          {#each models as m (m)}
            <option value={m}>{m}</option>
          {/each}
        </Select>
      {:else}
        <Button size="sm" variant="outline" onclick={() => void loadModels()}>
          Load models
        </Button>
      {/if}
    {/if}
  </div>
  {#if cfg.provider !== 'off'}
    <div class="mt-1.5 font-mono text-[10px] tracking-[0.05em] text-muted">
      Active: {cfg.provider}{cfg.model ? ` · ${cfg.model}` : ''}{cfg.hasKey ? ' · key sealed' : ''} · rescoring top {cfg.candidates ?? 30}
    </div>
  {/if}
</div>
