import { createFileRoute } from '@tanstack/react-router'
import { useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { Plus } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Panel } from '@/components/ui/panel'
import { Chip } from '@/components/ui/chip'
import { EmptyState } from '@/components/ui/empty-state'
import { InfoTip } from '@/components/ui/info-tip'
import { SkeletonRows } from '@/components/ui/skeleton'
import { confirm } from '@/components/ui/confirm'
import { useContextMenu, copyAppLink } from '@/components/ui/context-menu'
import { WorkflowDetail } from '@/components/workflows/workflow-detail'
import { StudioSkillEditor } from '@/components/workflows/skill-editor'
import { StudioGuide, type GuidePrefill } from '@/components/workflows/studio-guide'
import { cn } from '@/lib/cn'
import {
  createWorkflow,
  deleteWorkflow,
  setGapStatus,
  useGaps,
  useSkillLibrary,
  useWorkflows,
  type CapabilityGap,
  type SkillLibraryOwner,
  type TaskWorkflow,
} from '@/lib/workflows'

// The Studio — where people tailor how agents work. Two artifact kinds:
//   Skills     the HOW: Hermes SKILL.md files agents mount and live-read,
//              cross-agent view (shared + per-agent), Muse drafts from a
//              plain description. Editable where you hold access: admins /
//              agents.manage everywhere, explicit agent grants for that
//              agent's own skills.
//   Workflows  the WHICH: match rules classifying tickets, bound to the
//              skills that kind of work follows + expected toolkits.
//   Suggested  the honesty loop's queue: capability gaps agents reported,
//              ranked by recurrence — one click turns a gap into a skill
//              draft. Agents propose, humans ratify.
type Tab = 'skills' | 'workflows' | 'suggested'

export const Route = createFileRoute('/_app/studio')({
  component: StudioPage,
  // /studio?tab=workflows&w=<id> · /studio?sk=<owner>/<name>
  validateSearch: (search: Record<string, unknown>): { tab?: Tab; w?: string; sk?: string } => ({
    ...(search.tab === 'workflows' || search.tab === 'suggested' ? { tab: search.tab as Tab } : {}),
    ...(typeof search.w === 'string' && search.w ? { w: search.w } : {}),
    ...(typeof search.sk === 'string' && search.sk ? { sk: search.sk } : {}),
  }),
})

