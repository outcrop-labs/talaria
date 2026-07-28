import { useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { Loader2, Sparkles, Trash2 } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Generating } from '@/components/ui/generating'
import { Input } from '@/components/ui/input'
import { Textarea } from '@/components/ui/textarea'
import { Modal } from '@/components/ui/modal'
import { RichEditor } from '@/components/ui/rich-editor'
import { Select } from '@/components/ui/select'
import { createFleetAgent, type AgentDef } from '@/lib/fleet-defs'
import { parseAgentDraft, streamMuse, type AgentDraft } from '@/lib/muse'
import { cn } from '@/lib/cn'

// Spin up a brand-new agent two ways: DESCRIBE it (the AI designs the whole
// agent — identity, soul, starter skills — for review before anything is
// created) or configure it by hand from a template. Either way a template
// supplies the chassis: model tiers, tools, and plugins carry over with
// identity re-stamped; Talaria allocates the key, writes v1, renders, starts.
export function CreateAgentModal({
  open,
  onClose,
  templates,
  templateId: preselect,
}: {
  open: boolean
  onClose: () => void
  templates: AgentDef[]
  /** Preselect a template (e.g. "Duplicate" from a specific agent) — skips the describe step. */
  templateId?: string
}) {
  const qc = useQueryClient()
  const [step, setStep] = useState<'describe' | 'review'>(preselect ? 'review' : 'describe')

  // Describe → generate
  const [purpose, setPurpose] = useState('')
  const [generating, setGenerating] = useState(false)
  const [genPreview, setGenPreview] = useState('')
  const [chat, setChat] = useState<Array<{ role: 'user' | 'assistant'; content: string }>>([])
  const [genErr, setGenErr] = useState<string | null>(null)

  // Review fields (filled by generation or by hand)
  const [displayName, setDisplayName] = useState('')
  const [slug, setSlug] = useState('')
  const [department, setDepartment] = useState('')
  const [role, setRole] = useState('')
  const [soul, setSoul] = useState('')
  const [soulRev, setSoulRev] = useState(0)
  const [skills, setSkills] = useState<AgentDraft['skills']>([])
  const [templateId, setTemplateId] = useState(preselect ?? templates[0]?.id ?? '')  // '' = platform defaults
  const [start, setStart] = useState(true)
  const [busy, setBusy] = useState(false)
  const [err, setErr] = useState<string | null>(null)

  const applyDraft = (d: AgentDraft) => {
    setDisplayName(d.name)
    setSlug(d.handle)
    setDepartment(d.department)
    setRole(d.role)
    setSoul(d.soul)
    setSoulRev((r) => r + 1) // reseed the editor with the new draft
    setSkills(d.skills)
  }

  const currentDraftJson = () =>
    JSON.stringify({ name: displayName, handle: slug, department, role, soul, skills })

  const generate = async (instruction: string, refining: boolean) => {
    if (!instruction.trim()) return
    setGenerating(true)
    setGenErr(null)
    setGenPreview('')
    try {
      const full = await streamMuse(
        {
          kind: 'agent',
          instruction: instruction.trim(),
          ...(refining ? { current: currentDraftJson() } : {}),
          chat,
        },
        (piece) => setGenPreview((p) => p + piece),
      )
      const draft = parseAgentDraft(full)
      if (!draft) return setGenErr('could not design an agent from that. Try adding a sentence about what it should do')
      setChat((c) => [...c.slice(-8), { role: 'user', content: instruction.trim() }, { role: 'assistant', content: full }])
      applyDraft(draft)
      setStep('review')
    } catch (e) {
      setGenErr((e as Error).message)
    } finally {
      setGenerating(false)
    }
  }

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
      const r = await createFleetAgent({
        slug,
        department,
        displayName,
        role: role.trim() || null,
        ...(templateId ? { templateId } : {}),
        ...(soul.trim() ? { soul } : {}),
        ...(skills.length ? { skills } : {}),
        start,
      })
      if (r.error) return setErr(r.error)
      if (start && r.healthy === false) setErr('created, but the container is not healthy yet. Check /agents')
      await qc.invalidateQueries({ queryKey: ['fleet-defs'] })
      await qc.invalidateQueries({ queryKey: ['fleet-containers'] })
      if (!r.error) onClose()
    } finally {
      setBusy(false)
    }
  }

  // ── Step 1: describe ─────────────────────────────────────────────────────
  if (step === 'describe') {
    return (
      <Modal open={open} onClose={onClose} title="New agent" width="max-w-lg">
        <div className="space-y-5">
          <p className="text-sm leading-relaxed text-muted">
            Describe what this agent should do: its job, what it watches, what it produces. The AI designs the whole
            agent (identity, soul, starter skills) for you to review before anything is created.
          </p>
          <div className="flex items-end gap-2.5">
            <Sparkles size={14} className="mb-3 shrink-0 text-accent" />
            <Textarea
              autoGrow
              rows={3}
              value={purpose}
              onChange={(e) => setPurpose(e.target.value)}
              placeholder="e.g. “A release manager that tracks our deploy trains, chases sign-offs before each cut, and posts a go/no-go summary.”"
              className="max-h-48 text-sm"
              autoFocus
            />
          </div>
          {generating && (
            <pre className="max-h-40 overflow-y-auto whitespace-pre-wrap rounded-xl border border-line-subtle p-3 font-[var(--font-mono)] text-xs leading-5 text-muted">
              {genPreview || 'Designing'}
              <span className="animate-pulse text-accent">▍</span>
            </pre>
          )}
          {genErr && <p className="text-xs text-[color:var(--theme-danger)]">{genErr}</p>}
          <div className="flex items-center gap-3 border-t border-line-subtle pt-4">
            <button type="button" className="text-xs text-muted hover:text-fg" onClick={() => setStep('review')}>
              Configure manually →
            </button>
            <span className="ml-auto" />
            <Button variant="ghost" size="sm" onClick={onClose}>
              Cancel
            </Button>
            <Button onClick={() => void generate(purpose, false)} disabled={generating || !purpose.trim()}>
              {generating && <Loader2 size={14} className="animate-spin" />}
              {generating ? 'Designing' : 'Design agent'}
            </Button>
          </div>
        </div>
      </Modal>
    )
  }

  // ── Step 2: review + create ──────────────────────────────────────────────
  const generated = chat.length > 0
  return (
    <Modal open={open} onClose={onClose} title="New agent" width="max-w-2xl">
      <div className="max-h-[75vh] space-y-5 overflow-y-auto pr-1">
        <div className="grid grid-cols-2 gap-4">
          <div>
            <label className="mb-1.5 block text-[11px] uppercase tracking-wide text-muted">Name</label>
            <Input value={displayName} onChange={(e) => onName(e.target.value)} placeholder="Remy" autoFocus={!generated} />
          </div>
          <div>
            <label className="mb-1.5 block text-[11px] uppercase tracking-wide text-muted">Handle</label>
            <Input value={slug} onChange={(e) => setSlug(e.target.value)} placeholder="remy" />
          </div>
        </div>
        <div className="grid grid-cols-2 gap-4">
          <div>
            <label className="mb-1.5 block text-[11px] uppercase tracking-wide text-muted">Role</label>
            <Input value={role} onChange={(e) => setRole(e.target.value)} placeholder="Research Analyst" />
          </div>
          <div>
            <label className="mb-1.5 block text-[11px] uppercase tracking-wide text-muted">Department</label>
            <Input value={department} onChange={(e) => setDepartment(e.target.value)} placeholder="research" />
          </div>
        </div>
        <p className="-mt-2 text-xs text-muted">
          Role is the roster title; department is the routing/mount key. The fleet model id becomes{' '}
          {slug || 'handle'}-{department || 'department'}.
        </p>

        <div>
          <label className="mb-1.5 block text-[11px] uppercase tracking-wide text-muted">
            Chassis template <span className="normal-case">(model tiers, tools, and plugins carry over)</span>
          </label>
          <Select value={templateId} onChange={(e) => setTemplateId(e.target.value)} className="w-full">
            <option value="">Platform defaults: chassis + first local model</option>
            {templates.map((t) => (
              <option key={t.id} value={t.id}>
                {t.displayName} · {t.department} (v{t.currentVersion})
              </option>
            ))}
          </Select>
        </div>

        <div>
          <label className="mb-1.5 block text-[11px] uppercase tracking-wide text-muted">Soul</label>
          {soul.trim() ? (
            // Rich like the post-creation soul editor; autosave keeps `soul`
            // fresh for create + refine, reseeded whenever muse redrafts.
            <div className="max-h-72 overflow-y-auto">
              <RichEditor key={soulRev} value={soul} onSave={setSoul} autosave minHeight="9rem" />
            </div>
          ) : (
            <p className="text-xs text-muted">Starts from a scaffold you edit after creation, or go back and describe the agent to have one designed.</p>
          )}
        </div>

        {skills.length > 0 && (
          <div>
            <label className="mb-1.5 block text-[11px] uppercase tracking-wide text-muted">Starter skills</label>
            <ul className="divide-y divide-line-subtle rounded-lg border border-line-subtle">
              {skills.map((s) => (
                <SkillPreviewRow key={s.name} skill={s} onRemove={() => setSkills(skills.filter((x) => x.name !== s.name))} />
              ))}
            </ul>
          </div>
        )}

        {generated && (
          <RefineBar
            busy={generating}
            preview={generating ? genPreview : null}
            error={genErr}
            onRefine={(text) => void generate(text, true)}
          />
        )}

        <label className="flex cursor-pointer items-center gap-2 text-sm text-fg">
          <input type="checkbox" checked={start} onChange={(e) => setStart(e.target.checked)} className="accent-[color:var(--theme-accent)]" />
          Start the container now
        </label>
        {busy && (
          <Generating
            label={
              start
                ? `Hiring ${displayName || slug}: rendering the config, starting the container, waiting for health`
                : `Creating ${displayName || slug}`
            }
            lines={3}
          />
        )}
        {err && (
          <div className="text-sm" style={{ color: 'var(--theme-danger)' }}>
            {err}
          </div>
        )}
        <div className="flex items-center gap-2 border-t border-line-subtle pt-4">
          {!preselect && (
            <button type="button" className="text-xs text-muted hover:text-fg" onClick={() => setStep('describe')}>
              ← Describe instead
            </button>
          )}
          <span className="ml-auto" />
          <Button variant="ghost" size="sm" onClick={onClose}>
            Cancel
          </Button>
          <Button onClick={() => void create()} disabled={busy || generating || !slug || !department || !displayName}>
            {busy ? 'Creating' : 'Create agent'}
          </Button>
        </div>
      </div>
    </Modal>
  )
}

