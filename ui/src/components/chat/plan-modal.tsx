import { useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Modal } from '@/components/ui/modal'
import { Select } from '@/components/ui/select'
import { RichEditor } from '@/components/ui/rich-editor'
import { Generating } from '@/components/ui/generating'
import { Skeleton } from '@/components/ui/skeleton'
import { addDependency, createTask, useBoards } from '@/lib/boards'
import { useTemplates } from '@/lib/templates'
import type { AgentModel } from '@/lib/agents'
import type { Effort, Priority } from '@/lib/task-const'

interface Proposal {
  title: string
  description: string
  priority: Priority
  effort: Effort | null
  /** Indices of proposals in this batch that must land first. */
  dependsOn: number[]
  /** Routing labels from the workflow map — trip dispatch classification. */
  tags: string[]
  include: boolean
}

// Plan chat: an agent drafts tickets from a conversation (a channel or a plan);
// the human reviews/edits here and creates the keepers — into inbox, never
// assigned. Board-first: picking the board up front lets its default ticket
// template shape the drafts (resolution: explicit pick → agent → board default).
export function PlanModal({
  open,
  onClose,
  draftUrl,
  agents,
}: {
  open: boolean
  onClose: () => void
  draftUrl: string
  agents: AgentModel[]
}) {
  const qc = useQueryClient()
  const { data: boards = [], isLoading: boardsLoading } = useBoards()
  const { data: templates = [], isLoading: templatesLoading } = useTemplates()
  const [agentModel, setAgentModel] = useState(agents[0]?.id ?? '')
  const [tier, setTier] = useState('')
  const [boardId, setBoardId] = useState('')
  const [templateId, setTemplateId] = useState('') // '' = automatic (agent → board default)
  const [proposals, setProposals] = useState<Proposal[] | null>(null)
  const [phase, setPhase] = useState<'idle' | 'drafting' | 'creating' | 'done'>('idle')
  const [note, setNote] = useState<string | null>(null)

  const picked = agents.find((a) => a.id === (agentModel || agents[0]?.id))
  const tiers = picked?.tiers ?? []
  const editable = boards.filter((b) => b.role === 'owner' || b.role === 'editor')
  const ticketTemplates = templates.filter((t) => t.kind === 'ticket')
  const included = proposals?.filter((p) => p.include) ?? []

  const draft = async () => {
    setPhase('drafting')
    setNote(null)
    setProposals(null)
    try {
      const r = await fetch(draftUrl, {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({
          agentModel: picked?.id,
          tier: tier || null,
          boardId: boardId || null,
          templateId: templateId || null,
        }),
      })
      const j = (await r.json()) as { proposals?: Omit<Proposal, 'include'>[]; note?: string; error?: string }
      if (!r.ok || j.error) setNote(j.error ?? 'planning failed')
      else if (!j.proposals?.length) setNote(j.note ?? 'no tickets came back')
      else setProposals(j.proposals.map((p) => ({ ...p, dependsOn: p.dependsOn ?? [], tags: p.tags ?? [], include: true })))
    } catch {
      setNote('planning failed. Is the gateway up?')
    } finally {
      setPhase('idle')
    }
  }

  const [createdCount, setCreatedCount] = useState(0)

  // Two passes: create every included ticket (collecting ids by proposal index),
  // then wire dependencies between the ones that were created. Each success
  // unticks its proposal, so a retry after a mid-loop failure only creates
  // what's still pending — never duplicates.
  const createAll = async () => {
    if (!boardId || included.length === 0 || !proposals) return
    setPhase('creating')
    setNote(null)
    const createdIds = new Map<number, string>() // proposal index → task id
    let failed: string | null = null
    for (const [i, p] of proposals.entries()) {
      if (!p.include) continue
      try {
        const res = (await createTask(boardId, {
          title: p.title,
          description: p.description || undefined,
          priority: p.priority,
          effort: p.effort,
          tags: p.tags.length ? p.tags : undefined,
        })) as { task?: { id: string } }
        if (res.task?.id) createdIds.set(i, res.task.id)
        setCreatedCount((n) => n + 1)
        setProposals((prev) => prev?.map((x, j) => (j === i ? { ...x, include: false } : x)) ?? null)
      } catch {
        failed = p.title
        break
      }
    }
    // Dependencies between just-created tickets (skipped/failed ones drop out).
    for (const [i, taskId] of createdIds) {
      for (const dep of proposals[i]?.dependsOn ?? []) {
        const dependsOnId = createdIds.get(dep)
        if (dependsOnId) await addDependency(taskId, dependsOnId).catch(() => {})
      }
    }
    await qc.invalidateQueries({ queryKey: ['tasks', boardId] })
    if (failed) {
      setNote(`"${failed}" failed to create. The ones before it are done; retry creates only what's left`)
      setPhase('idle')
    } else {
      setPhase('done')
    }
  }

  const patch = (i: number, p: Partial<Proposal>) =>
    setProposals((prev) => prev?.map((x, j) => (j === i ? { ...x, ...p } : x)) ?? null)

  return (
    <Modal open={open} onClose={onClose} title="Plan from this conversation" takeover={!!proposals} width="max-w-lg">
      <div className="space-y-4">
        {phase === 'done' ? (
          <>
            <p className="text-sm text-fg">
              Created {createdCount} ticket{createdCount === 1 ? '' : 's'} in{' '}
              <span className="font-medium">{editable.find((b) => b.id === boardId)?.name}</span>. They're in the
              inbox, ready to assign.
            </p>
            <div className="flex justify-end border-t border-line-subtle pt-3">
              <Button size="sm" onClick={onClose}>
                Done
              </Button>
            </div>
          </>
        ) : phase === 'drafting' ? (
          <>
            {/* The result replaces these: skeleton proposal cards, sized like the real ones. */}
            <div className="flex items-center gap-2 text-sm text-muted">
              <span className="font-medium text-fg">{picked?.label ?? 'The agent'}</span> is reading the conversation and
              drafting tickets{templateId || boardId ? ' on your template' : ''}
            </div>
            <div className="space-y-3">
              <Generating lines={2} />
              <Generating lines={3} />
              <Generating lines={2} />
            </div>
            <div className="flex justify-end border-t border-line-subtle pt-3">
              <Button variant="ghost" size="sm" onClick={onClose}>
                Cancel
              </Button>
            </div>
          </>
        ) : proposals === null ? (
          <>
            <p className="text-sm text-muted">
              An agent reads the conversation and drafts tickets for the board you pick, formatted on the board's
              ticket template. You review before anything is created.
            </p>
            <div className="grid grid-cols-2 gap-3">
              <div>
                <label className="mb-1 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Planner</label>
                <Select value={picked?.id ?? ''} size="sm" onChange={(e) => setAgentModel(e.target.value)} className="w-full">
                  {agents.map((a) => (
                    <option key={a.id} value={a.id}>
                      {a.label}
                    </option>
                  ))}
                </Select>
              </div>
              <div>
                <label className="mb-1 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Model tier</label>
                <Select value={tier} size="sm" onChange={(e) => setTier(e.target.value)} className="w-full">
                  <option value="">main</option>
                  {tiers.map((t) => (
                    <option key={t} value={t}>
                      {t}
                    </option>
                  ))}
                </Select>
              </div>
              <div>
                <label className="mb-1 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Board</label>
                {boardsLoading ? (
                  // Select-shaped stand-in (sm control = h-9) so the field —
                  // and why Draft is still disabled — is visible on open.
                  <Skeleton className="h-9 w-full rounded-xl" />
                ) : (
                  <Select value={boardId} size="sm" onChange={(e) => setBoardId(e.target.value)} className="w-full">
                    <option value="">Pick a board</option>
                    {editable.map((b) => (
                      <option key={b.id} value={b.id}>
                        {b.name}
                      </option>
                    ))}
                  </Select>
                )}
              </div>
              <div>
                <label className="mb-1 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Ticket template</label>
                {templatesLoading ? (
                  <Skeleton className="h-9 w-full rounded-xl" delay={0.12} />
                ) : (
                  <Select value={templateId} size="sm" onChange={(e) => setTemplateId(e.target.value)} className="w-full">
                    <option value="">Automatic (agent → board default)</option>
                    {ticketTemplates.map((t) => (
                      <option key={t.id} value={t.id}>
                        {t.name}
                      </option>
                    ))}
                  </Select>
                )}
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
              <Button size="sm" onClick={() => void draft()} disabled={!picked || !boardId}>
                Draft tickets
              </Button>
            </div>
          </>
        ) : (
          <>
            <div className="max-h-[65vh] space-y-3 overflow-y-auto pr-1">
              {proposals.map((p, i) => (
                <ProposalCard key={i} index={i} proposal={p} all={proposals} onPatch={(patchP) => patch(i, patchP)} />
              ))}
            </div>
            <div className="flex items-center gap-2 border-t border-line-subtle pt-3">
              <span className="text-xs text-muted">
                → <span className="font-medium text-fg">{editable.find((b) => b.id === boardId)?.name}</span>
              </span>
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
                {phase === 'creating'
                  ? `Creating ${included.length} left`
                  : `Create ${included.length} ticket${included.length === 1 ? '' : 's'}`}
              </Button>
            </div>
          </>
        )}
      </div>
    </Modal>
  )
}

