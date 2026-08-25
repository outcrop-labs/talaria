<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import Button from '@/components/ui/Button.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import Generating from '@/components/ui/Generating.svelte'
  import InfoTip from '@/components/ui/InfoTip.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Modal from '@/components/ui/Modal.svelte'
  import Select from '@/components/ui/Select.svelte'
  import { confirm } from '@/components/ui/confirm.svelte'
  import { submitOnEnter } from '@/components/ui/control'
  import { cn } from '@/lib/cn'
  import { patchEndpoint, removeEndpoint, useAvailableModels, type EndpointOpResult } from '@/lib/models'
  import type { LlmEndpoint } from '@/lib/fleet-defs'
  import ModelsModelAdder from './ModelsModelAdder.svelte'
  import ModelsEffortRow from './ModelsEffortRow.svelte'
  import ModelsPrivacyRow from './ModelsPrivacyRow.svelte'
  import { describeAffected } from './models'

  // The provider's full surface: models (add/remove with catalog search), pricing,
  // class, privacy routing, and removal.
  let { ep, onClose }: { ep: LlmEndpoint; onClose: () => void } = $props()

  const qc = useQueryClient()
  const availableQuery = useAvailableModels(() => ep.id)
  const available = $derived(availableQuery.data)
  let err = $state<string | null>(null)
  let key = $state('')
  let savingKey = $state(false)
  // Registry edits change the gateway catalog too — refresh it so the Member
  // access panel and every model picker see additions/removals immediately.
  const refresh = () =>
    Promise.all([
      qc.invalidateQueries({ queryKey: ['fleet-endpoints'] }),
      qc.invalidateQueries({ queryKey: ['gateway-models'] }),
    ])

  const saveKey = async (value: string | null) => {
    savingKey = true
    try {
      await run(patchEndpoint(ep.id, { apiKey: value }))
      key = ''
    } finally {
      savingKey = false
    }
  }

  const run = async (p: Promise<{ error?: string }>) => {
    err = null
    const r = await p
    if (r.error) err = r.error
    await refresh()
  }
  let cascading = $state(false)
  // Returns the final result so callers can react to it (the provider-removal
  // button closes the modal only when the removal actually succeeded — a
  // refused delete must stay open and show why, or the refusal is invisible).
  const runCascading = async (op: (force: boolean) => Promise<EndpointOpResult>): Promise<EndpointOpResult> => {
    err = null
    let r = await op(false)
    if (r.needsForce && r.affected) {
      const go = await confirm({
        title: 'Still in use',
        message: `Still used by:\n${describeAffected(r.affected)}\n\nRemove it from those agents too? Each gets a new config version (revertible); running managed agents restart.`,
        confirmLabel: 'Remove everywhere',
        danger: true,
      })
      if (go) {
        cascading = true
        try {
          r = await op(true)
        } finally {
          cascading = false
        }
      }
    }
    if (r.error) err = r.error
    await refresh()
    void qc.invalidateQueries({ queryKey: ['fleet-defs'] })
    return r
  }

  const addModel = (id: string) => {
    if (!id.trim() || ep.models.includes(id.trim())) return
    void runCascading((force) => patchEndpoint(ep.id, { models: [...ep.models, id.trim()], force }))
  }
  const removeModel = (id: string) =>
    void runCascading((force) => patchEndpoint(ep.id, { models: ep.models.filter((m) => m !== id), force }))

  const setPrice = (m: string, priceKey: 'in' | 'out', raw: string) => {
    const next = { ...(ep.modelPrices ?? {}) }
    const entry = { ...(next[m] ?? {}) }
    if (raw === '') delete entry[priceKey]
    else entry[priceKey] = Number(raw)
    if (entry.in === undefined && entry.out === undefined) delete next[m]
    else next[m] = entry
    void run(patchEndpoint(ep.id, { modelPrices: next }))
  }

  // Declaring a ladder is a whole-map write like the pricing edits beside it.
  // A cache refresh rides along: the composer's effort chip reads through the
  // efforts API, which caches for a minute — kicking the query is what makes
  // the picker appear on a surface that already asked.
  const setEfforts = (m: string, levels: string[] | null) => {
    const next = { ...(ep.modelEfforts ?? {}) }
    if (levels) next[m] = levels
    else delete next[m]
    void run(patchEndpoint(ep.id, { modelEfforts: next }))
    void qc.invalidateQueries({ queryKey: ['model-efforts'] })
  }