function StudioPage() {
  const search = Route.useSearch()
  const navigate = Route.useNavigate()
  const tab: Tab = search.tab ?? 'skills'
  const { data: owners = [], isLoading: skillsLoading } = useSkillLibrary()
  const { data: workflows = [] } = useWorkflows()
  const { data: gaps = [] } = useGaps()
  const [guide, setGuide] = useState<(GuidePrefill & { gapId?: string }) | null>(null)
  const canTeach = owners.some((o) => o.canEdit)
  const qcTop = useQueryClient()

  const setTab = (t: Tab) => void navigate({ search: t === 'skills' ? {} : { tab: t } })
  const skillCount = owners.reduce((n, o) => n + o.skills.length, 0)

  const TABS: { id: Tab; label: string; count: number }[] = [
    { id: 'skills', label: 'Skills', count: skillCount },
    { id: 'workflows', label: 'Workflows', count: workflows.length },
    { id: 'suggested', label: 'Suggested', count: gaps.length },
  ]

  return (
    <div className="h-full overflow-y-auto p-8">
      <div className="mx-auto max-w-5xl space-y-6">
        <div className="flex items-center gap-1.5">
          <h1 className="mercury-text text-2xl font-semibold">Studio</h1>
          <InfoTip text="Tailor how agents work. Skills are the how — the flow content agents load and follow, edited live. Workflows are the which — match rules that classify tickets and bind them to skills. You can edit skills for agents you've been granted; admins shape the fleet." />
          {canTeach && (
            <Button size="sm" className="ml-auto" onClick={() => setGuide({})}>
              Teach your agents
            </Button>
          )}
        </div>

        <div className="flex gap-1 border-b border-line-subtle">
          {TABS.map((t) => (
            <button
              key={t.id}
              type="button"
              onClick={() => setTab(t.id)}
              className={cn('relative flex items-center gap-1.5 px-3 py-2 text-sm transition-colors', tab === t.id ? 'text-fg' : 'text-muted hover:text-fg')}
            >
              {t.label}
              <span className="text-[10px] text-muted">{t.count}</span>
              {tab === t.id && <span className="absolute inset-x-2 -bottom-px h-0.5 rounded-full bg-accent" />}
            </button>
          ))}
        </div>

        {tab === 'skills' ? (
          <SkillsTab
            owners={owners}
            loading={skillsLoading}
            openSkill={search.sk ?? null}
            setOpenSkill={(sk) => void navigate({ search: { ...(sk ? { sk } : {}) } })}
          />
        ) : tab === 'workflows' ? (
          <WorkflowsTab workflows={workflows} selectedId={search.w ?? null} select={(id) => void navigate({ search: { tab: 'workflows', ...(id ? { w: id } : {}) } })} />
        ) : (
          <SuggestedTab gaps={gaps} owners={owners} teach={(p) => setGuide(p)} />
        )}
      </div>
      {guide && (
        <StudioGuide
          open
          onClose={() => setGuide(null)}
          owners={owners}
          prefill={guide}
          onCreated={() => {
            // A gap is resolved only once a human actually ratified a skill
            // for it — cancelling the guide leaves the suggestion in place.
            if (guide.gapId) void setGapStatus(guide.gapId, 'resolved').then(() => qcTop.invalidateQueries({ queryKey: ['gaps', 'open'] }))
          }}
        />
      )}
    </div>
  )
}

// ── Skills: the cross-agent library ─────────────────────────────────────────

const SKILL_SKELETON = (name: string) =>
  `# ${name}\n\nWhen to use: describe the situation this flow is for — or open Muse and draft it from a description.\n\n## Steps\n\n1. \n`