// One proposal, room to breathe: title + meta on top, full-width description,
// and a "blocked by" row referencing sibling proposals (created as real
// ticket dependencies).
function ProposalCard({
  index,
  proposal: p,
  all,
  onPatch,
}: {
  index: number
  proposal: Proposal
  all: Proposal[]
  onPatch: (patch: Partial<Proposal>) => void
}) {
  const short = (s: string, n = 36) => (s.length > n ? `${s.slice(0, n)}…` : s)
  const addable = all
    .map((x, j) => ({ x, j }))
    .filter(({ j }) => j !== index && !p.dependsOn.includes(j))

  return (
    <div className={`rounded-lg border border-line bg-panel p-4 ${p.include ? '' : 'opacity-50'}`}>
      <div className="flex items-center gap-2">
        <input
          type="checkbox"
          checked={p.include}
          onChange={(e) => onPatch({ include: e.target.checked })}
          className="shrink-0 accent-[var(--theme-accent)]"
        />
        <span className="w-7 shrink-0 text-right text-xs text-muted">#{index + 1}</span>
        <Input size="sm" value={p.title} onChange={(e) => onPatch({ title: e.target.value })} className="flex-1" />
        <Select value={p.priority} size="sm" onChange={(e) => onPatch({ priority: e.target.value as Priority })} className="w-24 shrink-0">
          {(['low', 'medium', 'high', 'urgent'] as const).map((x) => (
            <option key={x} value={x}>
              {x}
            </option>
          ))}
        </Select>
        <Select
          value={p.effort ?? ''}
          size="sm"
          onChange={(e) => onPatch({ effort: (e.target.value || null) as Effort | null })}
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
        <>
          {/* Draft descriptions are ticket markdown — edit them the way the
              ticket will show them. Seeded per draft; autosave patches up. */}
          <div className="mt-3 max-h-64 overflow-y-auto">
            <RichEditor value={p.description} onSave={(md) => onPatch({ description: md })} autosave minHeight="8rem" />
          </div>
          <div className="mt-2 flex flex-wrap items-center gap-1.5">
            <span className="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Blocked by</span>
            {p.dependsOn.map((d) => (
              <span key={d} className="inline-flex items-center gap-1 rounded-md border border-line bg-raised px-2 py-0.5 text-xs text-fg">
                #{d + 1} {short(all[d]?.title ?? '')}
                <button
                  type="button"
                  className="text-muted hover:text-fg"
                  onClick={() => onPatch({ dependsOn: p.dependsOn.filter((x) => x !== d) })}
                  aria-label={`remove dependency on #${d + 1}`}
                >
                  ×
                </button>
              </span>
            ))}
            {addable.length > 0 && (
              <Select
                value=""
                size="sm"
                onChange={(e) => {
                  const d = Number(e.target.value)
                  if (Number.isInteger(d)) onPatch({ dependsOn: [...p.dependsOn, d] })
                }}
                className="w-40"
              >
                <option value="">+ add</option>
                {addable.map(({ x, j }) => (
                  <option key={j} value={j}>
                    #{j + 1} {short(x.title, 28)}
                  </option>
                ))}
              </Select>
            )}
            {p.dependsOn.length === 0 && addable.length === 0 && <span className="text-xs text-muted">—</span>}
          </div>
        </>
      )}
    </div>
  )
}