function SkillPreviewRow({ skill, onRemove }: { skill: { name: string; content: string }; onRemove: () => void }) {
  const [expanded, setExpanded] = useState(false)
  return (
    <li className="px-3.5 py-2.5">
      <div className="flex items-center gap-2">
        <button type="button" onClick={() => setExpanded((v) => !v)} className="min-w-0 flex-1 text-left">
          <span className="text-sm text-fg">{skill.name}</span>
          <span className="ml-2 text-xs text-muted">{expanded ? 'hide' : 'view'}</span>
        </button>
        <button type="button" title="Drop this skill" onClick={onRemove} className="shrink-0 text-muted transition-colors hover:text-[color:var(--theme-danger)]">
          <Trash2 size={14} />
        </button>
      </div>
      {expanded && (
        <pre className="mt-2 max-h-48 overflow-y-auto whitespace-pre-wrap rounded-lg border border-line-subtle p-3 font-[var(--font-mono)] text-xs leading-5 text-muted">
          {skill.content}
        </pre>
      )}
    </li>
  )
}

function RefineBar({
  busy,
  preview,
  error,
  onRefine,
}: {
  busy: boolean
  preview: string | null
  error: string | null
  onRefine: (text: string) => void
}) {
  const [text, setText] = useState('')
  return (
    <div className={cn('space-y-2 rounded-xl border border-line-subtle p-3.5', busy && 'opacity-90')}>
      <div className="flex items-end gap-2.5">
        <Sparkles size={14} className="mb-3 shrink-0 text-accent" />
        <Textarea
          autoGrow
          rows={1}
          value={text}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter' && !e.shiftKey) {
              e.preventDefault()
              if (!busy && text.trim()) {
                onRefine(text)
                setText('')
              }
            }
          }}
          placeholder="Refine the design, e.g. “more formal, and add a weekly retro skill”"
          className="max-h-32 text-sm"
        />
        <Button
          variant="outline"
          className="shrink-0"
          disabled={busy || !text.trim()}
          onClick={() => {
            onRefine(text)
            setText('')
          }}
        >
          {busy ? 'Refining' : 'Refine'}
        </Button>
      </div>
      {busy && preview !== null && (
        <pre className="max-h-32 overflow-y-auto whitespace-pre-wrap rounded-lg border border-line-subtle p-2.5 font-[var(--font-mono)] text-[11px] leading-4 text-muted">
          {preview || 'Designing'}
          <span className="animate-pulse text-accent">▍</span>
        </pre>
      )}
      {error && <p className="text-xs text-[color:var(--theme-danger)]">{error}</p>}
    </div>
  )
}