function SkillsTab({
  owners,
  loading,
  openSkill,
  setOpenSkill,
}: {
  owners: SkillLibraryOwner[]
  loading: boolean
  openSkill: string | null
  setOpenSkill: (sk: string | null) => void
}) {
  const qc = useQueryClient()
  const [drafts, setDrafts] = useState<Record<string, string>>({})

  const create = async (owner: string) => {
    const name = (drafts[owner] ?? '').trim().toLowerCase().replace(/\s+/g, '-')
    if (!name) return
    const r = await fetch(`/api/skills/${owner}/${name}`, {
      method: 'PUT',
      credentials: 'same-origin',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ content: SKILL_SKELETON(name) }),
    })
    if (!r.ok) return
    setDrafts((d) => ({ ...d, [owner]: '' }))
    await qc.invalidateQueries({ queryKey: ['skill-library'] })
    setOpenSkill(`${owner}/${name}`)
  }

  const [openOwner, openName] = openSkill?.split('/', 2) ?? []
  const openOwnerInfo = owners.find((o) => o.owner === openOwner)

  if (loading) return <SkeletonRows rows={6} />
  return (
    <div className="space-y-6">
      {owners.map((o) => (
        <Panel key={o.owner}>
          <div className="mb-2 flex items-center gap-2">
            <span className="text-sm font-semibold text-fg">{o.label}</span>
            {o.owner === 'shared' && <Chip>every agent</Chip>}
            {!o.canEdit && <span className="ml-auto text-[10px] uppercase tracking-wide text-muted">read-only</span>}
          </div>
          {o.skills.length ? (
            <div className="divide-y divide-line-subtle">
              {o.skills.map((s) => (
                <button
                  key={s.name}
                  type="button"
                  onClick={() => setOpenSkill(`${o.owner}/${s.name}`)}
                  className="flex w-full items-baseline gap-3 py-2.5 text-left transition-colors hover:bg-card/40"
                >
                  <span className="shrink-0 text-sm font-medium text-fg">{s.name}</span>
                  <span className="min-w-0 truncate text-sm text-muted">{s.description}</span>
                </button>
              ))}
            </div>
          ) : (
            <p className="py-2 text-xs text-muted">No skills yet.</p>
          )}
          {o.canEdit && (
            <div className="mt-2 flex items-center gap-1.5 border-t border-line-subtle pt-3">
              <Input
                size="sm"
                value={drafts[o.owner] ?? ''}
                onChange={(e) => setDrafts((d) => ({ ...d, [o.owner]: e.target.value }))}
                placeholder="new-skill-name"
                className="w-56"
                onKeyDown={(e) => e.key === 'Enter' && void create(o.owner)}
              />
              <Button size="sm" variant="outline" disabled={!(drafts[o.owner] ?? '').trim()} onClick={() => void create(o.owner)}>
                <Plus size={14} />
              </Button>
              <span className="text-xs text-muted">Name it, then describe it to Muse in the editor.</span>
            </div>
          )}
        </Panel>
      ))}
      {owners.length === 0 && <EmptyState icon="✦" title="No agents yet" hint="Skills appear here once the fleet has agents." />}
      {owners.length > 0 && owners.every((o) => !o.skills.length) && (
        <p className="text-center text-xs text-muted">Nothing taught yet — “Teach your agents” walks you through the first one.</p>
      )}

      {openOwner && openName && openOwnerInfo && (
        <StudioSkillEditor
          owner={openOwner}
          ownerLabel={openOwnerInfo.label}
          name={openName}
          canEdit={openOwnerInfo.canEdit}
          onClose={() => setOpenSkill(null)}
        />
      )}
    </div>
  )
}

// ── Workflows: match rules → skills ─────────────────────────────────────────

function WorkflowsTab({
  workflows,
  selectedId,
  select,
}: {
  workflows: TaskWorkflow[]
  selectedId: string | null
  select: (id: string | null) => void
}) {
  const qc = useQueryClient()
  const { openMenu, menu } = useContextMenu()
  const [newName, setNewName] = useState('')
  const selected = workflows.find((h) => h.id === selectedId) ?? null

  const refresh = () => qc.invalidateQueries({ queryKey: ['workflows'] })
  const create = async () => {
    const name = newName.trim()
    if (!name) return
    setNewName('')
    const { workflow } = await createWorkflow({ name })
    await refresh()
    if (workflow) select(workflow.id)
  }
  const remove = async (h: TaskWorkflow) => {
    if (!(await confirm({ title: 'Delete workflow', message: `Delete "${h.name}"? Matching tickets dispatch without its skills.`, confirmLabel: 'Delete', danger: true }))) return
    await deleteWorkflow(h.id)
    if (selectedId === h.id) select(null)
    await refresh()
  }

  return (
    <div className="grid gap-6 lg:grid-cols-[16rem_minmax(0,1fr)]">
      <aside className="space-y-3">
        <ul className="space-y-0.5">
          {workflows.map((h) => (
            <li key={h.id}>
              <button
                type="button"
                onClick={() => select(h.id)}
                onContextMenu={(e) =>
                  openMenu(e, [
                    { label: 'Open', onSelect: () => select(h.id) },
                    { label: 'Copy link', onSelect: () => copyAppLink(`/studio?tab=workflows&w=${h.id}`) },
                    'sep',
                    { label: 'Delete', danger: true, onSelect: () => void remove(h) },
                  ])
                }
                className={cn(
                  'flex w-full items-center gap-2 rounded-lg px-2.5 py-2 text-left text-sm transition-colors',
                  selected?.id === h.id ? 'bg-card text-fg' : 'text-muted hover:bg-card hover:text-fg',
                )}
              >
                <span className="min-w-0 flex-1 truncate font-sans">{h.name}</span>
                {!h.enabled && <Chip>off</Chip>}
              </button>
            </li>
          ))}
          {workflows.length === 0 && <li className="px-2.5 py-2 text-xs text-muted">None yet.</li>}
        </ul>
        <div className="flex items-center gap-1.5 border-t border-line-subtle pt-3">
          <Input size="sm" value={newName} onChange={(e) => setNewName(e.target.value)} placeholder="New workflow" onKeyDown={(e) => e.key === 'Enter' && void create()} />
          <Button size="sm" variant="outline" disabled={!newName.trim()} onClick={() => void create()}>
            <Plus size={14} />
          </Button>
        </div>
      </aside>

      {selected ? (
        <WorkflowDetail key={selected.id} workflow={selected} onChanged={refresh} onDelete={() => void remove(selected)} />
      ) : (
        <Panel>
          <EmptyState
            icon="⚙"
            title="No workflow selected"
            hint={workflows.length ? 'Pick one on the left, or create a new one.' : 'Create the first one on the left — e.g. "Development" matching your dev board, bound to the skills that kind of work follows.'}
          />
        </Panel>
      )}
      {menu}
    </div>
  )
}

