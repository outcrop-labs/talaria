import { useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Modal } from '@/components/ui/modal'
import { ModelPicker } from '@/components/fleet/model-picker'
import { InternalEditorModal } from '@/components/fleet/internal-editor-modal'
import { saveAgentEdit, type AgentDef, type LlmEndpoint, type ModelTarget } from '@/lib/fleet-defs'

type AliasRow = ModelTarget & { name: string }

// Module-scope (NOT inside the modal component): an inline component would get
// a new identity every render, remounting the row and dropping input focus on
// each keystroke.
function TargetRow({
  endpoints,
  value,
  onChange,
  onRemove,
  namePlaceholder,
  name,
  onName,
}: {
  endpoints: LlmEndpoint[]
  value: ModelTarget
  onChange: (t: ModelTarget) => void
  onRemove?: () => void
  namePlaceholder?: string
  name?: string
  onName?: (n: string) => void
}) {
  const epClass = endpoints.find((e) => e.name === value.endpoint)?.class ?? 'cloud'
  return (
    <div className="flex items-center gap-2">
      {onName !== undefined && (
        <Input value={name ?? ''} onChange={(e) => onName(e.target.value)} placeholder={namePlaceholder} size="sm" className="w-28 shrink-0" />
      )}
      <ModelPicker endpoints={endpoints} value={value} onChange={onChange} size="sm" className="min-w-0 flex-1" />
      <span className="w-10 shrink-0 text-xs" style={{ color: epClass === 'local' ? 'var(--theme-success)' : 'var(--theme-accent)' }}>
        {epClass}
      </span>
      {onRemove && (
        <Button variant="ghost" size="sm" className="shrink-0" onClick={onRemove}>
          ✕
        </Button>
      )}
    </div>
  )
}

// The agent config surface — soul, main model, alias tiers, fallback chain —
// as an embeddable form. Saving creates a NEW version; "apply" re-renders and
// restarts the managed container. Used standalone (AgentEditorModal) and as the
// Config tab of the unified manage modal.
export function AgentConfigForm({ def, endpoints, onSaved }: { def: AgentDef; endpoints: LlmEndpoint[]; onSaved?: () => void }) {
  const qc = useQueryClient()
  const cfg = def.latest?.config
  const [soul, setSoul] = useState(def.latest?.soul ?? '')
  const [main, setMain] = useState<ModelTarget>(cfg?.main ?? { endpoint: endpoints[0]?.name ?? '', model: '' })
  const [aliases, setAliases] = useState<AliasRow[]>(cfg?.aliases ?? [])
  const [fallbacks, setFallbacks] = useState<ModelTarget[]>(cfg?.fallbacks ?? [])
  const [note, setNote] = useState('')
  const [busy, setBusy] = useState(false)
  const [soulOpen, setSoulOpen] = useState(false)
  const [err, setErr] = useState<string | null>(null)

  const save = async (apply: boolean) => {
    setErr(null)
    setBusy(true)
    try {
      const r = await saveAgentEdit(def.id, { soul, main, aliases, fallbacks, note: note || undefined, apply })
      if (r.error) return setErr(r.error)
      await qc.invalidateQueries({ queryKey: ['fleet-defs'] })
      onSaved?.()
    } finally {
      setBusy(false)
    }
  }

  return (
      <div className="space-y-5">
        <section>
          <div className="mb-1.5 text-xs font-semibold uppercase tracking-wide text-muted">Main model</div>
          <TargetRow endpoints={endpoints} value={main} onChange={setMain} />
        </section>

        <section>
          <div className="mb-1.5 text-xs font-semibold uppercase tracking-wide text-muted">Model tiers (aliases)</div>
          <div className="space-y-1.5">
            {aliases.map((a, i) => (
              <TargetRow
                endpoints={endpoints}
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
                endpoints={endpoints}
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
          <div className="mb-1.5 flex items-center text-xs font-semibold uppercase tracking-wide text-muted">
            Soul
            <Button variant="outline" size="sm" className="ml-auto" onClick={() => setSoulOpen(true)}>
              Open workspace
            </Button>
          </div>
          <pre className="max-h-40 overflow-y-auto whitespace-pre-wrap rounded-lg border border-line-subtle p-3 font-[var(--font-mono)] text-xs text-muted">
            {soul || 'No soul yet.'}
          </pre>
          {soulOpen && (
            <InternalEditorModal
              open
              onClose={() => setSoulOpen(false)}
              title={`${def.displayName} · SOUL.md`}
              subtitle="Who the agent is. Saving publishes a soul-only version on top of the last saved config."
              value={soul}
              editable
              saving={busy}
              onSave={async (md) => {
                // Soul-only publish: model targets come from the LAST SAVED
                // version, not the form — pending model edits stay drafts.
                const base = def.latest?.config
                const r = await saveAgentEdit(def.id, {
                  soul: md,
                  main: base?.main ?? main,
                  aliases: base?.aliases ?? aliases,
                  fallbacks: base?.fallbacks ?? fallbacks,
                  note: 'soul edited',
                  apply: true,
                })
                if (r.error) throw new Error(r.error)
                setSoul(md)
                await qc.invalidateQueries({ queryKey: ['fleet-defs'] })
              }}
              history={{ kind: 'soul', id: def.id }}
              muse={{ kind: 'soul', context: `${def.displayName} — ${def.role ?? def.department} agent (slug "${def.slug}").` }}
            />
          )}
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
  )
}

// Standalone config editor (kept for direct use / back-compat).
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
  return (
    <Modal open={open} onClose={onClose} title={`Edit ${def.displayName} (v${def.currentVersion} → v${def.currentVersion + 1})`} width="max-w-2xl">
      <AgentConfigForm def={def} endpoints={endpoints} onSaved={onClose} />
    </Modal>
  )
}
