// THE NUMBER SPELLINGS THE APP SHARES, in one place.
//
// Every helper here but `formatTokens` had been written twice, in two files,
// with the same arithmetic and one deliberate disagreement. Each keeps that
// disagreement as an explicit flag rather than as a fork: the two answers are
// both right for their surface, and merging them would silently change a
// rendered string.
//
// PURE AND DEPENDENCY-FREE ON PURPOSE: no imports, no `$state`, no component.
// That is what lets a `.svelte.ts`, a plain node test and the Svelte-free
// vitest environment all load it (see the header of `components/models/fitness.ts`,
// which imports this module from a test that has no Svelte plugin).

/** Dollars, at two decimals.
 *
 *  Sub-cent amounts keep four decimals, because "$0.00" for a real spend is
 *  the kind of rounding that makes a reader distrust every other number on the
 *  page — this is the cost ledger's answer.
 *
 *  `unpricedAsWord` is the model/fitness surfaces' variant, and it says two
 *  extra things: a null price is the word "unpriced" (a caller holding a
 *  nullable figure never has to guard it), and a sub-cent spend floors at
 *  "<$0.01" instead of printing four decimals. That is right for a one-off
 *  run's compare-across-models number and wrong for a recurring bill, which is
 *  why `usdRate` in `components/models/fitness.ts` exists beside it rather than
 *  on top of it. */
export function formatUsd(n: number | null, opts?: { unpricedAsWord?: boolean }): string {
  if (n === null) return 'unpriced'
  if (n === 0) return '$0'
  if (n < 0.01) return opts?.unpricedAsWord ? '<$0.01' : `$${n.toFixed(4)}`
  return `$${n.toFixed(2)}`
}

/** A file size at the scale a human reads it.
 *
 *  The two callers belong to different worlds and disagree past the rounding:
 *
 *    attachments   exact bytes below a KB and the NEAREST KB above it ("512 B",
 *                  "2 KB"); no GB tier, so a 2 GiB upload reads "2048.0 MB".
 *    storage panel never below a KB (a 1-byte object is not "0 B") and rounded
 *                  UP, plus a GB tier, because a bucket is not an upload.
 *
 *  `ceilKb` picks the storage panel's answer. This is a flag and not one
 *  cascade because the two part company at more than the rounding: a
 *  non-finite size and anything past a GiB land on different tiers, so what the
 *  flag selects is the whole convention. */
export function formatBytes(n: number, opts?: { ceilKb?: boolean }): string {
  if (opts?.ceilKb) {
    if (n >= 1 << 30) return `${(n / (1 << 30)).toFixed(1)} GB`
    if (n >= 1 << 20) return `${(n / (1 << 20)).toFixed(1)} MB`
    return `${Math.ceil(n / 1024)} KB`
  }
  if (n < 1024) return `${n} B`
  if (n < 1024 * 1024) return `${Math.round(n / 1024)} KB`
  return `${(n / 1024 / 1024).toFixed(1)} MB`
}

/** A token count at the scale a ledger reports it: 1234 → "1.2k",
 *  5_600_000 → "5.6M". A decimal is dropped once the number is wide enough
 *  that it reads as noise rather than as precision. */
export function formatTokens(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(n >= 10_000_000 ? 0 : 1)}M`
  if (n >= 1_000) return `${(n / 1_000).toFixed(n >= 10_000 ? 0 : 1)}k`
  return String(n)
}

/** Accumulated agent time → compact "2h 15m" / "45m" / "30s" / "—".
 *
 *  Takes SECONDS. The name reads as milliseconds, but both call sites hand it a
 *  `…Seconds` field (`Task.timeSpentSeconds`, `taskTimeSpent()`), and a
 *  millisecond reading of the same numbers would print every ticket's time in
 *  hours. */
export function fmtDuration(seconds: number): string {
  if (!seconds) return '—'
  const h = Math.floor(seconds / 3600)
  const m = Math.floor((seconds % 3600) / 60)
  if (h) return m ? `${h}h ${m}m` : `${h}h`
  if (m) return `${m}m`
  return `${seconds}s`
}

/** The shared short-date spelling; the workchain rail and canvas render due
 *  dates through it. */
export function formatShortDate(iso: string): string {
  const d = new Date(iso)
  const sameYear = d.getFullYear() === new Date().getFullYear()
  return d.toLocaleDateString(undefined, { month: 'short', day: 'numeric', ...(sameYear ? {} : { year: 'numeric' }) })
}