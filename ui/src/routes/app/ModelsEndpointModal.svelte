<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import SettingsField from '@/components/settings/SettingsField.svelte'
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import Button from '@/components/ui/Button.svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import DangerLink from '@/components/ui/DangerLink.svelte'
  import Generating from '@/components/ui/Generating.svelte'
  import InfoTip from '@/components/ui/InfoTip.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Modal from '@/components/ui/Modal.svelte'
  import Select from '@/components/ui/Select.svelte'
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import Toggle from '@/components/ui/Toggle.svelte'
  import { confirm } from '@/components/ui/confirm.svelte'
  import { submitOnEnter } from '@/components/ui/control'
  import { patchEndpoint, removeEndpoint, useAvailableModels, type EndpointOpResult } from '@/lib/models'
  import type { LlmEndpoint } from '@/lib/fleet-defs'
  import ModelsModelAdder from './ModelsModelAdder.svelte'
  import ModelsEffortRow from './ModelsEffortRow.svelte'
  import ModelsPrivacyRow from './ModelsPrivacyRow.svelte'
  import ModelName from '@/components/models/ModelName.svelte'
  import { useModelNames } from '@/components/models/model-names.svelte'
  import { PRICING_QUERY_KEYS, UNPRICED_ANCHOR, clearPrice, includedBody, setPrice, settingLabel } from '@/components/observability/pricing'
  import { toastError } from '@/lib/toast.svelte'
  import { describeAffected } from './models'

  // The provider's full surface: models (add/remove with catalog search), pricing,
  // class, privacy routing, and removal.
  let { ep, onClose, focusKey = false }: { ep: LlmEndpoint; onClose: () => void; /** Open with the cursor in the key field. */ focusKey?: boolean } = $props()

  const qc = useQueryClient()
  const modelName = useModelNames()
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
  // Switching a model on here prices its calls at $0, labelled "included" (the
  // cost page's unpriced panel sets the same thing for usage nothing priced).
  const included = $derived(new Set((ep.adminPrices ?? []).filter((p) => p.mode === 'included').map((p) => p.model)))
  let planBusy = $state<string | null>(null)
  const setIncluded = async (model: string, on: boolean) => {
    // Switching on replaces a manual rate (one setting per model); say so
    // first, and switching off would then remove the rate with it.
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
  const removeProvider = async () => {
    if (!(await confirm({ title: 'Remove provider', message: `Remove the ${ep.name} provider?`, confirmLabel: 'Remove', danger: true }))) return
    // Close only when the removal succeeded: a refused delete must stay open
    // and show why, or the refusal is invisible.
    const r = await runCascading((force) => removeEndpoint(ep.id, force))
    if (r.ok && !r.error) onClose()
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

  const heading = 'font-sans text-[13px] font-medium leading-5 text-fg'
  const manualPrices = $derived((ep.adminPrices ?? []).filter((p) => p.mode === 'manual'))
  const rate = (n: number | null | undefined) => (n == null ? '—' : `$${n}`)
</script>

{#snippet model(id: string)}
  <ModelName {id} name={modelName(id)} />
{/snippet}

<Modal open {onClose} title={ep.name} width="max-w-2xl">
  <div class="space-y-7">
    <dl class="grid grid-cols-[7rem_minmax(0,1fr)] items-center gap-x-4 gap-y-2 font-sans text-[13px] leading-5">
      <dt class="text-muted">Hosting</dt>
      <dd>
        {#if ep.provider === 'custom'}
          <Select value={ep.class} size="sm" onchange={(e) => void run(patchEndpoint(ep.id, { class: e.currentTarget.value as 'local' | 'cloud' }))}>
            <option value="local">Self-hosted</option>
            <option value="cloud">Cloud</option>
          </Select>
        {:else}
          <span class="text-fg">{ep.class === 'local' ? 'Self-hosted' : 'Cloud'}</span>
        {/if}
      </dd>
      {#if ep.baseUrl}
        <dt class="text-muted">Base URL</dt>
        <dd class="truncate font-mono text-xs text-muted">{ep.baseUrl}</dd>
      {/if}
    </dl>

    <!-- The key is stored encrypted at rest and never shown again. -->
    <SettingsField
      label="API key"
      for="endpoint-key"
      info="Encrypted with the Talaria secret (AES-256-GCM). Stored in the database, never in a config file."
    >
      <div class="flex items-center gap-2">
        <Input
          id="endpoint-key"
          size="sm"
          type="password"
          bind:value={key}
          onkeydown={submitOnEnter(() => key.trim() && !savingKey && void saveKey(key.trim()))}
          placeholder={ep.hasKey ? 'Paste a new key to replace it' : 'Paste the provider key'}
          autocomplete="off"
          autofocus={focusKey}
        />
        <Button size="md" disabled={!key.trim() || savingKey} onclick={() => void saveKey(key.trim())}>
          {ep.hasKey ? 'Replace key' : 'Save key'}
        </Button>
      </div>
      <div class="flex items-center gap-2 font-sans text-[13px] leading-5 text-muted">
        {#if ep.hasKey}
          <StatusDot status="ok" />Key stored
          <DangerLink class="ml-auto" onClick={() => void saveKey('')} disabled={savingKey}>Remove key</DangerLink>
        {:else}
          <StatusDot status={ep.apiKeyEnv ? 'idle' : ep.class === 'cloud' ? 'warn' : 'idle'} />
          {#if ep.apiKeyEnv}
            No key stored. Using <span class="font-mono text-xs">${ep.apiKeyEnv}</span>
          {:else if ep.class === 'cloud'}
            Needs a key
          {:else}
            No key stored. A self-hosted server usually needs none.
          {/if}
        {/if}
      </div>
    </SettingsField>

    <section>
      <h3 class={heading}>Models</h3>
      <p class="mb-2 font-sans text-[13px] leading-5 text-muted">What your agents can use from this provider.</p>
      <SettingsGroup>
        {#each ep.models as m (m)}
          <SettingsRow>
            {#snippet label()}{@render model(m)}{/snippet}
            <DangerLink onClick={() => removeModel(m)}>Remove</DangerLink>
          </SettingsRow>
        {/each}
        <div class="px-5 py-4">
          <ModelsModelAdder catalog={available?.models ?? []} existing={ep.models} onAdd={addModel} />
          {#if available?.note}
            <p class="mt-2 font-sans text-[13px] leading-5 text-muted">The provider catalog is unavailable: {available.note}</p>
          {/if}
        </div>
      </SettingsGroup>
    </section>

    <!-- Reasoning-effort ladders. Rendered for EVERY endpoint class — a
         self-host is the case that needs it most: minimal OpenAI-compatible
         servers publish no per-model parameters at all, so without a
         declaration the composer never offers the dial for models that take
         one. The provider's own ladder (when it publishes one) rides along
         with the live catalog the modal already fetched. -->
    {#if ep.models.length > 0}
      <section>
        <div class="flex items-center gap-2">
          <h3 class={heading}>Reasoning effort</h3>
          <InfoTip
            text="Levels the model accepts, sent as written (for example low, medium, high). A declaration stands in where the provider's catalog publishes none, and replaces it where it does."
          />
        </div>
        <p class="mb-2 font-sans text-[13px] leading-5 text-muted">Declare the levels a model takes when its provider lists none.</p>
        <SettingsGroup>
          {#each ep.models as m (m)}
            <ModelsEffortRow
              model={m}
              modelName={modelName(m)}
              catalogEfforts={available?.catalog?.find((c) => c.id === m)?.efforts ?? null}
              declared={ep.modelEfforts?.[m]}
              onDeclare={(levels) => setEfforts(m, levels)}
            />
          {/each}
        </SettingsGroup>
      </section>
    {/if}

    {#if ep.class === 'cloud'}
      <section>
        <div class="flex items-center gap-2">
          <h3 class={heading}>Pricing</h3>
          <InfoTip
            text="Spend uses the provider's reported charge first, then the published price (OpenRouter's per-endpoint price when the call names the endpoint that served it). Published prices are read-only."
          />
        </div>
        <p class="mb-2 font-sans text-[13px] leading-5 text-muted">
          Per 1M tokens. Set a price for unpriced usage in
          <a href={`/observability/cost#${UNPRICED_ANCHOR}`} class="text-accent underline-offset-2 hover:underline">Observability › Cost</a>.
        </p>
        <div class="space-y-3">
          {#if (ep.providerPrices ?? []).length === 0}
            <p class="font-sans text-[13px] leading-5 text-muted">No published price yet. Spend uses the provider's reported charge when it sends one.</p>
          {:else}
            <div class="overflow-x-auto rounded-lg border border-line">
              <table class="w-full border-collapse text-left">
                <thead>
                  <tr class="border-b border-line font-sans text-xs leading-4 text-muted">
                    <th class="px-5 py-2 font-medium">Model</th>
                    <th class="px-3 py-2 text-right font-medium">Input</th>
                    <th class="px-3 py-2 text-right font-medium">Output</th>
                    <th class="px-3 py-2 text-right font-medium">Cache read</th>
                    <th class="py-2 pl-3 pr-5 font-medium">Source</th>
                  </tr>
                </thead>
                <tbody>
                  {#each ep.providerPrices ?? [] as row (`${row.model}:${row.variant}`)}
                    <tr class="h-10 border-b border-line-subtle last:border-0">
                      <td class="px-5 font-sans text-[13px] text-fg" title={row.model}>
                        {#if modelName(row.model)}{modelName(row.model)}{:else}<span class="font-mono text-xs">{row.model}</span>{/if}{row.variant ? ` · ${row.variant}` : ''}
                      </td>
                      <td class="px-3 text-right font-sans text-[13px] tabular-nums text-fg">{rate(row.in)}</td>
                      <td class="px-3 text-right font-sans text-[13px] tabular-nums text-fg">{rate(row.out)}</td>
                      <td class="px-3 text-right font-sans text-[13px] tabular-nums text-muted">{rate(row.cacheRead)}</td>
                      <td class="whitespace-nowrap pl-3 pr-5 font-sans text-[13px] text-muted" title={`Fetched ${row.fetchedAt}`}>{row.source}</td>
                    </tr>
                  {/each}
                </tbody>
              </table>
            </div>
          {/if}
          {#if manualPrices.length > 0}
            <SettingsGroup data-testid="endpoint-admin-prices">
              {#each manualPrices as row (row.model)}
                <SettingsRow>
                  {#snippet label()}{@render model(row.model)}{/snippet}
                  {#snippet meta()}<Chip>Manual</Chip>{/snippet}
                  <span class="font-sans text-[13px] leading-5 tabular-nums text-muted">{settingLabel(row)}</span>
                </SettingsRow>
              {/each}
            </SettingsGroup>
          {/if}
          {#if ep.models.length > 0}
            <div data-testid="endpoint-plan-models">
              <div class="mb-2 flex items-center gap-2">
                <h4 class="font-sans text-[13px] leading-5 text-muted">Included in a plan</h4>
                <InfoTip text="For a flat-rate plan or a free tier: calls to a model switched on here are priced at $0 and labeled included, even where a pay-as-you-go price is published. The provider's own charge, when it sends one, still wins." />
              </div>
              <SettingsGroup>
                {#each ep.models as m (m)}
                  <SettingsRow>
                    {#snippet label()}{@render model(m)}{/snippet}
                    <Toggle
                      checked={included.has(m)}
                      disabled={planBusy !== null}
                      title={`${m} is included in a plan`}
                      onChange={(on) => void setIncluded(m, on)}
                    />
                  </SettingsRow>
                {/each}
              </SettingsGroup>
            </div>
          {/if}
        </div>
      </section>

      <section>
        <h3 class={`mb-2 ${heading}`}>Privacy</h3>
        <SettingsGroup><ModelsPrivacyRow {ep} {run} /></SettingsGroup>
      </section>
    {/if}
  </div>

  {#snippet footer()}
    <!-- The outcome of a removal belongs beside the control that asked for
         it, and the body may be scrolled anywhere when it lands. -->
    {#if cascading}
      <Generating class="mb-3" site="models/endpoint-remove" label="Removing from every agent: new agent versions, a re-render, and a restart for each one that is running" lines={2} />
    {/if}
    {#if err}<div class="mb-3 font-sans text-[13px] leading-5 text-danger" role="alert">{err}</div>{/if}
    <div class="flex items-center gap-2">
      <DangerLink onClick={() => void removeProvider()}>Remove provider</DangerLink>
      <span class="ml-auto"></span>
      <Button variant="outline" size="sm" onclick={onClose}>Done</Button>
    </div>
  {/snippet}
</Modal>
