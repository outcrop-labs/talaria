<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import Button from '@/components/ui/Button.svelte'
  import Checkbox from '@/components/ui/Checkbox.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Panel from '@/components/ui/Panel.svelte'
  import QueryState from '@/components/ui/QueryState.svelte'
  import SectionHeader from '@/components/ui/SectionHeader.svelte'
  import Select from '@/components/ui/Select.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import { submitOnEnter } from '@/components/ui/control'
  import { useSavedFlash } from '@/components/ui/save-button.svelte'
  import { getJson, postJson, putJson } from '@/lib/fetch-json'
  import { slide } from '@/lib/motion'
  import { toastError } from '@/lib/toast.svelte'

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
  type Provider = {
    id: string
    label: string
    country: string
    needsUrl: boolean
    needsKey: boolean
    needsModel: boolean
    needsWire: boolean
    wire: string
    fallbackModels: string[]
    liveCatalog: boolean
    primitives: string[]
    fansOut: boolean
    calibration: string
  }
  type DecideData = {
    providers: Provider[]
    wires: string[]
    config: {
      provider: string
      url?: string | null
      model?: string | null
      wire?: string | null
      timeoutMs?: number | null
      hasKey: boolean
    }
    configured: boolean
    shadow: SiteShadow[]
    sites: Site[]
    speechRooms: Array<{ id: string; name: string; agents: number }>
    toolShadow: boolean
  }
  type LedgerRow = {
    subjectRef: string | null
    baseline: string | null
    portAnswer: string | null
    certainty: number | null
    calibrated: boolean
    model: string | null
    latencyMs: number | null
    agreed: boolean | null
    at: string | null
  }
  // THE CENSUS — every place in Talaria that asks a decision model anything.
  // Listed in full, including the sites this panel cannot switch: "where is a
  // decision model being used" has to have one complete answer, or the answer
  // is "nobody is sure".
  type Site = {
    id: string
    label: string
    primitive: string
    acts: string
    on: boolean
    floor: number
    defaultOn: boolean
    defaultFloor: number
    reads: string
    floorWhy: string
    switchLivesAt: string | null
    /** Where this site's numbers live when they are not in the shadow ledger.
     *  A guard finding IS the output, so there is no second answer to compare
     *  it against — a row that said "no comparisons yet" for ever would read
     *  as a bug rather than as the truth. */
    measuredAt: string | null
  }
  type SiteShadow = {
    site: string
    compared: number
    acted: number
    answered: number
    agreed: number
    judgments: number
    calibrated: number
    models: Array<{ model: string; judgments: number }>
    tokensIn: number
    metered: number
    p50Ms: number | null
    p99Ms: number | null
    meanCertaintyWhenDisagreed: number | null
    provider: string | null
    newest: string | null
  }
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
  // THE ROWS BEHIND THE PERCENTAGE. "Agreed on 94% of 300" is evidence about a
  // population; a person deciding whether to trust a judgment needs the cases
  // — and specifically the disagreements, because whether those were the model
  // being right or the model being wrong is the whole question.
  let openRows = $state<string | null>(null)
  let rows = $state<LedgerRow[]>([])
  let loadingRows = $state(false)
  const showRows = async (site: string) => {
    if (openRows === site) {
      openRows = null
      return
    }
    openRows = site
    rows = []
    loadingRows = true
    try {
      const r = await postJson<{ rows: LedgerRow[] }>('/api/admin/decide', {
        action: 'rows',
        site,
        limit: 25,
      })
      rows = r.rows ?? []
    } catch (e) {
      toastError('Could not read that site\u2019s ledger', e)
      openRows = null
    } finally {
      loadingRows = false
    }
  }
  const num = (n: number) => n.toLocaleString()
  const shadowFor = (id: string) => shadow.find((s) => s.site === id)
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
  const toolShadow = $derived(data?.toolShadow ?? false)
  const pct = (n: number, d: number) => (d > 0 ? `${Math.round((n / d) * 100)}%` : '—')
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

  const PRIMITIVE_LABEL: Record<string, string> = {
    noul: 'yes/no',
    choice: 'pick one',
    score: 'how much',
  }
  const CALIBRATION_NOTE: Record<string, string> = {
    native: 'the model emits calibrated probabilities directly',
    classifier: 'probabilities come from a classification head',
    logprobs: 'probabilities are derived from token logprobs — absent entirely on an endpoint that does not serve them',
  }
