// Unpriced cloud usage, as the Cost tab's admin panel reads and fixes it
// (GET /api/cost/unpriced, PUT/DELETE /api/cost/prices,
// POST /api/cost/unpriced/lookup — all admin). The decisions — the price
// precedence and each model's reason — are the api's; this module only
// names them, shapes the request bodies, and says what a save did.
import { createQuery } from '@tanstack/svelte-query'
import type { ChipTone } from '@/components/ui/chip'
import { delJson, getJson, postJson, putJson } from '@/lib/fetch-json'
import { formatTokens, formatUsd } from '@/lib/format'
import { resolve, type MaybeGetter } from '@/lib/reactive-arg'

/** The Cost tab anchor the "Unpriced cloud usage" alert links to. */
export const UNPRICED_ANCHOR = 'unpriced'

export const REASON_CODES = [
  'no_match',
  'variable_price',
  'suggested',
  'source_failed',
  'not_checked',
  'stale',
  'endpoint_gone',
  'no_endpoint',
  'no_model',
] as const
export type ReasonCode = (typeof REASON_CODES)[number]

/** A short chip label and tint per reason. A source that failed is a real
 *  failure (danger); a catalog miss or a stale price is attention (warn);
 *  the rest are facts about the ledger, not faults (neutral). */
export const REASON_META: Record<ReasonCode, { label: string; tone: ChipTone }> = {
  no_match: { label: 'no published price', tone: 'warn' },
  variable_price: { label: 'variable price', tone: 'warn' },
  suggested: { label: 'suggested price', tone: 'accent' },
  source_failed: { label: "source didn't answer", tone: 'danger' },
  not_checked: { label: 'not looked up yet', tone: 'neutral' },
  stale: { label: 'price no longer applies', tone: 'warn' },
  endpoint_gone: { label: 'endpoint removed', tone: 'neutral' },
  no_endpoint: { label: 'no endpoint recorded', tone: 'neutral' },
  no_model: { label: 'no model recorded', tone: 'neutral' },
}

export const reasonMeta = (code: string): { label: string; tone: ChipTone } =>
  (REASON_META as Record<string, { label: string; tone: ChipTone }>)[code] ?? { label: code, tone: 'neutral' }

export interface UnpricedReason {
  code: string
  text: string
  checkedAt?: string
}

export interface UnpricedModel {
  /** The ledger's endpoint text; '' when the rows carry none. */
  endpoint: string
  endpointExists: boolean
  provider: string | null
  model: string
  generations: number
  promptTokens: number
  completionTokens: number
  cacheWriteTokens: number
  cacheReadTokens: number
  tokens: number
  firstSeen: string | null
  lastSeen: string | null
  reason: UnpricedReason
  /** OpenRouter's entry for a model another host serves under the same
   *  last path segment — offered, never applied. */
  suggestion?: Suggestion
}

export interface Suggestion {
  matchedId: string
  in: number
  out: number
  cacheWrite: number | null
  cacheRead: number | null
}

/** A model priced by a FUZZY catalog match (a stripped tag, date or path
 *  segment found the entry) — an admin's manual rate overrides it. */
export interface FuzzyPriced {
  endpoint: string
  model: string
  matchedId: string
  matchKind: string
  generations: number
  tokens: number
  cost: number
}

export type PriceMode = 'manual' | 'included'

export interface PriceSetting {
  endpoint: string
  model: string
  mode: PriceMode
  in: number | null
  out: number | null
  cacheWrite: number | null
  cacheRead: number | null
  note: string | null
  setBy: string | null
  updatedAt: string | null
  rowsPriced: number
  tokensPriced: number
  /** Has usage, prices none of it: the provider's charge or a published
   *  price got there first. */
  shadowed: boolean
}

export interface Repriced {
  rows: number
  pricedRows: number
  unpricedRows: number
  tokens: number
  cost: number
  basis: string | null
}

export interface UnpricedOverview {
  unpriced: UnpricedModel[]
  prices: PriceSetting[]
  /** Absent from an api that predates it. */
  fuzzy?: FuzzyPriced[]
}

export function useUnpriced(enabled: MaybeGetter<boolean>) {
  return createQuery(() => ({
    queryKey: ['cost-unpriced'],
    queryFn: () => getJson<UnpricedOverview>('/api/cost/unpriced'),
    enabled: resolve(enabled),
    refetchInterval: 60_000,
  }))
}

/** Every query a pricing change moves: this list, the ledger totals, and the
 *  alert that counts unpriced tokens. */
export const PRICING_QUERY_KEYS = [['cost-unpriced'], ['cost'], ['alerts']] as const

/** What the panel calls the endpoint a row was booked under. */
export const endpointLabel = (endpoint: string): string => (endpoint === '' ? '(no endpoint)' : endpoint)

/** Rows booked without a model id can be priced by nothing — they age out of
 *  the window — so they offer no fixes (the api would refuse them). */
export const canFix = (m: Pick<UnpricedModel, 'model'>): boolean => m.model !== ''

/** "priced as openai/gpt-4o (fuzzy match)". */
export const pricedAsLabel = (matchedId: string): string => `priced as ${matchedId} (fuzzy match)`

/** The manual-price body that accepts a suggestion's rates as they stand. */
export const suggestionBody = (target: PriceTarget, s: Suggestion): Record<string, unknown> => ({
  endpoint: target.endpoint,
  model: target.model,
  mode: 'manual',
  priceInPerMtok: s.in,
  priceOutPerMtok: s.out,
  priceCacheWritePerMtok: s.cacheWrite,
  priceCacheReadPerMtok: s.cacheRead,
  note: `OpenRouter's rate for ${s.matchedId}, accepted as a suggestion`,
})

