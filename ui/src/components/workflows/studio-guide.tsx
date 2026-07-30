// The Studio's guided flow — "Teach your agents" — for people who shouldn't
// need to know that a workflow and a skill are different things. Four small
// steps: name the work, say how to recognize it, explain how it's done (Muse
// drafts the skill from plain words), pick who carries it. One Create at the
// end writes the skill + the workflow together and shows what will happen.
import { useRef, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Textarea } from '@/components/ui/textarea'
import { Modal } from '@/components/ui/modal'
import { Chip } from '@/components/ui/chip'
import { Radio } from '@/components/ui/checkbox'
import { GeneratingDots } from '@/components/ui/generating'
import { Markdown } from '@/components/ui/markdown'
import { cn } from '@/lib/cn'
import { useBoards } from '@/lib/boards'
import { streamMuse } from '@/lib/muse'
import { createWorkflow, updateWorkflow, type SkillLibraryOwner } from '@/lib/workflows'

export interface GuidePrefill {
  name?: string
  describe?: string
  boardIds?: string[]
  /** Preselects who carries the skill (an owner slug or 'shared') — set when
   *  the guide is launched from a specific agent's dashboard. */
  owner?: string
}

const STEPS = ['The work', 'When it applies', 'How it’s done', 'Who does it'] as const

const slugify = (v: string) =>
  v.trim().toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '').slice(0, 60) || 'workflow'

/** Free-entry token row (labels / keywords) with friendlier affordances. */
function Tokens({ value, onChange, placeholder }: { value: string[]; onChange: (v: string[]) => void; placeholder: string }) {
  const [draft, setDraft] = useState('')
  const add = () => {
    const t = draft.trim()
    setDraft('')
    if (t && !value.includes(t)) onChange([...value, t])
  }
  return (
    <div className="flex flex-wrap items-center gap-1.5">
      {value.map((t) => (
        <Chip key={t} onRemove={() => onChange(value.filter((x) => x !== t))}>
          {t}
        </Chip>
      ))}
      <Input
        size="sm"
        value={draft}
        onChange={(e) => setDraft(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === 'Enter') {
            e.preventDefault()
            add()
          }
        }}
        onBlur={add}
        placeholder={placeholder}
        className="w-44"
      />
    </div>
  )
}

