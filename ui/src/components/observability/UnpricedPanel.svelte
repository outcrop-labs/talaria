<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import Button from '@/components/ui/Button.svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import DangerLink from '@/components/ui/DangerLink.svelte'
  import Disclosure from '@/components/ui/Disclosure.svelte'
  import InfoTip from '@/components/ui/InfoTip.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Modal from '@/components/ui/Modal.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import SettingsField from '@/components/settings/SettingsField.svelte'
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import SettingsRowMenu from '@/components/settings/SettingsRowMenu.svelte'
  import SettingsSection from '@/components/settings/SettingsSection.svelte'
  import type { ContextMenuEntry } from '@/components/ui/context-menu.svelte'
  import { confirm } from '@/components/ui/confirm.svelte'
  import { errorMessage } from '@/lib/fetch-json'
  import { relativeTime } from '@/lib/fleet'
  import { formatTokens } from '@/lib/format'
  import { slide } from '@/lib/motion'
  import { pushToast, toastError } from '@/lib/toast.svelte'
  import {
    PRICING_QUERY_KEYS,
    UNPRICED_ANCHOR,
    canFix,
    canRetry,
    pricedAsLabel,
    suggestionBody,
    suggestionLabel,
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
  //
  // Unpriced models need someone, so they show whenever there are any. The
  // pricing an admin already set, and the close matches, are administration:
  // they wait behind "Manage pricing".
  const query = useUnpriced(true)
  const qc = useQueryClient()
  const data = $derived(query.data)
  const unpriced = $derived(data?.unpriced ?? [])
  const prices = $derived(data?.prices ?? [])
  const fuzzy = $derived(data?.fuzzy ?? [])

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
    ['input', 'Input', '2.00'],
    ['output', 'Output', '8.00'],
    ['cacheWrite', 'Cache write', 'input × 1.25'],
    ['cacheRead', 'Cache read', 'input × 0.1'],
  ] as const
  let dialog = $state<{ target: PriceTarget; mode: 'manual' | 'included' } | null>(null)
  let form = $state<PriceForm>(emptyPriceForm())
  let formError = $state<string | null>(null)
  let saving = $state(false)

  const openDialog = (target: PriceTarget, mode: 'manual' | 'included') => {
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

  // Accept a suggestion: OpenRouter's rate for the same name, saved as the
  // admin's own manual price (so it is labelled manual, and replaceable).
  const acceptSuggestion = async (m: UnpricedModel) => {
    if (!m.suggestion) return
    try {
      const r = await setPrice(suggestionBody({ endpoint: m.endpoint, model: m.model }, m.suggestion))
      pushToast({ title: 'Suggested price saved', body: repricedLine(r.repriced), tone: 'success' })
      await refresh()
    } catch (e) {
      toastError('Could not save the suggested price', e)
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

  const sentence = (s: string) => s.charAt(0).toUpperCase() + s.slice(1)
  const managed = $derived(
    [
      prices.length > 0 ? `${prices.length} set by an admin` : null,
      fuzzy.length > 0 ? `${fuzzy.length} priced by a close match` : null,
    ]
      .filter(Boolean)
      .join(' · '),
  )

  // Everything but "Set price", which is the row's one visible button.
  const fixMenu = (m: UnpricedModel): ContextMenuEntry[] => [
    ...(m.suggestion ? [{ label: 'Use the suggested rate', onSelect: () => void acceptSuggestion(m) }] : []),
    { label: 'Mark as included in a plan', onSelect: () => openDialog(m, 'included') },
    {
      label: retrying === key(m) ? 'Looking up' : canRetry(m) ? 'Retry lookup' : 'Retry lookup (no endpoint to ask)',
      disabled: !canRetry(m) || retrying !== null,
      onSelect: () => void retry(m),
    },
  ]
</script>

{#snippet identity(target: PriceTarget)}
  {#if target.model}
    <span class="font-mono text-xs font-normal" title={target.model}>{target.model}</span>
  {:else}
    No model id
  {/if}
{/snippet}

{#snippet endpoint(target: PriceTarget)}
  <span class="shrink-0 font-mono text-xs text-muted">{endpointLabel(target.endpoint)}</span>
{/snippet}

{#snippet heading(text: string, info: string)}
  <h3 class="mb-2 flex items-center gap-2 font-sans text-xs font-medium leading-4 text-muted">{text}<InfoTip text={info} /></h3>
{/snippet}

<!-- `empty:hidden`: with nothing to price this section is an empty element,
     and it must not take a gap in the tab's column. -->
<section bind:this={anchor} id={UNPRICED_ANCHOR} class="scroll-mt-4 space-y-8 empty:hidden" data-testid="unpriced-panel">
  {#if query.isError && data === undefined}
    <QueryError title="Could not load unpriced models" error={query.error} onRetry={() => void query.refetch()} />
  {:else if data === undefined}
    <SettingsGroup><div class="px-5 py-4"><SkeletonRows /></div></SettingsGroup>
  {:else}
    {#if unpriced.length > 0}
      <SettingsSection
        title="Unpriced models"
        description="Cloud models used in the last 30 days that have no price, so their spend is missing."
        info="Cloud usage with no provider-reported charge, no published price and no pricing of yours. Set a price, mark it included in a plan (priced at $0 and labeled), or retry the lookup. Each fix re-prices that model's past calls."
      >
        <SettingsGroup>
          {#each unpriced as m (key(m))}
            {@const reason = reasonMeta(m.reason.code)}
            <div data-testid="unpriced-row">
              <SettingsRow>
                {#snippet label()}{@render identity(m)}{/snippet}
                {#snippet meta()}
                  {@render endpoint(m)}
                  <Chip tone={reason.tone}>{sentence(reason.label)}</Chip>
                {/snippet}
                {#snippet hint()}
                  <p>
                    {m.reason.text}{#if m.reason.checkedAt}, checked {relativeTime(m.reason.checkedAt)}{/if}
                  </p>
                  <p class="tabular-nums">{formatTokens(m.tokens)} tokens ({tokensLine(m)}) in {m.generations.toLocaleString('en-US')} calls</p>
                  {#if m.suggestion}
                    <p>
                      Suggested: the rate of <span class="font-mono text-xs">{m.suggestion.matchedId}</span>, {suggestionLabel(m.suggestion)}
                    </p>
                  {/if}
                  {#if !canFix(m)}
                    <p>A call booked without a model id cannot be priced. These rows leave the 30-day window on their own.</p>
                  {/if}
                  {#if notes[key(m)]}
                    <p transition:slide={{ duration: 150 }} class="text-fg">{notes[key(m)]}</p>
                  {/if}
                {/snippet}
                {#if canFix(m)}
                  <Button size="sm" variant="outline" onclick={() => openDialog(m, 'manual')}>Set price</Button>
                  <SettingsRowMenu label={`More ways to price ${m.model}`} items={() => fixMenu(m)} />
                {/if}
              </SettingsRow>
            </div>
          {/each}
        </SettingsGroup>
      </SettingsSection>
    {/if}

    {#if fuzzy.length > 0 || prices.length > 0}
      <Disclosure variant="card">
        {#snippet title()}Manage pricing<span class="ml-2 font-normal text-muted">{managed}</span>{/snippet}
        <!-- The last row brings its own padding to the foot of the card. -->
        <div class="-mb-4 space-y-2">
          {#if fuzzy.length > 0}
            <section>
              {@render heading(
                'Priced by a close match',
                'A snapshot priced as its alias, or a path priced by its last segment. If you know the real rate, set it: a manual rate beats a close match, but never the provider\'s own charge or an exact published price.',
              )}
              <SettingsGroup bare class="border-b-0">
                {#each fuzzy as f (key(f))}
                  <div data-testid="fuzzy-row">
                    <SettingsRow>
                      {#snippet label()}{@render identity(f)}{/snippet}
                      {#snippet meta()}
                        {@render endpoint(f)}
                        <Chip tone="warn">{sentence(f.matchKind)}</Chip>
                      {/snippet}
                      {#snippet hint()}
                        <span class="tabular-nums">
                          {sentence(pricedAsLabel(f.matchedId))} · {formatTokens(f.tokens)} tokens in {f.generations.toLocaleString('en-US')} calls
                        </span>
                      {/snippet}
                      <Button size="sm" variant="outline" onclick={() => openDialog(f, 'manual')}>Set price</Button>
                      <SettingsRowMenu
                        label={`More ways to price ${f.model}`}
                        items={[{ label: 'Mark as included in a plan', onSelect: () => openDialog(f, 'included') }]}
                      />
                    </SettingsRow>
                  </div>
                {/each}
              </SettingsGroup>
            </section>
          {/if}

          {#if prices.length > 0}
            <section>
              {@render heading(
                'Priced by an admin',
                'Your own pricing for models no source publishes a price for. The provider\'s own charge always wins, and a published price wins over a manual rate. Included in a plan wins over a published price.',
              )}
              <SettingsGroup bare class="border-b-0">
                {#each prices as s (key(s))}
                  <div data-testid="price-setting">
                    <SettingsRow>
                      {#snippet label()}{@render identity(s)}{/snippet}
                      {#snippet meta()}
                        {@render endpoint(s)}
                        <Chip tone={s.mode === 'included' ? 'accent' : 'neutral'}>{s.mode === 'included' ? 'Included' : 'Manual'}</Chip>
                      {/snippet}
                      {#snippet hint()}
                        <p class="tabular-nums">{settingLabel(s)}</p>
                        <p>
                          {#if s.shadowed}
                            A published price or the provider's own charge now covers this model, so this pricing is not used.
                          {:else}
                            Prices {s.rowsPriced.toLocaleString('en-US')} call{s.rowsPriced === 1 ? '' : 's'} ({formatTokens(s.tokensPriced)} tokens) in the last 30 days.
                          {/if}
                          {#if s.note}{s.note}.{/if}
                          {#if s.setBy}Set by {s.setBy}{s.updatedAt ? `, ${relativeTime(s.updatedAt)}` : ''}.{/if}
                        </p>
                      {/snippet}
                      <DangerLink onClick={() => void remove({ endpoint: s.endpoint, model: s.model })}>Remove pricing</DangerLink>
                    </SettingsRow>
                  </div>
                {/each}
              </SettingsGroup>
            </section>
          {/if}
        </div>
      </Disclosure>
    {/if}
  {/if}
</section>

<Modal
  open={dialog !== null}
  onClose={() => (dialog = null)}
  title={dialog?.mode === 'included' ? 'Mark as included in a plan' : 'Set a price'}
>
  {#if dialog}
    <div class="space-y-5">
      <dl class="grid grid-cols-[5.5rem_minmax(0,1fr)] gap-x-4 gap-y-1 font-sans text-[13px] leading-5">
        <dt class="text-muted">Model</dt>
        <dd class="min-w-0 break-all font-mono text-xs leading-5 text-fg">{dialog.target.model}</dd>
        <dt class="text-muted">Endpoint</dt>
        <dd class="min-w-0 break-all font-mono text-xs leading-5 text-fg">{endpointLabel(dialog.target.endpoint)}</dd>
      </dl>
      {#if dialog.mode === 'manual'}
        <p class="font-sans text-[13px] leading-5 text-muted">
          Dollars per 1M tokens. A published price or the provider's own charge takes over when one arrives.
        </p>
        <div class="grid grid-cols-2 gap-x-3 gap-y-5">
          {#each RATE_FIELDS as [field, label, placeholder] (field)}
            <SettingsField {label} for={`price-${field}`}>
              <Input id={`price-${field}`} size="sm" inputmode="decimal" bind:value={form[field]} {placeholder} class="tabular-nums" />
            </SettingsField>
          {/each}
        </div>
      {:else}
        <p class="font-sans text-[13px] leading-5 text-muted">
          Calls to this model are priced at $0 and labeled as included, for a flat-rate plan or a free tier. The
          provider's own charge still wins when it sends one.
        </p>
      {/if}
      <SettingsField label="Note" for="price-note">
        <Input
          id="price-note"
          size="sm"
          bind:value={form.note}
          placeholder={dialog.mode === 'included' ? 'Z.ai Coding Plan' : 'From the provider’s pricing page'}
          maxlength={200}
        />
      </SettingsField>
      {#if formError}
        <div class="font-sans text-[13px] leading-5 text-danger" role="alert" data-testid="price-form-error">{formError}</div>
      {/if}
    </div>
  {/if}
  {#snippet footer()}
    <div class="flex justify-end gap-2">
      <Button variant="ghost" size="sm" onclick={() => (dialog = null)}>Cancel</Button>
      <Button size="sm" onclick={() => void save()} disabled={saving}>
        {saving ? 'Saving' : dialog?.mode === 'included' ? 'Mark included' : 'Save price'}
      </Button>
    </div>
  {/snippet}
</Modal>
