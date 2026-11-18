/** The dash every formatter shows for a value it cannot read. */
const UNKNOWN = '—'

/** Is `n` a count or a span a person can be shown? */
function readable(n: number): boolean {
  return Number.isFinite(n) && n >= 0
}

/** A byte count for people: 0 B, 1.5 KB, 2.0 MB. */
export function formatBytes(n: number): string {
  if (!readable(n)) return UNKNOWN
  if (n < 1024) return `${n} B`
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`
  return `${(n / (1024 * 1024)).toFixed(1)} MB`
}

/** A duration for people: 45 s, 3 min, 2 h. */
export function formatDuration(ms: number): string {
  if (!readable(ms)) return UNKNOWN
  if (ms < 60_000) return `${Math.round(ms / 1000)} s`
  if (ms < 3_600_000) return `${Math.round(ms / 60_000)} min`
  return `${Math.round(ms / 3_600_000)} h`
}
