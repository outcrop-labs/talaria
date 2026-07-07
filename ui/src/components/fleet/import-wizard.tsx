import { useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { Loader2 } from 'lucide-react'
import { Modal } from '@/components/ui/modal'
import { Button } from '@/components/ui/button'
import { Steps } from '@/components/ui/steps'
import { cn } from '@/lib/cn'
import { importFleet, reconcileFleet, type ReconcileResult } from '@/lib/fleet-defs'

// Step-by-step import: Scan (read the stack from disk) → Review (what was
// found) → Start (optionally bring the fleet up). Each step is one existing
// endpoint; the wizard just makes the sequence legible.
type ScanResult = NonNullable<Awaited<ReturnType<typeof importFleet>>>

const STEPS = ['Scan', 'Review', 'Start'] as const

export function ImportWizard({ onClose }: { onClose: () => void }) {
  const qc = useQueryClient()
  const [step, setStep] = useState(0)
  const [busy, setBusy] = useState(false)
  const [scan, setScan] = useState<ScanResult | null>(null)
  const [startResult, setStartResult] = useState<ReconcileResult | null>(null)

  const runScan = async () => {
    setBusy(true)
    try {
      const r = await importFleet()
      setScan(r ?? { agents: [], errors: ['Scan failed — check the server logs.'] })
      await qc.invalidateQueries({ queryKey: ['fleet-defs'] })
      setStep(1)
    } finally {
      setBusy(false)
    }
  }

  const runStart = async () => {
    setBusy(true)
    try {
      setStartResult(await reconcileFleet())
      await qc.invalidateQueries({ queryKey: ['fleet-containers'] })
      await qc.invalidateQueries({ queryKey: ['fleet'] })
    } finally {
      setBusy(false)
    }
  }

  const changed = scan?.agents.filter((a) => a.created).length ?? 0

  const footer =
    step === 0 ? (
      <div className="flex justify-end gap-2">
        <Button variant="ghost" size="sm" onClick={onClose}>
          Cancel
        </Button>
        <Button size="sm" onClick={() => void runScan()} disabled={busy}>
          {busy && <Loader2 size={14} className="animate-spin" />}
          {busy ? 'Scanning…' : 'Scan for agents'}
        </Button>
      </div>
    ) : step === 1 ? (
      <div className="flex justify-end gap-2">
        <Button variant="ghost" size="sm" onClick={onClose}>
          Close
        </Button>
        <Button size="sm" onClick={() => setStep(2)} disabled={!scan?.agents.length}>
          Continue
        </Button>
      </div>
    ) : startResult ? (
      <div className="flex justify-end">
        <Button size="sm" onClick={onClose}>
          Done
        </Button>
      </div>
    ) : (
      <div className="flex justify-end gap-2">
        <Button variant="ghost" size="sm" onClick={onClose}>
          Skip for now
        </Button>
        <Button size="sm" onClick={() => void runStart()} disabled={busy}>
          {busy && <Loader2 size={14} className="animate-spin" />}
          {busy ? 'Starting…' : 'Start agents'}
        </Button>
      </div>
    )

  return (
    <Modal open onClose={onClose} title="Import agents" width="max-w-lg" footer={footer}>
      <div className="space-y-5">
        <Steps steps={STEPS} current={step} />

        {step === 0 && (
          <div className="space-y-3 text-sm text-muted">
            <p>
              Talaria scans your stack for agents — the roster in <code className="text-fg">agents.yaml</code> plus each
              agent's <code className="text-fg">SOUL.md</code> and <code className="text-fg">config.yaml</code> — and
              adds them to the catalog.
            </p>
            <p>
              Nothing is started or changed on disk. Scanning is safe to repeat: an agent only gets a new version when
              one of its files changed.
            </p>
          </div>
        )}

        {step === 1 && scan && (
          <div className="space-y-3">
            <div className="text-sm text-fg">
              {scan.agents.length === 0
                ? 'No agents found.'
                : `${scan.agents.length} agent${scan.agents.length === 1 ? '' : 's'} found · ${changed} new or updated`}
            </div>
            {scan.agents.length > 0 && (
              <ul className="max-h-56 divide-y divide-line-subtle overflow-y-auto rounded-lg border border-line-subtle">
                {scan.agents.map((a) => (
                  <li key={a.slug} className="flex items-center gap-2 px-3 py-2 text-sm">
                    <span className="text-fg">{a.slug}</span>
                    <span className="text-xs text-muted">v{a.version}</span>
                    <span className={cn('ml-auto text-xs', a.created ? 'text-accent' : 'text-muted')}>
                      {a.created ? 'updated' : 'unchanged'}
                    </span>
                  </li>
                ))}
              </ul>
            )}
            {scan.errors.length > 0 && (
              <ul className="space-y-1 text-xs text-[color:var(--theme-danger)]">
                {scan.errors.map((e) => (
                  <li key={e}>{e}</li>
                ))}
              </ul>
            )}
          </div>
        )}

        {step === 2 &&
          (startResult ? (
            <div className="space-y-2 text-sm">
              {startResult.error ? (
                <p className="text-[color:var(--theme-danger)]">{startResult.error}</p>
              ) : (
                <p className="text-fg">
                  Started {startResult.started?.length ?? 0} · already running {startResult.alreadyRunning?.length ?? 0}
                </p>
              )}
              {(startResult.warnings ?? []).map((w) => (
                <p key={w} className="text-xs text-[color:var(--theme-warning)]">
                  {w}
                </p>
              ))}
            </div>
          ) : (
            <p className="text-sm text-muted">
              Start every enabled agent that isn't already running? You can skip this and start agents one by one from
              the roster instead.
            </p>
          ))}
      </div>
    </Modal>
  )
}
