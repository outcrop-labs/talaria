<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import Button from '@/components/ui/Button.svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import DangerLink from '@/components/ui/DangerLink.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Modal from '@/components/ui/Modal.svelte'
  import Panel from '@/components/ui/Panel.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import SectionHeader from '@/components/ui/SectionHeader.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import { confirm } from '@/components/ui/confirm.svelte'
  import { errorMessage } from '@/lib/fetch-json'
  import { relativeTime } from '@/lib/fleet'
  import { formatTokens } from '@/lib/format'
  import { slide } from '@/lib/motion'
  import { pushToast, toastError } from '@/lib/toast.svelte'
  import {
    PRICING_QUERY_KEYS,
    UNPRICED_ANCHOR,
    canRetry,
    clearPrice,
    emptyPriceForm,
    endpointLabel,
    includedBody,
    manualBody,
    reasonMeta,
    repricedLine,
    retryLookup,
    setPrice,
    settingLabel,
    tokensLine,
    useUnpriced,
    type PriceForm,
    type PriceTarget,
    type UnpricedModel,
  } from './pricing'

  // ADMIN ONLY — the caller mounts this for admins; the api refuses anyone
  // else. Every cloud model whose last-30-days usage has no price, why, and
  // the three ways to fix it: a manual rate, "included in a plan" ($0,
  // labelled), or a retried lookup. Pricing is read-time, so each fix
  // re-prices that model's past calls at once — the line under the row says
  // how many and at what.
  const query = useUnpriced(true)
  const qc = useQueryClient()
  const data = $derived(query.data)
  const unpriced = $derived(data?.unpriced ?? [])
  const prices = $derived(data?.prices ?? [])

  const refresh = () => Promise.all(PRICING_QUERY_KEYS.map((queryKey) => qc.invalidateQueries({ queryKey: [...queryKey] })))
  const key = (t: PriceTarget) => `${t.endpoint}\u0000${t.model}`

  // What the last action on a row said — kept on the row until it leaves the
  // list or is acted on again.
  let notes = $state<Record<string, string>>({})
  let retrying = $state<string | null>(null)

  // THE ALERT LINKS HERE (/observability/cost#unpriced): bring the panel into
  // view once it has something to show.
  let anchor = $state<HTMLElement | null>(null)
  let scrolled = false
  $effect(() => {
    if (scrolled || !anchor || data === undefined) return
    if (typeof location !== 'undefined' && location.hash === `#${UNPRICED_ANCHOR}`) {
      scrolled = true
      anchor.scrollIntoView({ block: 'start', behavior: 'smooth' })
    }
  })

  // ── Set price / Included: one dialog, two modes ──────────────────────────
  // The cache placeholders say what applies when the field is left blank.
  const RATE_FIELDS = [
    ['input', 'Input', 'e.g. 2.00'],
    ['output', 'Output', 'e.g. 8.00'],
    ['cacheWrite', 'Cache write', 'input × 1.25'],
    ['cacheRead', 'Cache read', 'input × 0.1'],
  ] as const
  let dialog = $state<{ target: UnpricedModel; mode: 'manual' | 'included' } | null>(null)
  let form = $state<PriceForm>(emptyPriceForm())
  let formError = $state<string | null>(null)
  let saving = $state(false)

  const openDialog = (target: UnpricedModel, mode: 'manual' | 'included') => {
    form = emptyPriceForm()
    formError = null
    dialog = { target, mode }
  }

  const save = async () => {
    if (!dialog) return
    const target = { endpoint: dialog.target.endpoint, model: dialog.target.model }
    let body: Record<string, unknown>
    if (dialog.mode === 'manual') {
      const built = manualBody(target, form)
      if (!built.ok) {
        formError = built.error
        return
      }
      body = built.body
    } else {
      body = includedBody(target, form.note)
    }
    saving = true
    formError = null
    try {
      const r = await setPrice(body)
      const line = repricedLine(r.repriced)
      notes[key(target)] = line
      pushToast({ title: dialog.mode === 'manual' ? 'Price saved' : 'Marked included in a plan', body: line, tone: 'success' })
      dialog = null
      await refresh()
    } catch (e) {
      formError = errorMessage(e)
    } finally {
      saving = false
    }
  }

  const retry = async (m: UnpricedModel) => {
    const target = { endpoint: m.endpoint, model: m.model }
    retrying = key(target)
    try {
      const r = await retryLookup(target)
      if (r.priced) {
        // The row leaves the list once it is priced, and its note with it —
        // so the answer rides a toast instead.
        pushToast({ title: `Found a published price for ${m.model}`, body: repricedLine(r.repriced), tone: 'success' })
      } else {
        notes[key(target)] = `Still no price: ${r.reason?.text ?? 'no reason recorded'}`
      }
      await refresh()
    } catch (e) {
      toastError('Price lookup failed', e)
    } finally {
      retrying = null
    }
  }

  const remove = async (target: PriceTarget) => {
    const ok = await confirm({
      title: 'Remove this pricing',
      message: `Stop pricing ${target.model} on ${endpointLabel(target.endpoint)} by hand? Its past calls go back to a published price if there is one, and to unpriced if not.`,
      confirmLabel: 'Remove',
      danger: true,
    })
    if (!ok) return
    try {
      const r = await clearPrice(target)
      pushToast({ title: 'Pricing removed', body: repricedLine(r.repriced), tone: 'info' })
      await refresh()
    } catch (e) {
      toastError('Could not remove the pricing', e)
    }
  }