</script>

<Panel>
  <SectionHeader
    class="mb-1"
    title="Decision model"
    info="A decision model answers a typed question — yes/no, pick one, how much — with a calibrated probability, instead of writing prose that code has to parse. Talaria's own judgments can run on one: the ticket-thread gate, guardrail checks, workflow matching, retrieval reranking. Optional and off by default; with it off every one of those keeps the behaviour it has today."
  />
  <p class="mb-3 text-xs text-muted">
    Run whichever model you want — a hosted decision API, a classifier on your own hardware, a model you
    already registered on Models, or any endpoint of your own that speaks one of the wires below. We ship
    no default and recommend no model. Changes apply immediately.
  </p>
  <QueryState query={query} errorTitle="Could not load the decision-model configuration" errorVariant="compact">
    {#snippet skeleton()}
      <div class="flex flex-wrap items-center gap-2">
        <Skeleton class="h-8 w-56" />
        <Skeleton class="h-8 w-64" />
      </div>
    {/snippet}
    {#snippet children(_d)}
    <div class="flex flex-wrap items-center gap-2">
      <Select
        size="sm"
        value={cfg?.provider ?? 'off'}
        onchange={(e) => void apply({ provider: e.currentTarget.value, model: null, url: null, wire: null })}
        class="w-56"
      >
        {#each data?.providers ?? [] as p (p.id)}
          <option value={p.id}>{p.label}{p.id === 'off' ? '' : ` (${p.country})`}</option>
        {/each}
      </Select>

      {#if meta?.needsWire}
        <Select
          size="sm"
          value={cfg?.wire ?? ''}
          onchange={(e) => void apply({ wire: e.currentTarget.value || null })}
          class="w-40"
        >
          <option value="">Wire…</option>
          {#each data?.wires ?? [] as w (w)}<option value={w}>{w}</option>{/each}
        </Select>
      {/if}

      {#if meta?.needsUrl}
        <Input
          size="sm"
          value={cfg?.url ?? ''}
          onblur={(e) => e.currentTarget.value !== (cfg?.url ?? '') && void apply({ url: e.currentTarget.value || null })}
          placeholder="http://tei.internal:80"
          class="w-64"
        />
      {/if}

      {#if meta?.needsKey}
        <Input
          size="sm"
          type="password"
          bind:value={key}
          onkeydown={submitOnEnter(() => key && !savingKey && void saveKey())}
          placeholder={cfg?.hasKey ? 'replace saved key' : 'API key'}
          class="w-52"
        />
        {#if key}
          <Button size="sm" onclick={() => void saveKey()} disabled={savingKey}>Save key</Button>
        {/if}
      {/if}

      {#if meta?.needsModel}
        <!-- STILL FREE TEXT, now with the endpoint's own list behind it. A
             picker would be a cage: the whole position of this registry is
             that an operator runs whichever model they want, including one we
             have never heard of. So the detected ids are offered and a
             mismatch is called out below, and nothing is refused. -->
        <Input
          size="sm"
          value={cfg?.model ?? ''}
          onblur={(e) => e.currentTarget.value !== (cfg?.model ?? '') && void apply({ model: e.currentTarget.value || null })}
          placeholder={modelOptions[0] ?? 'endpoint:model'}
          list={`decide-models-${meta.id}`}
          class="w-56"
        />
        {#if modelOptions.length > 0}
          <datalist id={`decide-models-${meta.id}`}>
            {#each modelOptions as m (m)}<option value={m}></option>{/each}
          </datalist>
        {/if}
        <Button
          size="sm"
          variant="ghost"
          onclick={() => void detect()}
          disabled={detecting || !data?.configured}
        >
          {detecting ? 'Detecting…' : 'Detect models'}
        </Button>
      {/if}

      {#if meta}
        <Button size="sm" variant="outline" onclick={() => void test()} disabled={testing || !data?.configured}>
          {testing ? 'Testing…' : 'Test'}
        </Button>
      {/if}
      {#if savedFlash.saved}<span class="text-xs text-success">Saved</span>{/if}
    </div>

    {#if meta?.needsModel && (unknownModel || detectError || (detected && detected.length > 0))}
      <div transition:slide={{ duration: 150 }} class="mt-2 text-[11px]">
        {#if unknownModel}
          <!-- THE WARNING THAT WOULD HAVE SAVED AN AFTERNOON. The endpoint
               listed its ids and the one configured is not among them, which
               means every call will be refused — said here rather than
               discovered from a call site silently falling back forever. -->
          <p class="text-warn">
            This endpoint does not list <span class="font-mono text-fg">{cfg?.model}</span> among the
            models it accepts. It offers {detected?.map((m) => m.name).join(', ')} — calls will be
            refused until this matches one of them, unless the endpoint takes ids it does not publish.
          </p>
        {:else if detected && detected.length > 0}
          <p class="text-muted">
            This endpoint accepts {detected.length === 1 ? '1 model' : `${detected.length} models`}:
            {#each detected as m, i (m.name)}<span class="font-mono text-fg">{m.name}</span>{#if m.released}<span class="text-muted"> ({m.released})</span>{/if}{#if i < detected.length - 1}<span class="text-muted">, </span>{/if}{/each}
          </p>
        {/if}
        {#if detectError}
          <!-- Not a failure of the config: plenty of endpoints serve
               judgments and publish no catalog, so this is a note and the
               field keeps working. -->
          <p class="text-muted">Could not list this endpoint's models — {detectError}</p>
        {/if}
      </div>
    {/if}

    {#if meta}
      <!-- THE CAPABILITY SHEET. Stated rather than assumed: a call site asking
           a shape this provider does not serve falls back instead of guessing,
           so an operator should be able to see which shapes those are. -->
      <div transition:slide={{ duration: 150 }} class="mt-3 space-y-1.5 border-t border-line-subtle pt-3">
        <div class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">What this provider answers</div>
        <p class="text-[11px] text-muted">
          {meta.primitives.map((p) => PRIMITIVE_LABEL[p] ?? p).join(' · ')}
          {#if !meta.fansOut}· one question per round trip{:else}· several questions in one round trip{/if}
        </p>
        <p class="text-[11px] text-muted">Confidence: {CALIBRATION_NOTE[meta.calibration] ?? meta.calibration}</p>
        {#if meta.country !== 'your hardware' && meta.country !== '—'}
          <p class="text-[11px] text-warn">
            Ticket and message text is sent to this provider to be judged. It leaves your instance.
          </p>
        {/if}
        <div class="flex items-center gap-2 pt-1">
          <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Timeout</span>
          <Input
            size="sm"
            type="number"
            value={String(cfg?.timeoutMs ?? 4000)}
            onblur={(e) => Number(e.currentTarget.value) !== (cfg?.timeoutMs ?? 4000) && void apply({ timeoutMs: Number(e.currentTarget.value) })}
            class="w-24"
          />
          <span class="text-[11px] text-muted">ms — past this the caller runs its own path instead of waiting.</span>
        </div>
        <!-- ITS OWN SWITCH. Configuring a decision model is not consent to put
             every agent turn through a question per offered tool, so this is a
             separate key and separate consent. It measures only — nothing is
             ever pruned from a live request. -->
        <div class="pt-2">
          <Checkbox
            class="gap-2 text-[11px] text-fg"
            checked={toolShadow}
            onChange={(on) => void apply({ toolShadow: on })}
            label="Measure tool-offer pruning"
          />
          <p class="mt-1 pl-6 text-[11px] text-muted">
            Judges the tools each agent turn was offered against the ones it actually called, so
            the ledger can answer whether pruning would have broken the turn. Nothing is pruned.
            Costs one question per offered tool, on every turn — leave it off unless you are
            collecting the numbers.
          </p>
        </div>
      </div>
    {/if}

    <!-- THE CENSUS, with each site's own numbers beside its own switch. That
         adjacency is the whole point of this section: shadow mode's argument
         is that a site switches over when its own traffic says so, and a
         number an operator can read but not act on is a dashboard rather than
         a feature. -->
    <div class="mt-4 border-t border-line-subtle pt-3">
      <div class="mb-2 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">
        Where it is used
      </div>
      <div class="space-y-3">
        {#each allSites as s (s.id)}
          {@const m = shadowFor(s.id)}
          <div class="text-[11px]">
            <div class="flex flex-wrap items-center gap-2">
              {#if s.switchLivesAt}
                <span class="font-mono text-fg">{s.label}</span>
                <span class="text-ink-dim">· {s.primitive}</span>
              {:else}
                <Checkbox
                  class="gap-2 text-[11px] text-fg"
                  checked={s.on}
                  onChange={(on) => void applySite(s.id, { on })}
                  label={s.label}
                />
                <span class="text-ink-dim">· {s.primitive}</span>
                {#if s.on}
                  <span class="text-muted">acts at</span>
                  <Input
                    size="sm"
                    type="number"
                    step="0.05"
                    min="0"
                    max="1"
                    value={String(s.floor)}
                    onblur={(e) =>
                      Number(e.currentTarget.value) !== s.floor &&
                      void applySite(s.id, { floor: Number(e.currentTarget.value) })}
                    class="w-20"
                  />
                  <span class="text-muted">
                    {s.reads === 'lean' ? 'probability of yes' : 'certainty'}
                  </span>
                  {#if s.floor !== s.defaultFloor}
                    <Button size="sm" variant="link" onclick={() => void applySite(s.id, { floor: s.defaultFloor })}>
                      reset to {s.defaultFloor}
                    </Button>
                  {/if}
                {/if}
              {/if}
            </div>
            <p class="mt-0.5 pl-6 text-muted">{s.acts}</p>
            {#if s.switchLivesAt}
              <!-- Named rather than offered twice: two spellings of one switch
                   is how the two come to disagree. -->
              <p class="pl-6 text-ink-dim">Switched on from {s.switchLivesAt}.</p>
            {:else}
              <p class="pl-6 text-ink-dim">{s.floorWhy}</p>
            {/if}
            {#if s.id === 'channel-speech'}
              <!-- THE SECOND GATE, which is per-room by necessity and was
                   therefore the one control invisible from here. "Which rooms
                   would start talking if I flip this" is the question somebody
                   asks immediately before flipping it. Read-only: the switch
                   stays in each room's own settings. -->
              <p class="pl-6 text-muted">
                {#if speechRooms.length === 0}
                  No channel has its own Agent initiative switch on, so nothing would speak yet.
                {:else}
                  {speechRooms.filter((r) => r.agents > 0).length} of {speechRooms.length}
                  {speechRooms.length === 1 ? 'room allows' : 'rooms allow'} this and
                  {speechRooms.filter((r) => r.agents > 0).length === 1 ? 'has' : 'have'} an agent in
                  {speechRooms.filter((r) => r.agents > 0).length === 1 ? 'it' : 'them'}:
                  {#each speechRooms.filter((r) => r.agents > 0).slice(0, 8) as r, i (r.id)}<span
                      class="font-mono text-fg">#{r.name}</span
                    >{#if i < Math.min(speechRooms.filter((x) => x.agents > 0).length, 8) - 1}, {/if}{/each}{#if speechRooms.filter(
                    (r) => r.agents > 0,
                  ).length > 8}, and more{/if}. Each room's own switch is in its settings.
                {/if}
              </p>
            {/if}

            {#if m}
              <!-- SHADOW MODE — what the port WOULD have decided, beside what
                   the existing code did decide. A site that has not switched
                   over runs its own path and the port is asked the same
                   question with nobody listening, so a threshold can be chosen
                   from this install's own traffic rather than from a vendor's
                   published numbers. -->
              <div transition:slide={{ duration: 150 }} class="mt-1 pl-6">
                {#if m.compared > 0}
                  <p class="text-muted">
                    Agreed with the existing code on
                    <span class="text-fg">{pct(m.agreed, m.answered)}</span>
                    of {m.answered} answered
                    {#if m.compared > m.answered}
                      — the port said nothing on {m.compared - m.answered} of {m.compared}
                    {/if}
                    {#if m.p50Ms !== null}· {m.p50Ms}ms median, {m.p99Ms}ms p99{/if}
                  </p>
                {/if}
                {#if m.acted > 0}
                  <!-- Counted apart from the comparisons on purpose: once a
                       site is switched over there is no second answer, and
                       folding these in would make it read as 100% agreement
                       forever. -->
                  <p class="text-muted">
                    Decided <span class="text-fg">{m.acted}</span>
                    {m.acted === 1 ? 'turn' : 'turns'} on its own since it was switched over — those have
                    no baseline to agree with.
                  </p>
                {/if}
                {#if m.judgments > m.calibrated}
                  <!-- Counted over the rows that carried a SINGLE judgment. A
                       derived row — the tool keep-set, the brief's reorder —
                       has no one probability behind it, so warning about its
                       missing distribution would be warning about something
                       that was never supposed to be there. -->
                  <p class="text-warn">
                    {m.judgments - m.calibrated} of {m.judgments} answers carried no real
                    distribution, so their confidence is unavailable.
                  </p>
                {/if}
                {#if m.meanCertaintyWhenDisagreed !== null}
                  <p class="text-muted">
                    When it disagreed, mean certainty was
                    <span class="text-fg">{m.meanCertaintyWhenDisagreed.toFixed(2)}</span>
                    — low means a threshold can filter the disagreements; high means read
                    them case by case before switching this site over.
                  </p>
                {/if}
                {#if m.models.length > 1}
                  <!-- TWO MODELS AT ONE SITE, which makes the rate above about
                       neither of them. An alias like jev-latest creates this
                       silently the day it starts pointing somewhere new, which
                       is why the ledger records the model that ANSWERED rather
                       than the one we asked for. -->
                  <p class="text-warn">
                    {m.models.length} models answered here —
                    {#each m.models as mm, i (mm.model)}<span class="font-mono text-fg">{mm.model}</span>
                      ({num(mm.judgments)}){#if i < m.models.length - 1}, {/if}{/each}
                    — so this agreement rate is not about either of them.
                  </p>
                {:else if m.models.length === 1}
                  {@const only = m.models[0]}
                  <p class="text-muted">
                    Answered by <span class="font-mono text-fg">{only?.model ?? '—'}</span>.
                  </p>
                {/if}
                {#if m.metered > 0}
                  <!-- WHAT IT COST. These calls do not go through the metered
                       relay — a decision is not a conversation — so they never
                       reach the token ledger and this is the only place a
                       site's spend is visible at all. -->
                  <p class="text-muted">
                    <span class="text-fg">{num(m.tokensIn)}</span> billable input tokens over
                    {num(m.metered)} reported {m.metered === 1 ? 'call' : 'calls'}
                    {#if m.metered < m.compared + m.acted}
                      — the other {num(m.compared + m.acted - m.metered)} reported no usage
                    {/if}
                  </p>
                {:else if m.compared + m.acted > 0}
                  <p class="text-ink-dim">This provider reports no token usage.</p>
                {/if}
                {#if m.compared + m.acted > 0}
                  <Button size="sm" variant="link" onclick={() => void showRows(s.id)}>
                    {openRows === s.id ? 'Hide the rows' : 'Read the rows'}
                  </Button>
                {/if}
                {#if openRows === s.id}
                  <div transition:slide={{ duration: 150 }} class="mt-1">
                    {#if loadingRows}
                      <p class="text-ink-dim">Reading…</p>
                    {:else if rows.length === 0}
                      <p class="text-ink-dim">Nothing recorded.</p>
                    {:else}
                      <!-- Disagreements first: they are what is being audited,
                           and an operator should not page past the easy rows to
                           reach them. The SUBJECT is a reference, never the
                           text — a ticket message is somebody's words and this
                           ledger is the wrong place to accumulate them. -->
                      <table class="w-full text-[10px]">
                        <thead class="text-ink-dim">
                          <tr class="text-left">
                            <th class="pr-2 font-normal">subject</th>
                            <th class="pr-2 font-normal">was</th>
                            <th class="pr-2 font-normal">judged</th>
                            <th class="pr-2 font-normal">certainty</th>
                            <th class="pr-2 font-normal">model</th>
                            <th class="font-normal">when</th>
                          </tr>
                        </thead>
                        <tbody>
                          {#each rows as r, i (`${r.at}-${i}`)}
                            <tr class={r.agreed === false ? 'text-warn' : 'text-muted'}>
                              <td class="max-w-[14rem] truncate pr-2 font-mono">{r.subjectRef ?? '—'}</td>
                              <td class="pr-2 font-mono">{r.baseline ?? '—'}</td>
                              <td class="pr-2 font-mono">{r.portAnswer ?? '— (silent)'}</td>
                              <td class="pr-2 font-mono">
                                {r.certainty === null ? '—' : r.certainty.toFixed(2)}{r.calibrated
                                  ? ''
                                  : '*'}
                              </td>
                              <td class="max-w-[10rem] truncate pr-2 font-mono">{r.model ?? '—'}</td>
                              <td class="font-mono">{r.at?.slice(0, 16) ?? '—'}</td>
                            </tr>
                          {/each}
                        </tbody>
                      </table>
                      <p class="mt-1 text-ink-dim">
                        Highlighted rows are the disagreements. A certainty marked * carried no real
                        distribution. "— (silent)" is a call the port did not answer, which counts in
                        the denominator above.
                      </p>
                    {/if}
                  </div>
                {/if}
              </div>
            {:else if s.measuredAt}
              <!-- Not an empty ledger: this site has nothing to compare
                   against, and says where its numbers are instead. -->
              <p class="pl-6 text-ink-dim">Measured as {s.measuredAt}.</p>
            {:else if !s.switchLivesAt}
              <p class="pl-6 text-ink-dim">
                No comparisons recorded yet{data?.configured ? '' : ' — nothing is configured'}.
              </p>
            {/if}
          </div>
        {/each}
      </div>
    </div>

    {#if result}
      <div transition:slide={{ duration: 150 }} class="mt-3 border-t border-line-subtle pt-3 text-xs">
        {#if result.ok}
          <p class="text-fg">
            Answered in {result.latencyMs}ms — {result.provider}{result.model ? ` · ${result.model}` : ''}
          </p>
          <p class="mt-1 text-muted">
            Asked whether a report of a broken deploy blocking the team is urgent, and answered
            <span class="font-mono text-fg">{((result.probability ?? 0) * 100).toFixed(0)}%</span> yes.
            Near 100% means it is wired up and reading the question the way we intend.
          </p>
          {#if result.calibrated === false}
            <p class="mt-1 text-warn">
              The answer carried no real distribution, so its confidence is unavailable. This provider
              will still answer, but every call site that gates on confidence will fall back to its own
              path. For a registered chat model this usually means the endpoint does not serve logprobs.
            </p>
          {/if}
        {:else}
          <!-- The PROVIDER'S own sentence where there is one. `refused` means
               it answered and said why; the rest are ours because there was no
               reply to quote. -->
          <p class="text-danger">{result.reason}</p>
          {#if result.kind === 'refused' && unknownModel}
            <p class="mt-1 text-muted">
              The model id is the usual cause — see the detected list above.
            </p>
          {/if}
        {/if}
      </div>
    {/if}
    {/snippet}
  </QueryState>
</Panel>