export function StudioGuide({
  open,
  onClose,
  owners,
  prefill,
  onCreated,
}: {
  open: boolean
  onClose: () => void
  owners: SkillLibraryOwner[]
  prefill?: GuidePrefill
  onCreated: (workflowId: string | null, skillRef: string) => void
}) {
  const qc = useQueryClient()
  const { data: boards = [] } = useBoards()
  const [step, setStep] = useState(0)

  // Step 1 — the work
  const [name, setName] = useState(prefill?.name ?? '')
  // Step 2 — recognition
  const [boardIds, setBoardIds] = useState<string[]>(prefill?.boardIds ?? [])
  const [labels, setLabels] = useState<string[]>([])
  const [keywords, setKeywords] = useState<string[]>([])
  // Step 3 — the flow
  const [describe, setDescribe] = useState(prefill?.describe ?? '')
  const [skillMd, setSkillMd] = useState('')
  const [drafting, setDrafting] = useState(false)
  const [museError, setMuseError] = useState<string | null>(null)
  const [editingMd, setEditingMd] = useState(false)
  const abortRef = useRef<AbortController | null>(null)
  // Step 4 — placement
  const editable = owners.filter((o) => o.canEdit)
  const [ownerPick, setOwnerPick] = useState<string>(prefill?.owner && owners.some((o) => o.owner === prefill.owner && o.canEdit) ? prefill.owner : '')
  const [creating, setCreating] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [done, setDone] = useState<string | null>(null) // skill ref when created

  const hasRules = boardIds.length > 0 || labels.length > 0 || keywords.length > 0
  const owner = ownerPick || (editable.some((o) => o.owner === 'shared') ? 'shared' : (editable[0]?.owner ?? ''))
  const boardName = (id: string) => boards.find((b) => b.id === id)?.name ?? 'a board'

  /** The plain-English promise of what this will do — shown from step 2 on. */
  const sentence = () => {
    const where = [
      boardIds.length ? `on ${boardIds.map(boardName).join(' or ')}` : '',
      labels.length ? `labeled ${labels.map((l) => `“${l}”`).join(' or ')}` : '',
      keywords.length ? `mentioning ${keywords.map((k) => `“${k}”`).join(' or ')}` : '',
    ]
      .filter(Boolean)
      .join(', ')
    const who =
      owner === 'shared' ? 'any agent picking it up' : (owners.find((o) => o.owner === owner)?.label ?? 'the agent')
    return `Tickets ${where || '(no recognition rules yet)'} → ${who} follows “${slugify(name)}”.`
  }

  const draft = async () => {
    if (!describe.trim()) return
    setDrafting(true)
    setMuseError(null)
    setSkillMd('')
    abortRef.current?.abort()
    abortRef.current = new AbortController()
    try {
      const text = await streamMuse(
        {
          kind: 'skill',
          instruction: describe,
          context: `A skill named "${slugify(name)}" — how "${name}" work is done at this org. SKILL.md format: a "# ${slugify(name)}" heading, one "When to use:" line, then concrete numbered steps a competent agent can follow.`,
        },
        (piece) => setSkillMd((s) => s + piece),
        abortRef.current.signal,
      )
      setSkillMd(text)
    } catch (e) {
      if ((e as Error).name !== 'AbortError') setMuseError((e as Error).message)
    } finally {
      setDrafting(false)
    }
  }

  const create = async () => {
    if (!owner || !skillMd.trim()) return
    setCreating(true)
    setError(null)
    try {
      // A colliding name gets a numeric suffix — never overwrite an existing skill.
      const taken = new Set(owners.find((o) => o.owner === owner)?.skills.map((s) => s.name) ?? [])
      let skillName = slugify(name)
      for (let i = 2; taken.has(skillName); i++) skillName = `${slugify(name)}-${i}`
      const put = await fetch(`/api/skills/${owner}/${skillName}`, {
        method: 'PUT',
        credentials: 'same-origin',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ content: skillMd }),
      })
      if (!put.ok) throw new Error('could not save the skill')
      let workflowId: string | null = null
      if (hasRules) {
        const { workflow } = await createWorkflow({ name: name.trim() })
        workflowId = workflow?.id ?? null
        if (workflowId) {
          await updateWorkflow(workflowId, {
            match: {
              ...(boardIds.length ? { boards: boardIds } : {}),
              ...(labels.length ? { labels } : {}),
              ...(keywords.length ? { keywords } : {}),
            },
            skills: [skillName],
          })
        }
      }
      await qc.invalidateQueries({ queryKey: ['skill-library'] })
      await qc.invalidateQueries({ queryKey: ['workflows'] })
      setDone(`${owner}/${skillName}`)
      onCreated(workflowId, `${owner}/${skillName}`)
    } catch (e) {
      setError((e as Error).message)
    } finally {
      setCreating(false)
    }
  }

  const canNext = [name.trim().length > 1, true, skillMd.trim().length > 0, !!owner][step]

  return (
    <Modal open={open} onClose={onClose} width="max-w-2xl" title={done ? 'Your agents learned something new' : 'Teach your agents'}>
      {done ? (
        <div className="space-y-4">
          <p className="text-sm text-fg">{sentence()}</p>
          <p className="text-sm text-muted">
            {hasRules
              ? 'From now on, matching tickets carry this flow when they’re dispatched. You can refine the skill or the rules any time in the Studio.'
              : 'The skill is in the library. Add recognition rules later (a workflow) and matching tickets will carry it automatically.'}
          </p>
          <div className="flex justify-end gap-2">
            <Button variant="outline" size="sm" onClick={onClose}>
              Done
            </Button>
          </div>
        </div>
      ) : (
        <div className="space-y-5">
          {/* Stepper */}
          <div className="flex items-center gap-2">
            {STEPS.map((s, i) => (
              <button
                key={s}
                type="button"
                onClick={() => i < step && setStep(i)}
                className={cn(
                  'flex items-center gap-1.5 text-xs transition-colors',
                  i === step ? 'text-fg' : i < step ? 'text-accent' : 'text-muted',
                  i < step && 'cursor-pointer',
                )}
              >
                <span
                  className={cn(
                    'flex h-5 w-5 items-center justify-center rounded-full border text-[10px]',
                    i === step ? 'border-accent text-fg' : i < step ? 'border-accent text-accent' : 'border-line-subtle',
                  )}
                >
                  {i < step ? '✓' : i + 1}
                </span>
                {s}
                {i < STEPS.length - 1 && <span className="mx-0.5 h-px w-4 bg-line-subtle" />}
              </button>
            ))}
          </div>

          {step === 0 && (
            <div className="space-y-3">
              <p className="text-sm text-muted">What kind of work are you teaching? Name it the way your team talks about it.</p>
              <Input
                autoFocus
                value={name}
                onChange={(e) => setName(e.target.value)}
                placeholder={'e.g. "Weekly metrics report" or "Bug triage"'}
                onKeyDown={(e) => e.key === 'Enter' && name.trim().length > 1 && setStep(1)}
              />
            </div>
          )}

          {step === 1 && (
            <div className="space-y-4">
              <p className="text-sm text-muted">
                How should Talaria recognize this work? Pick anything that applies — tickets matching it will carry your flow when an agent
                picks them up. You can also skip this and just add the know-how to the library.
              </p>
              <div className="space-y-1.5">
                <span className="text-xs font-medium text-fg">On these boards</span>
                <div className="flex flex-wrap gap-1.5">
                  {boards.map((b) => (
                    <Chip
                      key={b.id}
                      selected={boardIds.includes(b.id)}
                      onSelect={() => setBoardIds((v) => (v.includes(b.id) ? v.filter((x) => x !== b.id) : [...v, b.id]))}
                    >
                      {b.name}
                    </Chip>
                  ))}
                  {boards.length === 0 && <span className="text-xs text-muted">No boards yet.</span>}
                </div>
              </div>
              <div className="space-y-1.5">
                <span className="text-xs font-medium text-fg">With these labels</span>
                <Tokens value={labels} onChange={setLabels} placeholder="Add a label…" />
              </div>
              <div className="space-y-1.5">
                <span className="text-xs font-medium text-fg">Or mentioning</span>
                <Tokens value={keywords} onChange={setKeywords} placeholder="Add a phrase…" />
              </div>
              <p className="rounded-lg border border-line-subtle bg-card/40 px-3 py-2 text-xs text-muted">{sentence()}</p>
            </div>
          )}

          {step === 2 && (
            <div className="space-y-3">
              <p className="text-sm text-muted">
                Explain how this work should be done, like you would to a new teammate — what matters, what order, what “done” looks like.
                Muse turns it into a skill your agents follow.
              </p>
              <Textarea
                autoGrow
                rows={3}
                value={describe}
                onChange={(e) => setDescribe(e.target.value)}
                placeholder={'e.g. "Pull last week\'s numbers first, compare to the previous 4 weeks, lead with the one change that matters, keep it under a page…"'}
                className="max-h-40 text-sm"
              />
              <div className="flex items-center gap-2">
                <Button size="sm" onClick={() => void draft()} disabled={!describe.trim() || drafting}>
                  {skillMd ? 'Redraft' : 'Draft the skill'}
                </Button>
                {drafting && (
                  <span className="flex items-center gap-1.5 text-xs text-muted">
                    Muse is drafting <GeneratingDots />
                  </span>
                )}
                {museError && <span className="text-xs text-[color:var(--theme-danger)]">{museError}</span>}
              </div>
              {skillMd &&
                (editingMd ? (
                  <Textarea autoGrow rows={8} value={skillMd} onChange={(e) => setSkillMd(e.target.value)} className="max-h-72 font-mono text-xs" />
                ) : (
                  <button
                    type="button"
                    onClick={() => !drafting && setEditingMd(true)}
                    title="Click to edit"
                    className="block max-h-72 w-full overflow-y-auto rounded-xl border border-line-subtle bg-card/40 px-4 py-3 text-left text-sm"
                  >
                    <Markdown className="tiptap">{skillMd}</Markdown>
                  </button>
                ))}
            </div>
          )}

          {step === 3 && (
            <div className="space-y-4">
              <p className="text-sm text-muted">Who should know this?</p>
              <div className="space-y-2">
                {editable.some((o) => o.owner === 'shared') && (
                  <Radio name="owner" checked={owner === 'shared'} onChange={() => setOwnerPick('shared')} label="Every agent — shared know-how" />
                )}
                {editable
                  .filter((o) => o.owner !== 'shared')
                  .map((o) => (
                    <Radio key={o.owner} name="owner" checked={owner === o.owner} onChange={() => setOwnerPick(o.owner)} label={`Just ${o.label}`} />
                  ))}
                {editable.length === 0 && (
                  <p className="text-xs text-[color:var(--theme-warning)]">You don’t have edit access to any agent’s skills — ask an admin for access.</p>
                )}
              </div>
              <p className="rounded-lg border border-line-subtle bg-card/40 px-3 py-2 text-xs text-muted">{sentence()}</p>
              {error && <p className="text-xs text-[color:var(--theme-danger)]">{error}</p>}
            </div>
          )}

          <div className="flex items-center justify-between border-t border-line-subtle pt-3">
            <Button variant="ghost" size="sm" onClick={step === 0 ? onClose : () => setStep(step - 1)}>
              {step === 0 ? 'Cancel' : 'Back'}
            </Button>
            {step < STEPS.length - 1 ? (
              <Button size="sm" onClick={() => setStep(step + 1)} disabled={!canNext}>
                {step === 1 && !hasRules ? 'Skip for now' : 'Next'}
              </Button>
            ) : (
              <Button size="sm" onClick={() => void create()} disabled={!canNext || creating || !skillMd.trim()}>
                {creating ? 'Creating…' : hasRules ? 'Create workflow + skill' : 'Add to the library'}
              </Button>
            )}
          </div>
        </div>
      )}
    </Modal>
  )
}