</script>

<section bind:this={anchor} id={UNPRICED_ANCHOR} class="scroll-mt-4 space-y-4" data-testid="unpriced-panel">
  {#if query.isError && data === undefined}
    <QueryError title="Could not load unpriced models" error={query.error} onRetry={() => void query.refetch()} />
  {:else if data === undefined}
    <Panel><SkeletonRows /></Panel>
  {:else if unpriced.length > 0 || prices.length > 0}
    <Panel>
      <SectionHeader
        title="Unpriced models · 30 days"
        info="Cloud usage with no provider-reported charge, no published price and no pricing of yours. Set a price, mark it included in a plan (priced at $0 and labelled), or retry the lookup. Each fix re-prices that model's past calls."
        action={String(unpriced.length).padStart(2, '0')}
      />
      {#if unpriced.length === 0}
        <p class="text-xs text-muted">Every cloud model used in the last 30 days has a price.</p>
      {:else}
        <ul class="divide-y divide-line">
          {#each unpriced as m (key(m))}
            {@const meta = reasonMeta(m.reason.code)}
            <li class="space-y-2 py-3" data-testid="unpriced-row">
              <div class="flex flex-wrap items-baseline gap-x-3 gap-y-1">
                <span class="min-w-0 font-mono text-xs text-muted">{endpointLabel(m.endpoint)}</span>
                <span class="min-w-0 flex-1 truncate font-mono text-sm text-fg" title={m.model}>{m.model}</span>
                <span class="shrink-0 font-mono text-sm text-fg">{formatTokens(m.tokens)}</span>
                <span class="shrink-0 font-mono text-[11px] text-muted">{tokensLine(m)} · {m.generations} calls</span>
              </div>
              <div class="flex flex-wrap items-start gap-2">
                <Chip tone={meta.tone}>{meta.label}</Chip>
                <p class="min-w-0 flex-1 text-xs text-muted">
                  {m.reason.text}
                  {#if m.reason.checkedAt}<span class="text-ink-dim"> · checked {relativeTime(m.reason.checkedAt)}</span>{/if}
                </p>
              </div>
              <div class="flex flex-wrap items-center gap-2">
                <Button size="sm" variant="outline" onclick={() => openDialog(m, 'manual')}>Set price</Button>
                <Button size="sm" variant="outline" onclick={() => openDialog(m, 'included')}>Included in a plan</Button>
                <span title={canRetry(m) ? undefined : 'No configured endpoint to ask — price it by hand instead.'}>
                  <Button
                    size="sm"
                    variant="ghost"
                    disabled={!canRetry(m) || retrying !== null}
                    onclick={() => void retry(m)}
                  >
                    {retrying === key(m) ? 'Looking up' : 'Retry lookup'}
                  </Button>
                </span>
              </div>
              {#if notes[key(m)]}
                <p transition:slide={{ duration: 150 }} class="text-xs text-fg">{notes[key(m)]}</p>
              {/if}
            </li>
          {/each}
        </ul>
      {/if}
    </Panel>

    {#if prices.length > 0}
      <Panel>
        <SectionHeader
          title="Priced by an admin"
          info="Your pricing for models no source publishes a price for. The provider's own charge always wins, and a published price wins over a manual rate; included-in-a-plan wins over a published price."
          action={String(prices.length).padStart(2, '0')}
        />
        <ul class="divide-y divide-line">
          {#each prices as s (key(s))}
            <li class="flex flex-wrap items-baseline gap-x-3 gap-y-1 py-3" data-testid="price-setting">
              <span class="font-mono text-xs text-muted">{endpointLabel(s.endpoint)}</span>
              <span class="min-w-0 flex-1 truncate font-mono text-sm text-fg" title={s.model}>{s.model}</span>
              <Chip tone={s.mode === 'included' ? 'accent' : 'neutral'}>{s.mode === 'included' ? 'included' : 'manual'}</Chip>
              <span class="font-mono text-[11px] text-fg">{settingLabel(s)}</span>
              <span class="w-full text-xs text-muted">
                {#if s.shadowed}
                  A published price or the provider's own charge now covers this model; this pricing is not used.
                {:else}
                  Prices {s.rowsPriced} call{s.rowsPriced === 1 ? '' : 's'} ({formatTokens(s.tokensPriced)} tokens) in the last 30 days.
                {/if}
                {#if s.note}<span> · {s.note}</span>{/if}
                {#if s.setBy}<span class="text-ink-dim"> · {s.setBy}{s.updatedAt ? `, ${relativeTime(s.updatedAt)}` : ''}</span>{/if}
                <DangerLink class="ml-2" onClick={() => void remove({ endpoint: s.endpoint, model: s.model })}>Remove</DangerLink>
              </span>
            </li>
          {/each}
        </ul>
      </Panel>
    {/if}
  {/if}
</section>

<Modal
  open={dialog !== null}
  onClose={() => (dialog = null)}
  title={dialog?.mode === 'included' ? 'Included in a plan' : 'Set a price'}
>
  {#if dialog}
    <div class="space-y-4">
      <p class="font-mono text-xs text-muted">
        {endpointLabel(dialog.target.endpoint)} · <span class="text-fg">{dialog.target.model}</span>
      </p>
      {#if dialog.mode === 'manual'}
        <p class="text-xs text-muted">
          Dollars per 1M tokens. A price the provider publishes later takes over from this one; the provider's own
          charge always wins.
        </p>
        <div class="grid grid-cols-2 gap-3">
          {#each RATE_FIELDS as [field, label, placeholder] (field)}
            <div>
              <label for={`price-${field}`} class="mb-1 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">{label}</label>
              <Input id={`price-${field}`} inputmode="decimal" bind:value={form[field]} {placeholder} class="font-[var(--font-mono)]" />
            </div>
          {/each}
        </div>
      {:else}
        <p class="text-xs text-muted">
          Calls to this model are priced at $0 and labelled “included”, never shown as a silent zero. Use it for a
          flat-rate plan (a coding plan, a subscription) or a free tier. The provider's own charge, when it sends one,
          still wins.
        </p>
      {/if}
      <div>
        <label for="price-note" class="mb-1 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Note</label>
        <Input
          id="price-note"
          bind:value={form.note}
          placeholder={dialog.mode === 'included' ? 'e.g. Z.ai Coding Plan' : 'e.g. from the provider’s pricing page'}
          maxlength={200}
        />
      </div>
      {#if formError}<div class="text-sm text-danger" data-testid="price-form-error">{formError}</div>{/if}
      <div class="flex justify-end gap-2 border-t border-line pt-3">
        <Button variant="ghost" size="sm" onclick={() => (dialog = null)}>Cancel</Button>
        <Button size="sm" onclick={() => void save()} disabled={saving}>
          {saving ? 'Saving' : dialog.mode === 'included' ? 'Mark included' : 'Save price'}
        </Button>
      </div>
    </div>
  {/if}
</Modal>
