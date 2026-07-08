import { useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { Loader2 } from 'lucide-react'
import { Modal } from '@/components/ui/modal'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Steps } from '@/components/ui/steps'
import { reconcileFleet } from '@/lib/fleet-defs'
import { cn } from '@/lib/cn'

interface FederateResult {
  agents: Array<{ slug: string; model: string; status: 'federated' | 'exists' }>
  errors: string[]
}

const STEPS = ['Source', 'Federate', 'Start'] as const

// Bring outside agents in — as natives. Reads a Hermes-format directory and
// recreates each agent inside Talaria: our chassis, our orchestration, fresh
// key and state volume, skills carried over. The source dir is not referenced
// again afterwards.
export function FederateModal({ onClose }: { onClose: () => void }) {
  const qc = useQueryClient()
  const [step, setStep] = useState(0)
  const [dir, setDir] = useState('')
  const [busy, setBusy] = useState(false)
  const [result, setResult] = useState<FederateResult | null>(null)
  const [started, setStarted] = useState<string | null>(null)
  const [err, setErr] = useState<string | null>(null)

  const federate = async () => {
    setBusy(true)
    setErr(null)
    try {
      const r = await fetch('/api/fleet/federate', {
        method: 'POST',
        credentials: 'same-origin',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ dir: dir.trim() }),
      })
      const j = (await r.json().catch(() => null)) as { result?: FederateResult; error?: string } | null
      if (!j?.result) return setErr(j?.error ?? 'federation failed')
      setResult(j.result)
      await qc.invalidateQueries({ queryKey: ['fleet-defs'] })
      setStep(1)
    } finally {
      setBusy(false)
    }
  }

  const startAll = async () => {
    setBusy(true)
    try {
      const r = await reconcileFleet()
      setStarted(r.error ?? `Started ${r.started?.length ?? 0} · already running ${r.alreadyRunning?.length ?? 0}`)
      await qc.invalidateQueries({ queryKey: ['fleet-containers'] })
      setStep(2)
    } finally {
      setBusy(false)
    }
  }

  const fresh = result?.agents.filter((a) => a.status === 'federated') ?? []

  const footer =
    step === 0 ? (
      <div className="flex justify-end gap-2">
        <Button variant="ghost" size="sm" onClick={onClose}>
          Cancel
        </Button>
        <Button size="sm" onClick={() => void federate()} disabled={busy || !dir.trim()}>
          {busy && <Loader2 size={14} className="animate-spin" />}
          {busy ? 'Federating…' : 'Federate'}
        </Button>
      </div>
    ) : step === 1 ? (
      <div className="flex justify-end gap-2">
        <Button variant="ghost" size="sm" onClick={onClose}>
          Close
        </Button>
        <Button size="sm" onClick={() => void startAll()} disabled={busy || fresh.length === 0}>
          {busy ? 'Starting…' : `Start ${fresh.length} agent${fresh.length === 1 ? '' : 's'}`}
        </Button>
      </div>
    ) : (
      <div className="flex justify-end">
        <Button size="sm" onClick={onClose}>
          Done
        </Button>
      </div>
    )

  return (
    <Modal open onClose={onClose} title="Federate outside agents" width="max-w-lg" footer={footer}>
      <div className="space-y-5">
        <Steps steps={STEPS} current={step} />

        {step === 0 && (
          <div className="space-y-4">
            <p className="text-sm leading-relaxed text-muted">
              Point at a Hermes-format directory on the server (<code className="text-fg">agents.yaml</code> roster,
              each agent's <code className="text-fg">SOUL.md</code> + <code className="text-fg">config.yaml</code>).
              Each agent is recreated <em>natively</em>: Talaria's chassis and orchestration, a fresh key and state
              volume, models mapped into the registry, skills carried over. The source directory is never referenced
              again. Agents whose handle already exists are skipped.
            </p>
            <div>
              <label className="mb-1.5 block text-[11px] uppercase tracking-wide text-muted">Directory on server</label>
              <Input value={dir} onChange={(e) => setDir(e.target.value)} placeholder="/path/to/stack" autoFocus />
            </div>
            {err && <p className="text-xs text-[color:var(--theme-danger)]">{err}</p>}
          </div>
        )}

        {step === 1 && result && (
          <div className="space-y-3">
            <div className="text-sm text-fg">
              {fresh.length} federated · {result.agents.length - fresh.length} already here
            </div>
            {result.agents.length > 0 && (
              <ul className="max-h-56 divide-y divide-line-subtle overflow-y-auto rounded-lg border border-line-subtle">
                {result.agents.map((a) => (
                  <li key={a.slug} className="flex items-center gap-2 px-3.5 py-2 text-sm">
                    <span className="text-fg">{a.slug}</span>
                    <span className={cn('ml-auto text-xs', a.status === 'federated' ? 'text-accent' : 'text-muted')}>
                      {a.status === 'federated' ? 'federated' : 'already exists'}
                    </span>
                  </li>
                ))}
              </ul>
            )}
            {result.errors.map((e) => (
              <p key={e} className="text-xs text-[color:var(--theme-danger)]">
                {e}
              </p>
            ))}
          </div>
        )}

        {step === 2 && <p className="text-sm text-fg">{started}</p>}
      </div>
    </Modal>
  )
}
