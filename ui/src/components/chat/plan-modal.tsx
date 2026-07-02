import { useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Modal } from '@/components/ui/modal'
import { Select } from '@/components/ui/select'
import { Textarea } from '@/components/ui/textarea'
import { createTask, useBoards } from '@/lib/boards'
import type { AgentModel } from '@/lib/agents'
import type { Effort, Priority } from '@/lib/task-const'

interface Proposal {
  title: string
  description: string
  priority: Priority
  effort: Effort | null
  include: boolean
}

// Plan chat: a channel agent drafts tickets from the conversation; the human
// reviews/edits here and creates the keepers — into inbox, never assigned.
export function PlanModal({
  open,
  onClose,
  channelId,
  channelAgents,
  fleet,
}: {
  open: boolean
  onClose: () => void
  channelId: string
  channelAgents: string[]
  fleet: AgentModel[]
}) {
  const qc = useQueryClient()
  const { data: boards = [] } = useBoards()
  const agents = fleet.filter((a) => channelAgents.includes(a.id))
  const [agentModel, setAgentModel] = useState(agents[0]?.id ?? '')
  const [tier, setTier] = useState('')
  const [boardId, setBoardId] = useState('')
  const [proposals, setProposals] = useState<Proposal[] | null>(null)
  const [phase, setPhase] = useState<'idle' | 'drafting' | 'creating' | 'done'>('idle')
  const [note, setNote] = useState<string | null>(null)

  const picked = agents.find((a) => a.id === (agentModel || agents[0]?.id))
  const tiers = picked?.tiers ?? []
  const editable = boards.filter((b) => b.role === 'owner' || b.role === 'editor')
  const included = proposals?.filter((p) => p.include) ?? []

  const draft = async () => {
    setPhase('drafting')
    setNote(null)
    setProposals(null)
    try {
      const r = await fetch(`/api/channels/${channelId}/plan`, {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ agentModel: picked?.id, tier: tier || null }),
      })
      const j = (await r.json()) as { proposals?: Omit<Proposal, 'include'>[]; note?: string; error?: string }
      if (!r.ok || j.error) setNote(j.error ?? 'planning failed')
      else if (!j.proposals?.length) setNote(j.note ?? 'no tickets came back')
      else setProposals(j.proposals.map((p) => ({ ...p, include: true })))
    } catch {
      setNote('planning failed — is the gateway up?')
    } finally {
      setPhase('idle')
    }
  }

  const [createdCount, setCreatedCount] = useState(0)

  // Idempotent: each success unticks its proposal, so a retry after a mid-loop
  // failure only creates what's still pending — never duplicates.
  const createAll = async () => {
    if (!boardId || included.length === 0) return
    setPhase('creating')
    setNote(null)
    let failed: string | null = null
    for (const p of included) {
      try {
        await createTask(boardId, {
          title: p.title,
          description: p.description || undefined,
          priority: p.priority,
          effort: p.effort,
        })
        setCreatedCount((n) => n + 1)
        setProposals((prev) => prev?.map((x) => (x === p ? { ...x, include: false } : x)) ?? null)
      } catch {
        failed = p.title
        break
      }
    }
    await qc.invalidateQueries({ queryKey: ['tasks', boardId] })
    if (failed) {
      setNote(`"${failed}" failed to create — the ones before it are done; retry creates only what's left`)
      setPhase('idle')
    } else {
      setPhase('done')
    }
  }

  const patch = (i: number, p: Partial<Proposal>) =>
    setProposals((prev) => prev?.map((x, j) => (j === i ? { ...x, ...p } : x)) ?? null)

  return (
    <Modal open={open} onClose={onClose} title="Plan from this conversation">
      <div className="space-y-4">
        {phase === 'done' ? (
          <>
            <p className="text-sm text-fg">
              Created {createdCount} ticket{createdCount === 1 ? '' : 's'} in{' '}
              <span className="font-medium">{editable.find((b) => b.id === boardId)?.name}</span> — they're in the
              inbox, ready to assign.
            </p>
            <div className="flex justify-end border-t border-line-subtle pt-3">
              <Button size="sm" onClick={onClose}>
                Done
              </Button>
            </div>
          </>
        ) : proposals === null ? (
          <>
            <p className="text-sm text-muted">
              A channel agent reads the conversation and drafts tickets. You review before anything is created.
            </p>
            <div className="grid grid-cols-2 gap-3">
              <div>
                <label className="mb-1 block text-[11px] uppercase tracking-wide text-muted">Planner</label>
                <Select value={picked?.id ?? ''} size="sm" onChange={(e) => setAgentModel(e.target.value)} className="w-full">
                  {agents.map((a) => (
                    <option key={a.id} value={a.id}>
                      {a.label}
                    </option>
                  ))}
                </Select>
              </div>
              <div>
                <label className="mb-1 block text-[11px] uppercase tracking-wide text-muted">Model tier</label>
                <Select value={tier} size="sm" onChange={(e) => setTier(e.target.value)} className="w-full">
                  <option value="">main</option>
                  {tiers.map((t) => (
                    <option key={t} value={t}>
                      {t}
                    </option>
                  ))}
                </Select>
              </div>
            </div>
            {note && (
              <div className="text-sm" style={{ color: 'var(--theme-danger)' }}>
                {note}
              </div>
            )}
            <div className="flex justify-end gap-2 border-t border-line-subtle pt-3">
              <Button variant="ghost" size="sm" onClick={onClose}>
                Cancel
              </Button>
              <Button size="sm" onClick={() => void draft()} disabled={phase === 'drafting' || !picked}>
                {phase === 'drafting' ? 'Drafting…' : 'Draft tickets'}
              </Button>
            </div>
          </>
        ) : (
          <>
            <div className="max-h-[50vh] space-y-3 overflow-y-auto pr-1">
              {proposals.map((p, i) => (
                <div key={i} className="rounded-xl border border-line-subtle p-3">
                  <div className="flex items-center gap-2">
                    <input
                      type="checkbox"
                      checked={p.include}
                      onChange={(e) => patch(i, { include: e.target.checked })}
                      className="shrink-0 accent-[var(--theme-accent)]"
                    />
                    <Input size="sm" value={p.title} onChange={(e) => patch(i, { title: e.target.value })} className="flex-1" />
                    <Select value={p.priority} size="sm" onChange={(e) => patch(i, { priority: e.target.value as Priority })} className="w-24 shrink-0">
                      {(['low', 'medium', 'high', 'urgent'] as const).map((x) => (
                        <option key={x} value={x}>
                          {x}
                        </option>
                      ))}
                    </Select>
                    <Select
                      value={p.effort ?? ''}
                      size="sm"
                      onChange={(e) => patch(i, { effort: (e.target.value || null) as Effort | null })}
                      className="w-20 shrink-0"
                    >
                      <option value="">—</option>
                      {(['xs', 's', 'm', 'l', 'xl'] as const).map((x) => (
                        <option key={x} value={x}>
                          {x.toUpperCase()}
                        </option>
                      ))}
                    </Select>
                  </div>
                  {p.include && (
                    <Textarea
                      value={p.description}
                      onChange={(e) => patch(i, { description: e.target.value })}
                      rows={3}
                      className="mt-2 text-xs"
                    />
                  )}
                </div>
              ))}
            </div>
            <div className="flex items-center gap-2 border-t border-line-subtle pt-3">
              <Select value={boardId} size="sm" onChange={(e) => setBoardId(e.target.value)} className="w-56">
                <option value="">Pick a board…</option>
                {editable.map((b) => (
                  <option key={b.id} value={b.id}>
                    {b.name}
                  </option>
                ))}
              </Select>
              {note && (
                <span className="text-xs" style={{ color: 'var(--theme-danger)' }}>
                  {note}
                </span>
              )}
              <span className="ml-auto" />
              <Button variant="ghost" size="sm" onClick={() => setProposals(null)}>
                Back
              </Button>
              <Button size="sm" onClick={() => void createAll()} disabled={phase === 'creating' || !boardId || included.length === 0}>
                {phase === 'creating' ? 'Creating…' : `Create ${included.length} ticket${included.length === 1 ? '' : 's'}`}
              </Button>
            </div>
          </>
        )}
      </div>
    </Modal>
  )
}