/** "$0.02 in · $0.076 out per 1M" for a suggestion. */
export const suggestionLabel = (s: Suggestion): string =>
  settingLabel({ mode: 'manual', in: s.in, out: s.out, cacheWrite: s.cacheWrite, cacheRead: s.cacheRead })

/** Retry asks a configured cloud endpoint; history booked under a deleted
 *  endpoint, or none, can only be priced by hand. */
export const canRetry = (m: Pick<UnpricedModel, 'endpoint' | 'endpointExists'>): boolean =>
  m.endpoint !== '' && m.endpointExists

/** "1.2M in · 40k out · 900k cache" — the cache figure only when there is one. */
export function tokensLine(m: Pick<UnpricedModel, 'promptTokens' | 'completionTokens' | 'cacheWriteTokens' | 'cacheReadTokens'>): string {
  const cache = m.cacheWriteTokens + m.cacheReadTokens
  const parts = [`${formatTokens(m.promptTokens)} in`, `${formatTokens(m.completionTokens)} out`]
  if (cache > 0) parts.push(`${formatTokens(cache)} cache`)
  return parts.join(' · ')
}

/** A rate field as typed. Blank is "not given"; anything that is not a
 *  non-negative number is an error the form names. */
export function parseRate(text: string): { ok: true; value: number | null } | { ok: false } {
  const t = text.trim()
  if (t === '') return { ok: true, value: null }
  const n = Number(t.replace(/^\$/, ''))
  return Number.isFinite(n) && n >= 0 ? { ok: true, value: n } : { ok: false }
}

export interface PriceForm {
  input: string
  output: string
  cacheWrite: string
  cacheRead: string
  note: string
}

export const emptyPriceForm = (): PriceForm => ({ input: '', output: '', cacheWrite: '', cacheRead: '', note: '' })

export interface PriceTarget {
  endpoint: string
  model: string
}

/** The PUT body for a manual rate, or the sentence that says which field is
 *  wrong. Input and output are required; the cache rates are optional (the
 *  api bills cache tokens at the input rate × 1.25 / × 0.1 without them). */
export function manualBody(
  target: PriceTarget,
  form: PriceForm,
): { ok: true; body: Record<string, unknown> } | { ok: false; error: string } {
  const fields = [
    ['input', 'Input', form.input],
    ['output', 'Output', form.output],
    ['cacheWrite', 'Cache write', form.cacheWrite],
    ['cacheRead', 'Cache read', form.cacheRead],
  ] as const
  const values: Record<string, number | null> = {}
  for (const [key, label, text] of fields) {
    const r = parseRate(text)
    if (!r.ok) return { ok: false, error: `${label} must be a dollar amount per 1M tokens, 0 or more.` }
    values[key] = r.value
  }
  if (values.input === null || values.output === null) {
    return { ok: false, error: 'Input and output rates are both required.' }
  }
  if (values.input === 0 && values.output === 0) {
    return { ok: false, error: 'A $0 rate is “Included in a plan” — mark the model included instead.' }
  }
  return {
    ok: true,
    body: {
      endpoint: target.endpoint,
      model: target.model,
      mode: 'manual',
      priceInPerMtok: values.input,
      priceOutPerMtok: values.output,
      priceCacheWritePerMtok: values.cacheWrite,
      priceCacheReadPerMtok: values.cacheRead,
      note: form.note.trim() || null,
    },
  }
}

/** The PUT body for "included in a plan / free" — $0, and no rates. */
export const includedBody = (target: PriceTarget, note: string): Record<string, unknown> => ({
  endpoint: target.endpoint,
  model: target.model,
  mode: 'included',
  note: note.trim() || null,
})

const rate = (n: number) => `$${Number(n.toFixed(4))}`

/** What a decision charges, in words: never a bare "$0". */
export function settingLabel(s: Pick<PriceSetting, 'mode' | 'in' | 'out' | 'cacheWrite' | 'cacheRead'>): string {
  if (s.mode === 'included') return 'Included in a plan · $0'
  const parts = [`${rate(s.in ?? 0)} in`, `${rate(s.out ?? 0)} out`]
  if (s.cacheWrite !== null) parts.push(`${rate(s.cacheWrite)} cache write`)
  if (s.cacheRead !== null) parts.push(`${rate(s.cacheRead)} cache read`)
  return `${parts.join(' · ')} per 1M`
}

/** What a save, a removal or a retry did to the window's past calls. */
export function repricedLine(r: Repriced): string {
  const calls = (n: number) => `${n} past call${n === 1 ? '' : 's'}`
  if (r.rows === 0) return 'No calls in the last 30 days to re-price.'
  if (r.unpricedRows === r.rows) return `${calls(r.rows)} still have no price.`
  const how = r.basis === 'included' ? 'included in a plan, $0' : `${formatUsd(r.cost, { group: true })}${r.basis ? ` (${r.basis})` : ''}`
  const rest = r.unpricedRows > 0 ? `; ${r.unpricedRows} still unpriced` : ''
  return `Re-priced ${calls(r.pricedRows)}: ${how}${rest}.`
}

export const setPrice = (body: Record<string, unknown>) =>
  putJson<{ price: unknown; repriced: Repriced }>('/api/cost/prices', body)

export const clearPrice = (target: PriceTarget) =>
  delJson<{ ok: true; repriced: Repriced }>('/api/cost/prices', target)

export const retryLookup = (target: PriceTarget) =>
  postJson<{ priced: boolean; reason: UnpricedReason | null; repriced: Repriced }>('/api/cost/unpriced/lookup', target)
