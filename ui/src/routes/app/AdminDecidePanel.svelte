<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import Button from '@/components/ui/Button.svelte'
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
  }
  type SiteShadow = {
    site: string
    compared: number
    answered: number
    agreed: number
    calibrated: number
    p50Ms: number | null
    p99Ms: number | null
    meanCertaintyWhenDisagreed: number | null
    provider: string | null
    newest: string | null
  }
  type TestResult = {
    ok: boolean
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
  const pct = (n: number, d: number) => (d > 0 ? `${Math.round((n / d) * 100)}%` : '—')
  const savedFlash = useSavedFlash()

  let key = $state('')
  let savingKey = $state(false)
  let testing = $state(false)
  let result = $state<TestResult | null>(null)

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
        <!-- A free-text model id rather than a picker: the catalog here is a
             documented fallback list at best, and an operator running a model
             we have never heard of must be able to name it. -->
        <Input
          size="sm"
          value={cfg?.model ?? ''}
          onblur={(e) => e.currentTarget.value !== (cfg?.model ?? '') && void apply({ model: e.currentTarget.value || null })}
          placeholder={meta.fallbackModels[0] ?? 'endpoint:model'}
          list={`decide-models-${meta.id}`}
          class="w-56"
        />
        {#if meta.fallbackModels.length > 0}
          <datalist id={`decide-models-${meta.id}`}>
            {#each meta.fallbackModels as m (m)}<option value={m}></option>{/each}
          </datalist>
        {/if}
      {/if}

      {#if meta}
        <Button size="sm" variant="outline" onclick={() => void test()} disabled={testing || !data?.configured}>
          {testing ? 'Testing…' : 'Test'}
        </Button>
      {/if}
      {#if savedFlash.saved}<span class="text-xs text-success">Saved</span>{/if}
    </div>

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
      </div>
    {/if}

    {#if shadow.length > 0}
      <!-- SHADOW MODE — what the port WOULD have decided, beside what the
           existing code did decide. Nothing here affects behaviour: a wired
           site runs its own path and the port is asked the same question with
           nobody listening, so a threshold can be chosen from this install's
           own traffic rather than from a vendor's published numbers. -->
      <div transition:slide={{ duration: 150 }} class="mt-4 border-t border-line-subtle pt-3">
        <div class="mb-2 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">
          Shadow comparisons
        </div>
        <div class="space-y-2">
          {#each shadow as s (s.site)}
            <div class="text-[11px]">
              <div class="font-mono text-fg">{s.site}</div>
              <p class="text-muted">
                Agreed with the existing code on
                <span class="text-fg">{pct(s.agreed, s.answered)}</span>
                of {s.answered} answered
                {#if s.compared > s.answered}
                  — the port said nothing on {s.compared - s.answered} of {s.compared}
                {/if}
                {#if s.p50Ms !== null}· {s.p50Ms}ms median, {s.p99Ms}ms p99{/if}
              </p>
              {#if s.answered > s.calibrated}
                <p class="text-warn">
                  {s.answered - s.calibrated} of {s.answered} answers carried no real
                  distribution, so their confidence is unavailable.
                </p>
              {/if}
              {#if s.meanCertaintyWhenDisagreed !== null}
                <p class="text-muted">
                  When it disagreed, mean certainty was
                  <span class="text-fg">{s.meanCertaintyWhenDisagreed.toFixed(2)}</span>
                  — low means a threshold can filter the disagreements; high means read
                  them case by case before switching this site over.
                </p>
              {/if}
            </div>
          {/each}
        </div>
      </div>
    {/if}

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
          <p class="text-danger">{result.reason}</p>
        {/if}
      </div>
    {/if}
    {/snippet}
  </QueryState>
</Panel>
