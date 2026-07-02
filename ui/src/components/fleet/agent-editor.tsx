import { useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Modal } from '@/components/ui/modal'
import { Textarea } from '@/components/ui/textarea'
import { ModelPicker } from '@/components/fleet/model-picker'
import { saveAgentEdit, type AgentDef, type LlmEndpoint, type ModelTarget } from '@/lib/fleet-defs'

type AliasRow = ModelTarget & { name: string }

// Edit an agent's configurable surface — soul, main model, alias tiers,
// fallback chain. Saving creates a NEW version; "apply" re-renders and
// restarts the managed container so it takes effect immediately.
export function AgentEditorModal({
  open,
  onClose,
  def,
  endpoints,
}: {
  open: boolean
  onClose: () => void
  def: AgentDef
  endpoints: LlmEndpoint[]
}) {
  const qc = useQueryClient()
  const cfg = def.latest?.config
  const [soul, setSoul] = useState(def.latest?.soul ?? '')
  const [main, setMain] = useState<ModelTarget>(cfg?.main ?? { endpoint: endpoints[0]?.name ?? '', model: '' })
  const [aliases, setAliases] = useState<AliasRow[]>(cfg?.aliases ?? [])
  const [fallbacks, setFallbacks] = useState<ModelTarget[]>(cfg?.fallbacks ?? [])
  const [note, setNote] = useState('')
  const [busy, setBusy] = useState(false)
  const [err, setErr] = useState<string | null>(null)

  const save = async (apply: boolean) => {
    setErr(null)
    setBusy(true)
    try {
      const r = await saveAgentEdit(def.id, { soul, main, aliases, fallbacks, note: note || undefined, apply })
      if (r.error) return setErr(r.error)
      await qc.invalidateQueries({ queryKey: ['fleet-defs'] })
      onClose()
    } finally {
      setBusy(false)
    }
  }

  const epClass = (name: string) => endpoints.find((e) => e.name === name)?.class ?? 'cloud'

  const TargetRow = ({
    value,
    onChange,
    onRemove,
    namePlaceholder,
    name,
    onName,
  }: {
    value: ModelTarget
    onChange: (t: ModelTarget) => void
    onRemove?: () => void
    namePlaceholder?: string
    name?: string
    onName?: (n: string) => void
  }) => (
    <div className="flex items-center gap-2">
      {onName !== undefined && (
        <Input value={name ?? ''} onChange={(e) => onName(e.target.value)} placeholder={namePlaceholder} size="sm" className="w-28 shrink-0" />
      )}
      <ModelPicker endpoints={endpoints} value={value} onChange={onChange} size="sm" className="min-w-0 flex-1" />
      <span className="w-10 shrink-0 text-xs" style={{ color: epClass(value.endpoint) === 'local' ? 'var(--theme-success)' : 'var(--theme-accent)' }}>
        {epClass(value.endpoint)}
      </span>
      {onRemove && (
        <Button variant="ghost" size="sm" className="shrink-0" onClick={onRemove}>
          ✕
        </Button>
      )}
    </div>
  )

  return (
    <Modal open={open} onClose={onClose} title={`Edit ${def.displayName} (v${def.currentVersion} → v${def.currentVersion + 1})`} width="max-w-2xl">
      <div className="space-y-5">
        <section>
          <div className="mb-1.5 text-xs font-semibold uppercase tracking-wide text-muted">Main model</div>
          <TargetRow value={main} onChange={setMain} />
        </section>

        <section>
          <div className="mb-1.5 text-xs font-semibold uppercase tracking-wide text-muted">Model tiers (aliases)</div>
          <div className="space-y-1.5">
            {aliases.map((a, i) => (
              <TargetRow
                key={i}
                value={a}
                name={a.name}
                onName={(name) => setAliases(aliases.map((x, j) => (j === i ? { ...x, name } : x)))}
                namePlaceholder="alias"
                onChange={(t) => setAliases(aliases.map((x, j) => (j === i ? { ...x, ...t } : x)))}
                onRemove={() => setAliases(aliases.filter((_, j) => j !== i))}
              />
            ))}
            <Button variant="outline" size="sm" onClick={() => setAliases([...aliases, { name: '', endpoint: endpoints[0]?.name ?? '', model: '' }])}>
              + Add tier
            </Button>
          </div>
        </section>

        <section>
          <div className="mb-1.5 text-xs font-semibold uppercase tracking-wide text-muted">Fallbacks (when the model above is down)</div>
          <div className="space-y-1.5">
            {fallbacks.map((f, i) => (
              <TargetRow
                key={i}
                value={f}
                onChange={(t) => setFallbacks(fallbacks.map((x, j) => (j === i ? t : x)))}
                onRemove={() => setFallbacks(fallbacks.filter((_, j) => j !== i))}
              />
            ))}
            <Button variant="outline" size="sm" onClick={() => setFallbacks([...fallbacks, { endpoint: endpoints[0]?.name ?? '', model: '' }])}>
              + Add fallback
            </Button>
          </div>
        </section>

        <section>
          <div className="mb-1.5 text-xs font-semibold uppercase tracking-wide text-muted">Soul</div>
          <Textarea value={soul} onChange={(e) => setSoul(e.target.value)} rows={12} className="font-[var(--font-mono)] text-xs" />
        </section>

        {err && (
          <div className="text-sm" style={{ color: 'var(--theme-danger)' }}>
            {err}
          </div>
        )}

        <div className="flex items-center gap-2 border-t border-line-subtle pt-3">
          <Input value={note} onChange={(e) => setNote(e.target.value)} placeholder="version note (optional)" size="sm" className="min-w-0 flex-1" />
          <Button variant="outline" size="sm" onClick={() => void save(false)} disabled={busy}>
            Save version
          </Button>
          {def.managed && (
            <Button size="sm" onClick={() => void save(true)} disabled={busy}>
              {busy ? 'Applying…' : 'Save & apply'}
            </Button>
          )}
        </div>
      </div>
    </Modal>
  )
}