</script>

<Modal open {onClose} title={`${ep.name} · ${ep.provider}`} width="max-w-2xl">
  <div class="space-y-5">
    <div class="flex items-center gap-3 font-mono text-[11px] text-muted">
      {#if ep.baseUrl}<span class="truncate">{ep.baseUrl}</span>{/if}
      <span class="ml-auto"></span>
      {#if ep.provider === 'custom'}
        <Select value={ep.class} size="sm" onchange={(e) => void run(patchEndpoint(ep.id, { class: e.currentTarget.value as 'local' | 'cloud' }))}>
          <option value="local">self-hosted</option>
          <option value="cloud">cloud</option>
        </Select>
      {:else}
        <span class={cn('uppercase tracking-[0.05em]', ep.class === 'local' ? 'text-success' : 'text-accent')}>
          {ep.class === 'local' ? 'self-hosted' : 'cloud'}
        </span>
      {/if}
    </div>

    <!-- Provider API key — stored encrypted at rest, never shown again -->
    <section>
      <div class="mb-2 flex items-center gap-2 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">
        <span>API key</span>
        {#if ep.hasKey}
          <span class="tracking-[0.05em] text-success">● encrypted key stored</span>
        {:else}
          <span class="tracking-[0.05em] text-muted">none stored{ep.apiKeyEnv ? `, using $${ep.apiKeyEnv}` : ''}</span>
        {/if}
      </div>
      <div class="flex items-center gap-2">
        <Input
          type="password"
          bind:value={key}
          onkeydown={submitOnEnter(() => key.trim() && !savingKey && void saveKey(key.trim()))}
          placeholder={ep.hasKey ? 'paste a new key to rotate' : 'paste the provider key'}
          autocomplete="off"
        />
        <Button size="sm" disabled={!key.trim() || savingKey} onclick={() => void saveKey(key.trim())}>
          {ep.hasKey ? 'Rotate' : 'Save'}
        </Button>
        {#if ep.hasKey}
          <Button variant="ghost" size="sm" disabled={savingKey} onclick={() => void saveKey('')}>
            Clear
          </Button>
        {/if}
      </div>
      <InfoTip class="mt-1" text="Encrypted with the Talaria secret (AES-256-GCM). Stored in the database, never in a config file." />
    </section>

    <!-- Models the org can use from this provider -->
    <section>
      <div class="mb-2 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Available models</div>
      {#if ep.models.length > 0}
        <div class="mb-2 divide-y divide-line">
          {#each ep.models as m (m)}
            <div class="flex items-center gap-2 py-1.5 text-sm transition-colors dither-fill">
              <span class="min-w-0 flex-1 truncate font-mono text-xs text-fg">{m}</span>
              <Button variant="ghost" size="xs" class="shrink-0 hover:text-danger" onclick={() => removeModel(m)}>
                Remove
              </Button>
            </div>
          {/each}
        </div>
      {:else}
        <EmptyState variant="inline" class="mb-2" title="No models added yet." />
      {/if}
      <ModelsModelAdder catalog={available?.models ?? []} existing={ep.models} onAdd={addModel} />
      {#if available?.note}<div class="mt-1.5 text-xs text-muted">Provider catalog unavailable: {available.note}</div>{/if}
    </section>

    <!-- Reasoning-effort ladders. Rendered for EVERY endpoint class — a
         self-host is the case that needs it most: minimal OpenAI-compatible
         servers publish no per-model parameters at all, so without a
         declaration the composer never offers the dial for models that take
         one. The provider's own ladder (when it publishes one) rides along
         with the live catalog the modal already fetched. -->
    {#if ep.models.length > 0}
      <section>
        <div class="mb-2 flex items-center gap-2 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">
          <span>Reasoning effort</span>
        </div>
        <div class="divide-y divide-line">
          {#each ep.models as m (m)}
            <ModelsEffortRow
              model={m}
              catalogEfforts={available?.catalog?.find((c) => c.id === m)?.efforts ?? null}
              declared={ep.modelEfforts?.[m]}
              onDeclare={(levels) => setEfforts(m, levels)}
            />
          {/each}
        </div>
        <InfoTip
          class="mt-1"
          text="Levels the model accepts, sent verbatim (e.g. low, medium, high). A declaration stands in where the provider's catalog publishes none, and replaces it where it does."
        />
      </section>
    {/if}

    {#if ep.class === 'cloud'}
      <section>
        <div class="mb-2 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Pricing · $/1M tokens (in / out)</div>
        <div class="space-y-1.5">
          <div class="flex items-center gap-2 font-mono text-xs">
            <span class="min-w-0 flex-1 truncate text-muted">endpoint default (fallback)</span>
            <Input size="sm" type="number" value={ep.priceInPerMtok ?? ''} placeholder="in" class="w-20 shrink-0"
              onblur={(e) => { const v = e.currentTarget.value.trim(); void run(patchEndpoint(ep.id, { priceInPerMtok: v === '' ? null : Number(v) })) }} />
            <Input size="sm" type="number" value={ep.priceOutPerMtok ?? ''} placeholder="out" class="w-20 shrink-0"
              onblur={(e) => { const v = e.currentTarget.value.trim(); void run(patchEndpoint(ep.id, { priceOutPerMtok: v === '' ? null : Number(v) })) }} />
          </div>
          {#each ep.models as m (m)}
            {@const p = ep.modelPrices?.[m]}
            {@const auto = ep.autoPrices?.[m]}
            {@const overridden = p?.in !== undefined || p?.out !== undefined}
            <div class="flex items-center gap-2 font-mono text-xs">
              <span class="min-w-0 flex-1 truncate text-fg">{m}</span>
              {#if !overridden && auto}<span class="shrink-0 text-success">auto</span>{/if}
              {#if !overridden && !auto}<span class="shrink-0 text-muted">unpriced</span>{/if}
              <Input size="sm" type="number" value={p?.in ?? ''} placeholder={auto ? String(auto.in) : 'in'} class="w-20 shrink-0" onblur={(e) => setPrice(m, 'in', e.currentTarget.value.trim())} />
              <Input size="sm" type="number" value={p?.out ?? ''} placeholder={auto ? String(auto.out) : 'out'} class="w-20 shrink-0" onblur={(e) => setPrice(m, 'out', e.currentTarget.value.trim())} />
            </div>
          {/each}
        </div>
      </section>
    {/if}

    {#if ep.class === 'cloud'}<ModelsPrivacyRow {ep} {run} />{/if}

    {#if cascading}
      <Generating site="models/endpoint-remove" label="Removing from every agent: new agent versions, re-render, restarting the affected agents" lines={2} />
    {/if}
    {#if err}<div class="text-xs text-danger">{err}</div>{/if}

    <div class="flex items-center gap-2 border-t border-line pt-3">
      <Button
        variant="ghost"
        size="sm"
        onclick={async () => { if (await confirm({ title: 'Remove provider', message: `Remove the ${ep.name} provider?`, confirmLabel: 'Remove', danger: true })) void runCascading((force) => removeEndpoint(ep.id, force)).then((r) => { if (r.ok && !r.error) onClose() }) }}
      >
        Remove provider
      </Button>
      <span class="ml-auto"></span>
      <Button size="sm" onclick={onClose}>Done</Button>
    </div>
  </div>
</Modal>
