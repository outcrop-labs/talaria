<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import FactsList from '@/components/admin/FactsList.svelte'
  import NoticeRow from '@/components/admin/NoticeRow.svelte'
  import RowStatus from '@/components/admin/RowStatus.svelte'
  import type { Fact } from '@/components/admin/facts'
  import { tableCell, tableHead, tableNum, tableRow } from '@/components/admin/table'
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import SettingsSection from '@/components/settings/SettingsSection.svelte'
  import Button from '@/components/ui/Button.svelte'
  import Input from '@/components/ui/Input.svelte'
  import QueryState from '@/components/ui/QueryState.svelte'
  import Select from '@/components/ui/Select.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import Toggle from '@/components/ui/Toggle.svelte'
  import { submitOnEnter } from '@/components/ui/control'
  import { useSavedFlash } from '@/components/ui/save-button.svelte'
  import { cn } from '@/lib/cn'
  import { getJson, postJson, putJson } from '@/lib/fetch-json'
  import { slide } from '@/lib/motion'
  import { toastError } from '@/lib/toast.svelte'
  import AdminDecideSiteModal from './AdminDecideSiteModal.svelte'
  import { PRIMITIVE_LABEL, pct, siteStatus, type DecideData } from './admin-decide'

  // THE DECISION MODEL — the port Talaria's own judgments can run on instead
  // of asking a text model to decide and parsing the answer back out.
  //
  // The panel's whole job is to make the CHOICE real and to be honest about
  // what each option can do: which question shapes a provider answers,
  // whether it answers several in one request, and — the field that matters
  // most — whether the confidence it reports is a real number or nothing at
  // all. A provider whose answers are uncalibrated still works; every
  // confidence-gated call site will just keep falling back, and an operator
  // has to be able to see that here rather than infer it from behaviour.
  type ModelInfo = { name: string; description?: string; released?: string }
  type ModelsResult = { ok: boolean; kind?: string; reason?: string; models?: ModelInfo[] }
  type TestResult = {
    ok: boolean
    kind?: string
    reason?: string
    provider?: string
    model?: string | null
    probability?: number | null
    certainty?: number
    calibrated?: boolean
    latencyMs?: number
  }

  const qc = useQueryClient()
  const query = createQuery(() => ({
    queryKey: ['decide-config'],
    queryFn: (): Promise<DecideData> => getJson<DecideData>('/api/admin/decide'),
  }))
  const data = $derived(query.data)
  const cfg = $derived(data?.config)
  const meta = $derived(data?.providers.find((p) => p.id === cfg?.provider))
  const shadow = $derived(data?.shadow ?? [])
  const allSites = $derived(data?.sites ?? [])
  const speechRooms = $derived(data?.speechRooms ?? [])
  // One site's detail at a time, looked up by id so the dialog follows a save.
  let openSiteId = $state<string | null>(null)
  const openSite = $derived(allSites.find((s) => s.id === openSiteId))
  const shadowFor = (id: string) => shadow.find((s) => s.site === id)
  const setFloor = (floor: number) => {
    if (openSiteId) void applySite(openSiteId, { floor })
  }
  // Writing a site's switch or floor goes through the same PUT, under its own
  // key, so it gets its own audit entry rather than riding a provider change.
  const applySite = async (id: string, patch: { on?: boolean; floor?: number }) => {
    try {
      await putJson('/api/admin/decide', { site: { id, ...patch } })
    } catch (e) {
      toastError('Could not change that site', e)
      return
    }
    await qc.invalidateQueries({ queryKey: ['decide-config'] })
    savedFlash.flash()
  }
  const savedFlash = useSavedFlash()

  let key = $state('')
  let savingKey = $state(false)
  let testing = $state(false)
  let result = $state<TestResult | null>(null)

  // WHICH MODEL IDS THE ENDPOINT ACTUALLY TAKES, asked of the endpoint.
  //
  // The model field used to be free text with a datalist of ids we had
  // written down, and the first real configuration of this panel failed on it:
  // the shortest plausible thing to type for TypeSafe's Jev is `jev`, which is
  // not a model id, and the only place that was said was a `400 Unknown model:
  // jev` nobody could see. A datalist suggests; it does not constrain, and it
  // cannot know about a model the provider shipped after we did. The endpoint
  // knows. So we ask it, and the field stays free text — an operator running
  // something we have never heard of must still be able to name it.
  let detecting = $state(false)
  let detected = $state<ModelInfo[] | null>(null)
  let detectError = $state<string | null>(null)
  // What `detected` is about, so switching provider or pasting a new key
  // re-asks rather than showing the previous endpoint's catalog.
  let detectedFor = $state<string | null>(null)
  const configKey = $derived(
    cfg ? `${cfg.provider}|${cfg.url ?? ''}|${cfg.wire ?? ''}|${cfg.hasKey}` : null,
  )
  const detect = async () => {
    const forKey = configKey
    detecting = true
    detectError = null
    try {
      const r = await postJson<ModelsResult>('/api/admin/decide', { action: 'models' })
      detected = r.ok ? (r.models ?? []) : null
      detectError = r.ok ? null : (r.reason ?? 'The endpoint did not list its models.')
    } catch (e) {
      detected = null
      detectError = e instanceof Error ? e.message : String(e)
    } finally {
      detectedFor = forKey
      detecting = false
    }
  }
  // Ask on load for a provider that publishes a catalog, so the list is there
  // before anyone types into the field. Keyed on the config it describes: a
  // provider or key change re-asks, a re-render does not.
  $effect(() => {
    const k = configKey
    if (!k || !data?.configured || !meta?.liveCatalog || !meta.needsModel) return
    if (detectedFor === k || detecting) return
    void detect()
  })
  // Only a SUCCESSFUL detection may contradict what is typed. Warning from our
  // own fallback list would scold an operator for naming a model that provider
  // shipped last week, which is the opposite of the point.
  const unknownModel = $derived(
    detected !== null &&
      detected.length > 0 &&
      !!cfg?.model &&
      detectedFor === configKey &&
      !detected.some((m) => m.name === cfg.model),
  )
  const modelOptions = $derived(
    detected !== null && detected.length > 0
      ? detected.map((m) => m.name)
      : (meta?.fallbackModels ?? []),
  )

  // Autosave: provider / url / wire / model / timeout apply on change. Only
  // the API key — a secret being committed — keeps an explicit save.
  const apply = async (patch: Record<string, unknown>) => {
    try {
      await putJson('/api/admin/decide', patch)
    } catch (e) {
      toastError('Apply failed', e)
      return
    }
    result = null
    await qc.invalidateQueries({ queryKey: ['decide-config'] })
    savedFlash.flash()
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
  const test = async () => {
    testing = true
    result = null
    try {
      result = await postJson<TestResult>('/api/admin/decide', { action: 'test' })
    } catch (e) {
      toastError('Test failed', e)
    } finally {
      testing = false
    }
  }

  const CALIBRATION_NOTE: Record<string, string> = {
    native: 'The model reports calibrated probabilities itself',
    classifier: 'Probabilities come from a classification head',
    logprobs: 'Worked out from token logprobs, so missing on an endpoint that does not serve them',
  }
  const sentence = (parts: string[]) => parts.map((p, i) => (i === 0 ? p : p.toLowerCase())).join(', ')
  // THE CAPABILITY SHEET. Stated rather than assumed: a call site asking a
  // shape this provider does not serve falls back instead of guessing, so an
  // operator should be able to see which shapes those are.
  const capabilities = $derived.by((): Fact[] =>
    meta
      ? [
          { label: 'Answers', value: sentence(meta.primitives.map((p) => PRIMITIVE_LABEL[p] ?? p)) },
          { label: 'Per request', value: meta.fansOut ? 'Several questions' : 'One question' },
          { label: 'Confidence', value: CALIBRATION_NOTE[meta.calibration] ?? meta.calibration },
        ]
      : [],
  )
</script>

<SettingsSection
  title="Decision model"
  description="The model that answers Talaria's own yes-or-no and pick-one questions. Off by default."
  info="A decision model answers a typed question, such as yes or no, pick one, or how much, with a probability, instead of writing prose that code has to read back. Use a hosted decision API, a classifier on your own hardware, a model you already added on Models, or any endpoint of your own. Talaria ships no default and recommends no model. Changes apply at once."
>
  {#snippet action()}
    {#if savedFlash.saved}<span class="font-sans text-[13px] leading-5 text-success">Saved</span>{/if}
    {#if meta}
      <Button size="sm" variant="outline" onclick={() => void test()} disabled={testing || !data?.configured}>
        {testing ? 'Testing' : 'Test'}
      </Button>
    {/if}
  {/snippet}
  <SettingsGroup>
    <QueryState query={query} errorTitle="Could not load the decision-model configuration" errorVariant="compact">
      {#snippet skeleton()}
        <div class="px-5 py-4"><SkeletonRows rows={3} /></div>
      {/snippet}
      {#snippet children(_d)}
        <SettingsRow
          label="Provider"
          hint={meta && meta.id !== 'off' ? `Where it runs: ${meta.country}.` : 'Every decision keeps the behaviour it has today.'}
          as="label"
        >
          <Select
            size="sm"
            value={cfg?.provider ?? 'off'}
            onchange={(e) => void apply({ provider: e.currentTarget.value, model: null, url: null, wire: null })}
            class="w-[280px]"
          >
            {#each data?.providers ?? [] as p (p.id)}
              <option value={p.id}>{p.label}</option>
            {/each}
          </Select>
        </SettingsRow>

        {#if meta?.needsWire}
          <SettingsRow label="Protocol" hint="The request format your endpoint speaks." as="label">
            <Select size="sm" value={cfg?.wire ?? ''} onchange={(e) => void apply({ wire: e.currentTarget.value || null })} class="w-[280px]">
              <option value="">Choose a protocol</option>
              {#each data?.wires ?? [] as w (w)}<option value={w}>{w}</option>{/each}
            </Select>
          </SettingsRow>
        {/if}

        {#if meta?.needsUrl}
          <SettingsRow label="Endpoint URL" as="label">
            <Input
              size="sm"
              value={cfg?.url ?? ''}
              onblur={(e) => e.currentTarget.value !== (cfg?.url ?? '') && void apply({ url: e.currentTarget.value || null })}
              placeholder="http://tei.internal:80"
              class="w-[280px] font-mono"
            />
          </SettingsRow>
        {/if}

        {#if meta?.needsKey}
          <SettingsRow label="API key" hint={cfg?.hasKey ? 'A key is stored. Type a new one to replace it.' : 'Stored encrypted. It is never shown again.'}>
            <Input
              size="sm"
              type="password"
              autocomplete="off"
              aria-label="Decision model API key"
              bind:value={key}
              onkeydown={submitOnEnter(() => key && !savingKey && void saveKey())}
              placeholder={cfg?.hasKey ? 'Paste a new key' : 'Paste the provider key'}
              class="w-[280px]"
            />
            {#if key}
              <Button size="md" onclick={() => void saveKey()} disabled={savingKey}>Save key</Button>
            {/if}
          </SettingsRow>
        {/if}

        {#if meta?.needsModel}
          <!-- STILL FREE TEXT, now with the endpoint's own list behind it. A
               picker would be a cage: the whole position of this registry is
               that an operator runs whichever model they want, including one we
               have never heard of. So the detected ids are offered and a
               mismatch is called out below, and nothing is refused. -->
          <SettingsRow
            label="Model"
            info={detected && detected.length > 0
              ? `This endpoint accepts: ${detected.map((m) => (m.released ? `${m.name} (${m.released})` : m.name)).join(', ')}`
              : undefined}
          >
            {#snippet hint()}
              {#if detectError}
                <!-- Not a failure of the config: plenty of endpoints serve
                     judgments and publish no catalog, so this is a note and
                     the field keeps working. -->
                Could not list this endpoint's models: {detectError}
              {:else if detected && detected.length > 0}
                This endpoint accepts {detected.length === 1 ? '1 model' : `${detected.length} models`}.
              {:else}
                Any model id the endpoint accepts.
              {/if}
            {/snippet}
            <Input
              size="sm"
              aria-label="Decision model id"
              value={cfg?.model ?? ''}
              onblur={(e) => e.currentTarget.value !== (cfg?.model ?? '') && void apply({ model: e.currentTarget.value || null })}
              placeholder={modelOptions[0] ?? 'endpoint:model'}
              list={`decide-models-${meta.id}`}
              class="w-[280px] font-mono"
            />
            {#if modelOptions.length > 0}
              <datalist id={`decide-models-${meta.id}`}>
                {#each modelOptions as m (m)}<option value={m}></option>{/each}
              </datalist>
            {/if}
            <Button size="md" variant="outline" onclick={() => void detect()} disabled={detecting || !data?.configured}>
              {detecting ? 'Detecting' : 'Detect'}
            </Button>
          </SettingsRow>
          {#if unknownModel}
            <!-- THE WARNING THAT WOULD HAVE SAVED AN AFTERNOON. The endpoint
                 listed its ids and the one configured is not among them, which
                 means every call will be refused — said here rather than
                 discovered from a call site silently falling back forever. -->
            <div transition:slide={{ duration: 150 }}>
              <NoticeRow status="warn">
                This endpoint does not list <span class="font-mono text-xs text-fg">{cfg?.model}</span>, so calls will be refused unless it
                takes ids it does not publish.
              </NoticeRow>
            </div>
          {/if}
        {/if}

        {#if meta}
          <SettingsRow label="Timeout" hint="Past this, the caller uses its own path instead of waiting." as="label">
            <Input
              size="sm"
              type="number"
              value={String(cfg?.timeoutMs ?? 4000)}
              onblur={(e) => Number(e.currentTarget.value) !== (cfg?.timeoutMs ?? 4000) && void apply({ timeoutMs: Number(e.currentTarget.value) })}
              class="w-28 tabular-nums"
            />
            <span class="font-sans text-[13px] leading-5 text-muted">ms</span>
          </SettingsRow>
          <div transition:slide={{ duration: 150 }}><FactsList facts={capabilities} /></div>
          {#if meta.country !== 'your hardware' && meta.country !== '—'}
            <NoticeRow status="warn">Ticket and message text is sent to this provider to be judged, so it leaves your instance.</NoticeRow>
          {/if}
        {/if}

        {#if result}
          <div transition:slide={{ duration: 150 }}>
            {#if result.ok}
              <NoticeRow status="ok">
                Answered in <span class="tabular-nums">{result.latencyMs}</span> ms from {result.provider}{#if result.model}{' · '}<span class="font-mono text-xs">{result.model}</span>{/if}.
                It rated a clearly urgent report
                <span class="tabular-nums text-fg">{((result.probability ?? 0) * 100).toFixed(0)}%</span> yes, and near 100% means it reads
                the question as intended.
              </NoticeRow>
              {#if result.calibrated === false}
                <NoticeRow status="warn" class="border-t border-line-subtle">
                  The answer carried no confidence, so every decision that needs one will fall back to its own path. For a model from
                  Models this usually means the endpoint does not serve logprobs.
                </NoticeRow>
              {/if}
            {:else}
              <!-- The PROVIDER'S own sentence where there is one. `refused`
                   means it answered and said why; the rest are ours because
                   there was no reply to quote. -->
              <NoticeRow status="danger">
                {result.reason}{#if result.kind === 'refused' && unknownModel}{' '}The model id is the usual cause.{/if}
              </NoticeRow>
            {/if}
          </div>
        {/if}
      {/snippet}
    </QueryState>
  </SettingsGroup>
</SettingsSection>

<!-- THE CENSUS, with each site's own numbers beside its own switch. That
     adjacency is the whole point of this section: shadow mode's argument is
     that a site switches over when its own traffic says so, and a number an
     operator can read but not act on is a dashboard rather than a feature. -->
{#if allSites.length > 0}
  <SettingsSection
    title="Where it decides"
    description="Each place Talaria can ask the decision model, with what its own traffic measured."
    info="A decision that is off keeps running its existing code, and the model is asked the same question with nobody listening. Agreement is how often the two matched, so you can turn a decision on when its own numbers say so."
  >
    <SettingsGroup>
      <div class="overflow-x-auto">
      <table class="w-full min-w-[760px] table-fixed">
        <colgroup>
          <col />
          <col class="w-44" />
          <col class="w-28" />
          <col class="w-24" />
          <col class="w-40" />
          <col class="w-16" />
          <col class="w-28" />
        </colgroup>
        <thead>
          <tr>
            <th class={tableHead}>Decision</th>
            <th class={tableHead}>Model</th>
            <th class={cn(tableHead, 'text-right')}>Agreement</th>
            <th class={cn(tableHead, 'text-right')}>Threshold</th>
            <th class={tableHead}>Status</th>
            <th class={tableHead}><span class="sr-only">On</span></th>
            <th class={tableHead}><span class="sr-only">Details</span></th>
          </tr>
        </thead>
        <tbody>
          {#each allSites as s (s.id)}
            {@const m = shadowFor(s.id)}
            {@const st = siteStatus(s, m)}
            <tr class={tableRow}>
              <td class={cn(tableCell, 'truncate font-medium')}>{s.label}</td>
              <td class={cn(tableCell, 'truncate font-mono text-xs text-muted')}>
                {#if m && m.models.length === 1}{m.models[0]?.model}{:else if m && m.models.length > 1}{m.models.length} models{:else}—{/if}
              </td>
              <td class={cn(tableCell, tableNum, !(m && m.answered > 0) && 'text-muted')}>{m ? pct(m.agreed, m.answered) : '—'}</td>
              <td class={cn(tableCell, tableNum, !(s.on && !s.switchLivesAt) && 'text-muted')}>
                {s.on && !s.switchLivesAt ? s.floor.toFixed(2) : '—'}
              </td>
              <td class={tableCell}><RowStatus status={st.status} title={s.switchLivesAt ?? undefined}>{st.word}</RowStatus></td>
              <td class={tableCell}>
                {#if !s.switchLivesAt}
                  <Toggle checked={s.on} onChange={(on) => void applySite(s.id, { on })} title={`Use the decision model for ${s.label}`} />
                {/if}
              </td>
              <td class={cn(tableCell, 'text-right')}>
                <Button size="sm" variant="outline" onclick={() => (openSiteId = s.id)}>Details</Button>
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
      </div>
    </SettingsGroup>
  </SettingsSection>
{/if}

{#if openSite}
  <AdminDecideSiteModal
    site={openSite}
    shadow={shadowFor(openSite.id)}
    configured={data?.configured ?? false}
    {speechRooms}
    onFloor={setFloor}
    onClose={() => (openSiteId = null)}
  />
{/if}
