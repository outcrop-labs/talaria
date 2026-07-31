// The confab guard's caveat on a flagged agent reply (annotate/strict modes).
// Renders from message metadata — the findings are never part of the reply
// text, so they can't leak back into any transcript.

export interface GuardFinding {
  check: string
  severity: 'low' | 'medium' | 'high'
  confidence: number
  message: string
  snippet: string
}

export function GuardCaveat({ findings }: { findings?: GuardFinding[] | null }) {
  if (!findings?.length) return null
  return (
    // Attention panel (spec §8): gold hairline + mono uppercase label; body
    // stays readable sans on the readout tone.
    <div className="mt-2 space-y-1 rounded-md border px-2.5 py-2 text-xs" style={{ borderColor: 'var(--theme-warning)' }}>
      <div className="font-mono text-[10px] uppercase tracking-[0.08em]" style={{ color: 'var(--theme-warning)' }}>
        Unverified · confab guard flagged this reply
      </div>
      {findings.map((f, i) => (
        <div key={`${f.check}-${i}`} className="font-sans text-fg">
          <span className="font-mono text-[10px] uppercase tracking-[0.05em]" style={{ color: 'var(--theme-warning)' }}>
            {f.check.replace(/_/g, ' ')}
          </span>
          {' · '}
          {f.message}
          {f.snippet ? <span className="text-muted"> ({f.snippet})</span> : null}
        </div>
      ))}
    </div>
  )
}
