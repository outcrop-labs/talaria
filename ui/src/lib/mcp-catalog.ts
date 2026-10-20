// What the marketplace says about catalog LIVENESS — the liveness job's
// verdict per server and its counts across the mirror. Pure, in lib/ so
// vitest pins the sentences; the picker and the featured editor import them
// through routes/app/mcp.ts.

/** One server's last verdict. `null` on the row = never checked. */
export interface LibraryLive {
  state: 'ok' | 'auth' | 'dead' | 'unknown'
  /** Epoch ms; null when the entry changed since (a re-check is due). */
  checkedAt: number | null
  detail: string | null
}

/** The mirror-wide counts `catalog.liveness` carries. */
export interface CatalogLiveness {
  checked: number
  dead: number
  unknown: number
  pending: number
}

/** The tail of the catalog status line: how much of the catalog has been
 *  asked and how much of it did not answer. Empty until the first verdict. */
export function livenessLine(l: CatalogLiveness | undefined | null): string {
  if (!l || l.checked === 0) return l && l.pending > 0 ? 'liveness not checked yet' : ''
  const parts = [`${l.checked.toLocaleString()} checked`]
  if (l.dead > 0) parts.push(`${l.dead.toLocaleString()} unreachable`)
  if (l.pending > 0) parts.push(`${l.pending.toLocaleString()} to go`)
  return parts.join(' · ')
}

/** The "N unreachable hidden · show" affordance's words; null when nothing
 *  was hidden. */
export function hiddenDeadLine(n: number | undefined): string | null {
  if (!n || n <= 0) return null
  return `${n.toLocaleString()} unreachable ${n === 1 ? 'server' : 'servers'} hidden`
}

/** The empty state's button when every match was hidden as dead: the
 *  count, not the status line's full sentence. */
export function showDeadLabel(n: number | undefined): string {
  return n && n > 0 ? `Show ${n.toLocaleString()} unreachable` : 'Show unreachable'
}

/** The muted line under a dead card: when the job last failed to reach it,
 *  and how. Null for anything but a dead verdict. */
export function deadLine(live: LibraryLive | null | undefined, relative: (iso: string) => string): string | null {
  if (!live || live.state !== 'dead') return null
  const when = live.checkedAt ? ` ${relative(new Date(live.checkedAt).toISOString())}` : ''
  return `Unreachable when last checked${when}`
}

// ── The operator's featured list ────────────────────────────────────────────

export interface StandInRow {
  domain: string
  title: string
  url: string
  description?: string | null
}

export interface FeaturedConfigRow {
  domains: string[]
  hidden: string[]
  standIns: StandInRow[]
}

/** GET /api/mcp/library/featured: what is stored plus what the editor needs
 *  to draw it. */
export interface FeaturedResponse extends FeaturedConfigRow {
  builtins: string[]
  /** The merged shelf order the marketplace shows. */
  shelf: string[]
}

/** A bare host, the server's rule: two-plus labels of [a-z0-9-], no scheme,
 *  port or path, not an IP. Lets the editor refuse before the round trip. */
export function isBareHost(raw: string): boolean {
  const h = raw.trim().replace(/\.$/, '').toLowerCase()
  const labels = h.split('.')
  if (h.length > 253 || labels.length < 2) return false
  if (/^\d+$/.test(labels[labels.length - 1] ?? '')) return false
  return labels.every((l) => l.length > 0 && l.length <= 63 && /^[a-z0-9-]+$/.test(l) && !l.startsWith('-') && !l.endsWith('-'))
}

/** The chips the editor draws: every built-in and every added domain, in
 *  shelf order, with hidden built-ins still listed (struck through, restorable). */
export interface FeaturedChip {
  domain: string
  builtin: boolean
  hidden: boolean
}

export function featuredChips(builtins: string[], cfg: FeaturedConfigRow): FeaturedChip[] {
  const out: FeaturedChip[] = []
  const seen = new Set<string>()
  for (const d of [...builtins, ...cfg.domains]) {
    if (seen.has(d)) continue
    seen.add(d)
    out.push({ domain: d, builtin: builtins.includes(d), hidden: cfg.hidden.includes(d) })
  }
  return out
}

/** Removing a chip: a built-in is HIDDEN (it cannot be deleted from the
 *  build), an added domain is dropped. Restoring un-hides. Pure. */
export function toggleChip(builtins: string[], cfg: FeaturedConfigRow, domain: string): FeaturedConfigRow {
  if (cfg.hidden.includes(domain)) return { ...cfg, hidden: cfg.hidden.filter((d) => d !== domain) }
  if (builtins.includes(domain)) return { ...cfg, hidden: [...cfg.hidden, domain] }
  return { ...cfg, domains: cfg.domains.filter((d) => d !== domain) }
}

export function addDomain(cfg: FeaturedConfigRow, raw: string): FeaturedConfigRow {
  const d = raw.trim().replace(/\.$/, '').toLowerCase()
  return {
    ...cfg,
    domains: cfg.domains.includes(d) ? cfg.domains : [...cfg.domains, d],
    hidden: cfg.hidden.filter((h) => h !== d),
  }
}
