import { useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Modal } from '@/components/ui/modal'
import { Select } from '@/components/ui/select'
import { createFleetAgent, type AgentDef } from '@/lib/fleet-defs'

// Spin up a brand-new agent from a template: pick an existing agent as the
// chassis (model tiers, tools, plugins carry over, identity re-stamped), give
// it a name — Talaria allocates the key, writes v1, renders, and starts it.
export function CreateAgentModal({
  open,
  onClose,
  templates,
}: {
  open: boolean
  onClose: () => void
  templates: AgentDef[]
}) {
  const qc = useQueryClient()
  const [displayName, setDisplayName] = useState('')
  const [slug, setSlug] = useState('')
  const [department, setDepartment] = useState('')
  const [templateId, setTemplateId] = useState(templates[0]?.id ?? '')
  const [start, setStart] = useState(true)
  const [busy, setBusy] = useState(false)
  const [err, setErr] = useState<string | null>(null)

  const onName = (v: string) => {
    setDisplayName(v)
    if (!slug || slug === displayName.toLowerCase().replace(/[^a-z0-9]/g, '')) {
      setSlug(v.toLowerCase().replace(/[^a-z0-9]/g, ''))
    }
  }

  const create = async () => {
    setErr(null)
    setBusy(true)
    try {
      const r = await createFleetAgent({ slug, department, displayName, templateId, start })
      if (r.error) return setErr(r.error)
      if (start && r.healthy === false) setErr('created, but the container is not healthy yet — check /agents')
      await qc.invalidateQueries({ queryKey: ['fleet-defs'] })
      await qc.invalidateQueries({ queryKey: ['fleet-containers'] })
      if (!r.error) onClose()
    } finally {
      setBusy(false)
    }
  }

  return (
    <Modal open={open} onClose={onClose} title="New agent">
      <div className="space-y-4">
        <div className="grid grid-cols-2 gap-3">
          <div>
            <label className="mb-1 block text-[11px] uppercase tracking-wide text-muted">Name</label>
            <Input value={displayName} onChange={(e) => onName(e.target.value)} placeholder="Remy" autoFocus />
          </div>
          <div>
            <label className="mb-1 block text-[11px] uppercase tracking-wide text-muted">Slug</label>
            <Input value={slug} onChange={(e) => setSlug(e.target.value)} placeholder="remy" />
          </div>
        </div>
        <div>
          <label className="mb-1 block text-[11px] uppercase tracking-wide text-muted">Department / role</label>
          <Input value={department} onChange={(e) => setDepartment(e.target.value)} placeholder="research" />
          <p className="mt-1 text-xs text-muted">Fleet model id becomes {slug || 'slug'}-{department || 'department'}.</p>
        </div>
        <div>
          <label className="mb-1 block text-[11px] uppercase tracking-wide text-muted">Template</label>
          <Select value={templateId} onChange={(e) => setTemplateId(e.target.value)} className="w-full">
            {templates.map((t) => (
              <option key={t.id} value={t.id}>
                {t.displayName} — {t.department} (v{t.currentVersion})
              </option>
            ))}
          </Select>
          <p className="mt-1 text-xs text-muted">
            Model tiers, tools, and plugins carry over with identity re-stamped; the soul starts from a scaffold you edit.
          </p>
        </div>
        <label className="flex cursor-pointer items-center gap-2 text-sm text-fg">
          <input type="checkbox" checked={start} onChange={(e) => setStart(e.target.checked)} className="accent-[color:var(--theme-accent)]" />
          Start the container now
        </label>
        {err && (
          <div className="text-sm" style={{ color: 'var(--theme-danger)' }}>
            {err}
          </div>
        )}
        <div className="flex justify-end gap-2 border-t border-line-subtle pt-3">
          <Button variant="ghost" size="sm" onClick={onClose}>
            Cancel
          </Button>
          <Button size="sm" onClick={() => void create()} disabled={busy || !slug || !department || !displayName || !templateId}>
            {busy ? 'Creating…' : 'Create agent'}
          </Button>
        </div>
      </div>
    </Modal>
  )
}