// ── Suggested: the honesty loop's queue ─────────────────────────────────────

function SuggestedTab({
  gaps,
  owners,
  teach,
}: {
  gaps: CapabilityGap[]
  owners: SkillLibraryOwner[]
  teach: (p: GuidePrefill & { gapId?: string }) => void
}) {
  const qc = useQueryClient()
  const editable = owners.filter((o) => o.canEdit)
  const refresh = () => qc.invalidateQueries({ queryKey: ['gaps', 'open'] })

  // "Build it" hands the gap to the guided flow, prefilled with the agent's
  // own words; the gap resolves only when the human ratifies a skill for it.
  const build = (gap: CapabilityGap) => {
    teach({
      name: gap.kind.replace(/-/g, ' '),
      describe: `${gap.missing}${gap.needs ? `\n\nWhat the reporting agent said a flow would need:\n${gap.needs}` : ''}`,
      boardIds: gap.boardId ? [gap.boardId] : [],
      gapId: gap.id,
    })
  }
  const dismiss = async (gap: CapabilityGap) => {
    await setGapStatus(gap.id, 'dismissed')
    refresh()
  }

  if (!gaps.length)
    return (
      <Panel>
        <EmptyState
          icon="✦"
          title="Nothing suggested"
          hint="When an agent hits work it genuinely can't do properly, the gap lands here — ranked by how often the shape recurs — ready to become a skill."
        />
      </Panel>
    )
  return (
    <div className="space-y-3">
      {gaps.map((g) => (
        <Panel key={g.id}>
          <div className="flex items-center gap-2">
            <span className="text-sm font-semibold text-fg">{g.kind}</span>
            <Chip tone={g.seenCount > 2 ? 'warn' : 'neutral'}>{g.seenCount}×</Chip>
            <span className="text-xs text-muted">reported by {g.agentModel}</span>
            <div className="ml-auto flex items-center gap-1.5">
              {editable.length > 0 && (
                <Button size="sm" onClick={() => build(g)}>
                  Build it
                </Button>
              )}
              <Button size="sm" variant="ghost" onClick={() => void dismiss(g)}>
                Dismiss
              </Button>
            </div>
          </div>
          <p className="mt-1.5 text-sm text-muted">{g.missing}</p>
          {g.needs && <p className="mt-1 line-clamp-3 text-xs text-muted">{g.needs}</p>}
          {g.exampleTaskId && (
            <a href={`/boards?t=${g.exampleTaskId}`} className="mt-1.5 inline-block text-xs text-accent hover:underline">
              Example ticket
            </a>
          )}
        </Panel>
      ))}
    </div>
  )
}
