// ── Line diff (LCS) — small, dependency-free, capped for huge documents ──────
// Shared by DiffView.svelte and InternalEditorModal.svelte.
export type DiffLine = { type: 'same' | 'add' | 'del'; text: string } | { type: 'skip'; count: number }

export function diffLines(oldText: string, newText: string): DiffLine[] | null {
  const a = oldText.split('\n')
  const b = newText.split('\n')
  const n = a.length
  const m = b.length
  if (n * m > 2_000_000) return null // too big to diff comfortably in the client
  const w = m + 1
  const dp = new Int32Array((n + 1) * w)
  for (let i = n - 1; i >= 0; i--)
    for (let j = m - 1; j >= 0; j--)
      dp[i * w + j] = a[i] === b[j] ? dp[(i + 1) * w + j + 1]! + 1 : Math.max(dp[(i + 1) * w + j]!, dp[i * w + j + 1]!)
  const raw: Array<Exclude<DiffLine, { type: 'skip' }>> = []
  let i = 0
  let j = 0
  while (i < n && j < m) {
    if (a[i] === b[j]) raw.push({ type: 'same', text: a[i++]! }), j++
    else if (dp[(i + 1) * w + j]! >= dp[i * w + j + 1]!) raw.push({ type: 'del', text: a[i++]! })
    else raw.push({ type: 'add', text: b[j++]! })
  }
  while (i < n) raw.push({ type: 'del', text: a[i++]! })
  while (j < m) raw.push({ type: 'add', text: b[j++]! })

  // Collapse long unchanged runs to 3 lines of context on each side.
  const out: DiffLine[] = []
  let run: Array<Exclude<DiffLine, { type: 'skip' }>> = []
  const flush = (last: boolean) => {
    const keep = 3
    if (run.length > keep * 2 + 1) {
      const head = out.length === 0 ? [] : run.slice(0, keep)
      const tail = last ? [] : run.slice(-keep)
      out.push(...head, { type: 'skip', count: run.length - head.length - tail.length }, ...tail)
    } else out.push(...run)
    run = []
  }
  for (const line of raw) {
    if (line.type === 'same') run.push(line)
    else {
      flush(false)
      out.push(line)
    }
  }
  flush(true)
  return out
}
