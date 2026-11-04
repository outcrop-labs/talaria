<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import Button from '@/components/ui/Button.svelte'
  import Chip from '@/components/ui/Chip.svelte'
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
  import Checkbox from '@/components/ui/Checkbox.svelte'
  import { PRICING_QUERY_KEYS, UNPRICED_ANCHOR, clearPrice, includedBody, setPrice, settingLabel } from '@/components/observability/pricing'
  import { toastError } from '@/lib/toast.svelte'
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

  // A FLAT-RATE PLAN (a coding plan, a subscription) is not a per-token
  // bill, and a public catalog cannot know the account is on one — a Z.ai
  // Coding Plan's GLM ids all match OpenRouter's pay-as-you-go entries. Ticking
  // a model here prices its calls at $0, labelled "included" (the cost page's
  // unpriced panel sets the same thing for usage nothing priced).
  const included = $derived(new Set((ep.adminPrices ?? []).filter((p) => p.mode === 'included').map((p) => p.model)))
  let planBusy = $state<string | null>(null)
  const setIncluded = async (model: string, on: boolean) => {
    // Ticking replaces a manual rate (one setting per model); say so first,
    // and unticking would then remove the rate with it.
    const manual = (ep.adminPrices ?? []).find((p) => p.model === model && p.mode === 'manual')
    if (on && manual) {
      const ok = await confirm({
        title: 'Replace the manual rate',
        message: `${model} has a manual rate (${settingLabel(manual)}). Marking it included in a plan replaces that rate with $0.`,
        confirmLabel: 'Mark included',
      })
      if (!ok) return
    }
    planBusy = model
    try {
      if (on) await setPrice(includedBody({ endpoint: ep.name, model }, ''))
      else await clearPrice({ endpoint: ep.name, model })
      await Promise.all([refresh(), ...PRICING_QUERY_KEYS.map((queryKey) => qc.invalidateQueries({ queryKey: [...queryKey] }))])
    } catch (e) {
      toastError('Could not change plan pricing', e)
    } finally {
      planBusy = null
    }
  }

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
        message: `Still used by:\n${describeAffected(r.affected)}\n\nRemove it from those agents too? Each gets a new config version (revertible); running managed agents restart, and a stopped one picks it up when it next starts.`,
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
        <div class="mb-2 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Pricing · $/1M tokens</div>
        {#if (ep.providerPrices ?? []).length === 0}
          <p class="text-xs text-muted">No published price yet. Spend uses the provider's reported charge when they send one.</p>
        {:else}
          <div class="space-y-1.5">
            {#each ep.providerPrices ?? [] as row (`${row.model}:${row.variant}`)}
              <div class="flex items-baseline gap-2 font-mono text-xs">
                <span class="min-w-0 flex-1 truncate text-fg">{row.model}{row.variant ? ` · ${row.variant}` : ''}</span>
                <span class="shrink-0 text-muted" title="in / out{row.cacheRead != null ? ' · cache read' : ''}">
                  {row.in ?? '—'} / {row.out ?? '—'}{row.cacheRead != null ? ` · ${row.cacheRead}` : ''}
                </span>
                <span class="shrink-0 text-muted">{row.source}</span>
                <span class="shrink-0 text-muted">{row.fetchedAt}</span>
              </div>
            {/each}
          </div>
        {/if}
        {#if (ep.adminPrices ?? []).some((p) => p.mode === 'manual')}
          <div class="mt-3 space-y-1.5" data-testid="endpoint-admin-prices">
            {#each (ep.adminPrices ?? []).filter((p) => p.mode === 'manual') as row (row.model)}
              <div class="flex items-baseline gap-2 font-mono text-xs">
                <span class="min-w-0 flex-1 truncate text-fg">{row.model}</span>
                <Chip>manual</Chip>
                <span class="shrink-0 text-muted">{settingLabel(row)}</span>
              </div>
            {/each}
          </div>
        {/if}
        {#if ep.models.length > 0}
          <div class="mt-3" data-testid="endpoint-plan-models">
            <div class="mb-1 flex items-center gap-1.5 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">
              Included in a plan
              <InfoTip text="For a flat-rate plan or a free tier: calls to a ticked model are priced at $0 and labelled included, even where a pay-as-you-go price is published. The provider's own charge, when it sends one, still wins." />
            </div>
            <div class="flex flex-wrap gap-x-4 gap-y-1">
              {#each ep.models as model (model)}
                <Checkbox
                  checked={included.has(model)}
                  disabled={planBusy !== null}
                  label={model}
                  class="font-mono"
                  onChange={(on) => void setIncluded(model, on)}
                />
              {/each}
            </div>
          </div>
        {/if}
        <p class="mt-1 text-xs text-muted">
          Published prices are read-only: spend uses the provider's reported charge first, then the published price
          (OpenRouter's per-endpoint price when the call names the endpoint that served it). Usage nothing prices is
          listed, with the reason, under
          <a href={`/observability/cost#${UNPRICED_ANCHOR}`} class="text-accent underline-offset-2 hover:underline">Observability · Cost</a>,
          where an admin can set a price or mark a model included in a plan.
        </p>
      </section>
    {/if}

    {#if ep.class === 'cloud'}<ModelsPrivacyRow {ep} {run} />{/if}

    {#if cascading}
      <Generating site="models/endpoint-remove" label="Removing from every agent: new agent versions, a re-render, and a restart for each one that is running" lines={2} />
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
