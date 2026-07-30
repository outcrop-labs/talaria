import { createFileRoute } from '@tanstack/react-router'
import { useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { MoreHorizontal, Plus, Lock } from 'lucide-react'
import { Avatar } from '@/components/ui/avatar'
import { Button } from '@/components/ui/button'
import { Panel } from '@/components/ui/panel'
import { Chip } from '@/components/ui/chip'
import { Modal } from '@/components/ui/modal'
import { EmptyState } from '@/components/ui/empty-state'
import { InfoTip } from '@/components/ui/info-tip'
import { SkeletonRows } from '@/components/ui/skeleton'
import { confirm, prompt } from '@/components/ui/confirm'
import { DropdownMenu } from '@/components/ui/context-menu'
import { WorkflowDetail } from '@/components/workflows/workflow-detail'
import { StudioSkillEditor } from '@/components/workflows/skill-editor'
import { StudioGuide, type GuidePrefill } from '@/components/workflows/studio-guide'
import { cn } from '@/lib/cn'
import { useAgents } from '@/lib/agents'
import { useSession } from '@/lib/session'
import { useBoards } from '@/lib/boards'
import {
  copySkillTo,
  deleteSkillReq,
  deleteWorkflow,
  moveSkillTo,
  renameSkill,
  setGapStatus,
  useGaps,
  useSkillLibrary,
  useWorkflows,
  type CapabilityGap,
  type SkillLibraryOwner,
  type TaskWorkflow,
} from '@/lib/workflows'

// The Studio — build your agents, one at a time. The rail picks who you're
// building for ("Every agent" = shared know-how, or one agent); the dashboard
// is that agent's whole working world:
//   Suggestions   what IT asked for (capability gaps, ranked by recurrence)
//   Knows         its skills — own + inherited shared know-how
//   Routed here   the workflows that steer ticket work onto it
// "Teach" runs the guided flow (name → recognize → Muse-drafted skill → done)
// with this agent already chosen. Skills/workflows stay Hermes-native
// underneath — this surface only decides who learns what.
export const Route = createFileRoute('/_app/studio')({
  component: StudioPage,
  // /studio?a=<owner> · &sk=<owner>/<name> opens a skill · &w=<id> a workflow
  validateSearch: (search: Record<string, unknown>): { a?: string; sk?: string; w?: string } => ({
    ...(typeof search.a === 'string' && search.a ? { a: search.a } : {}),
    ...(typeof search.sk === 'string' && search.sk ? { sk: search.sk } : {}),
    ...(typeof search.w === 'string' && search.w ? { w: search.w } : {}),
  }),
})

function StudioPage() {
  const search = Route.useSearch()
  const navigate = Route.useNavigate()
  const qc = useQueryClient()
  const { data: rawOwners = [], isLoading } = useSkillLibrary()
  // PLATFORM skills (talaria-toolkit and friends) are plumbing, not
  // teachable know-how — the Studio doesn't show them at all.
  const owners = rawOwners.map((o) => ({ ...o, skills: o.skills.filter((sk) => !sk.platform) }))
  const { data: workflows = [] } = useWorkflows()
  const { data: gaps = [] } = useGaps()
  const { data: agentsData } = useAgents()
  const { data: boards = [] } = useBoards()
  const { data: session } = useSession()
  const isAdmin = session?.role === 'admin'
  const [guide, setGuide] = useState<(GuidePrefill & { gapId?: string; owner?: string }) | null>(null)

  const agents = owners.filter((o) => o.owner !== 'shared')
  const shared = owners.find((o) => o.owner === 'shared')
  const selectedKey = search.a ?? agents.find((o) => o.canEdit)?.owner ?? agents[0]?.owner ?? 'shared'
  const selected = owners.find((o) => o.owner === selectedKey) ?? shared ?? null
  const select = (a: string) => void navigate({ search: { a } })

  const sharedSkills = new Set(shared?.skills.map((s) => s.name) ?? [])
  const boardName = (id: string) => boards.find((b) => b.id === id)?.name ?? 'a board'
  const agentMeta = (o: SkillLibraryOwner) => agentsData?.agents.find((a) => a.id === o.model)
  const firstName = (o: SkillLibraryOwner) => o.label.split(' ')[0]!

  /** Workflows steering work onto this owner: any bound skill it carries.
   *  The shared view also collects workflows whose skills nobody carries. */
  const routedTo = (o: SkillLibraryOwner): TaskWorkflow[] => {
    const own = new Set(o.skills.map((s) => s.name))
    const carriedBySomeone = new Set(owners.flatMap((x) => x.skills.map((s) => s.name)))
    return workflows.filter((w) => {
      if (o.owner === 'shared') return w.skills.length === 0 || w.skills.some((sk) => sharedSkills.has(sk) || !carriedBySomeone.has(sk))
      return w.skills.some((sk) => own.has(sk) || sharedSkills.has(sk))
    })
  }

  /** Plain-English row for a workflow — what it does, not how it's stored. */
  const describe = (w: TaskWorkflow): string => {
    const where = [
      w.match.boards?.length ? `on ${w.match.boards.map(boardName).join(' or ')}` : '',
      w.match.labels?.length ? `labeled ${w.match.labels.map((l) => `“${l}”`).join(' or ')}` : '',
      w.match.keywords?.length ? `mentioning ${w.match.keywords.map((k) => `“${k}”`).join(' or ')}` : '',
    ]
      .filter(Boolean)
      .join(', ')
    const flow = w.skills.length ? `follow ${w.skills.map((sk) => `“${sk}”`).join(' + ')}` : 'no skills bound yet'
    return `Tickets ${where || '(no rules yet)'} → ${flow}`
  }

  const ownerGaps = (o: SkillLibraryOwner): CapabilityGap[] =>
    o.owner === 'shared' ? gaps : gaps.filter((g) => g.agentModel === o.model)

  // Overlays driven by URL params
  const [skOwner, skName] = search.sk?.split('/', 2) ?? []
  const skOwnerInfo = owners.find((o) => o.owner === skOwner)
  const openWorkflow = workflows.find((w) => w.id === search.w) ?? null
  const closeOverlay = () => void navigate({ search: { a: selectedKey } })

  const removeWorkflow = async (w: TaskWorkflow) => {
    if (!(await confirm({ title: 'Delete workflow', message: `Delete "${w.name}"? Matching tickets dispatch without its skills.`, confirmLabel: 'Delete', danger: true }))) return
    await deleteWorkflow(w.id)
    closeOverlay()
    await qc.invalidateQueries({ queryKey: ['workflows'] })
  }

  if (isLoading)
    return (
      <div className="h-full overflow-y-auto p-8">
        <div className="mx-auto max-w-6xl">
          <SkeletonRows rows={8} avatar />
        </div>
      </div>
    )

  return (
    <div className="h-full overflow-y-auto p-8">
      <div className="mx-auto max-w-6xl">
        <div className="mb-6 flex items-center gap-1.5">
          <h1 className="mercury-text text-2xl font-semibold">Studio</h1>
          <InfoTip text="Build your agents, one at a time: what each one knows (skills), what work gets routed to it (workflows), and what it's asked for help with. Pick who you're building for on the left." />
        </div>

        <div className="grid gap-8 lg:grid-cols-[15rem_minmax(0,1fr)]">
          {/* ── Who you're building for ── */}
          <aside className="space-y-1">
            <div className="mb-2 px-2.5 text-[11px] uppercase tracking-wide text-muted">Building for</div>
            {agents.map((o) => {
              const meta = agentMeta(o)
              const openCount = ownerGaps(o).length
              return (
                <button
                  key={o.owner}
                  type="button"
                  onClick={() => select(o.owner)}
                  className={cn(
                    'flex w-full items-center gap-2.5 rounded-xl px-2.5 py-2 text-left transition-colors',
                    selectedKey === o.owner ? 'bg-card' : 'hover:bg-card/50',
                  )}
                >
                  <Avatar name={o.label} className="h-8 w-8 shrink-0 text-xs" />
                  <span className="min-w-0 flex-1">
                    <span className={cn('block truncate text-sm font-medium', selectedKey === o.owner ? 'text-fg' : 'text-muted')}>{o.label}</span>
                    <span className="block truncate text-[11px] text-muted">{meta?.role ?? o.model ?? ''}</span>
                  </span>
                  {openCount > 0 && <Chip tone="warn">{openCount}</Chip>}
                  {!o.canEdit && <Lock size={12} className="shrink-0 text-muted" />}
                </button>
              )
            })}
            <div className="my-2 border-t border-line-subtle" />
            <button
              type="button"
              onClick={() => select('shared')}
              className={cn(
                'flex w-full items-center gap-2.5 rounded-xl px-2.5 py-2 text-left transition-colors',
                selectedKey === 'shared' ? 'bg-card' : 'hover:bg-card/50',
              )}
            >
              <span className="flex h-8 w-8 shrink-0 items-center justify-center rounded-full border border-line-subtle text-sm text-muted">✦</span>
              <span className="min-w-0 flex-1">
                <span className={cn('block truncate text-sm font-medium', selectedKey === 'shared' ? 'text-fg' : 'text-muted')}>Every agent</span>
                <span className="block text-[11px] text-muted">shared know-how</span>
              </span>
              {shared && !shared.canEdit && <Lock size={12} className="shrink-0 text-muted" />}
            </button>
          </aside>

          {/* ── The selected agent's world ── */}
          {selected ? (
            <main className="min-w-0 space-y-8">
              <div className="flex items-center gap-4">
                {selected.owner === 'shared' ? (
                  <span className="flex h-14 w-14 items-center justify-center rounded-2xl border border-line-subtle text-2xl text-muted">✦</span>
                ) : (
                  <Avatar name={selected.label} className="h-14 w-14 text-lg" />
                )}
                <div className="min-w-0 flex-1">
                  <h2 className="truncate text-xl font-semibold text-fg">{selected.owner === 'shared' ? 'Every agent' : selected.label}</h2>
                  <p className="truncate text-sm text-muted">
                    {selected.owner === 'shared'
                      ? 'Know-how every agent in the fleet carries.'
                      : (agentMeta(selected)?.role ?? selected.model ?? '')}
                  </p>
                </div>
                {selected.canEdit && (
                  <Button onClick={() => setGuide({ owner: selected.owner })}>
                    Teach {selected.owner === 'shared' ? 'everyone' : firstName(selected)}
                  </Button>
                )}
              </div>

              {/* Suggestions — what the agent asked for */}
              {ownerGaps(selected).length > 0 && (
                <section>
                  <SectionTitle
                    title={selected.owner === 'shared' ? 'Agents asked for help with' : `${firstName(selected)} asked for help with`}
                    hint="Work it hit and couldn't do properly — build the know-how and it won't happen again."
                  />
                  <div className="space-y-2">
                    {ownerGaps(selected).map((g) => (
                      <Panel key={g.id} className="border-[color:var(--theme-warning)]/25">
                        <div className="flex items-center gap-2">
                          <span className="text-sm font-medium text-fg">{g.kind.replace(/-/g, ' ')}</span>
                          <Chip tone="warn">{g.seenCount}×</Chip>
                          {selected.owner === 'shared' && <span className="text-xs text-muted">{g.agentModel}</span>}
                          <div className="ml-auto flex items-center gap-1.5">
                            {selected.canEdit && (
                              <Button
                                size="sm"
                                onClick={() =>
                                  setGuide({
                                    owner: selected.owner,
                                    gapId: g.id,
                                    name: g.kind.replace(/-/g, ' '),
                                    describe: `${g.missing}${g.needs ? `\n\nWhat the reporting agent said a flow would need:\n${g.needs}` : ''}`,
                                    boardIds: g.boardId ? [g.boardId] : [],
                                  })
                                }
                              >
                                Build it
                              </Button>
                            )}
                            <Button
                              size="sm"
                              variant="ghost"
                              onClick={() => void setGapStatus(g.id, 'dismissed').then(() => qc.invalidateQueries({ queryKey: ['gaps', 'open'] }))}
                            >
                              Dismiss
                            </Button>
                          </div>
                        </div>
                        <p className="mt-1 text-sm text-muted">{g.missing}</p>
                      </Panel>
                    ))}
                  </div>
                </section>
              )}

              {/* Knows — skills */}
              <section>
                <SectionTitle
                  title={selected.owner === 'shared' ? 'Everyone knows' : `${firstName(selected)} knows`}
                  hint="Skills — the how. Agents read these live; edits apply on their next run."
                  action={
                    selected.canEdit ? (
                      <Button size="sm" variant="outline" onClick={() => setGuide({ owner: selected.owner })}>
                        <Plus size={14} />
                      </Button>
                    ) : undefined
                  }
                />
                {selected.skills.length === 0 && (selected.owner === 'shared' || sharedSkills.size === 0) ? (
                  <Panel>
                    <EmptyState
                      icon="✦"
                      title="Nothing yet"
                      hint={selected.canEdit ? '“Teach” walks you through the first one — name the work, explain it in plain words, Muse drafts the skill.' : 'Nothing taught yet.'}
                    />
                  </Panel>
                ) : (
                  <Panel className="p-0">
                    <div className="divide-y divide-line-subtle">
                      {selected.skills.map((s) => (
                        <SkillRow
                          key={s.name}
                          owner={selected.owner}
                          skill={s}
                          owners={owners}
                          canEdit={selected.canEdit && (!s.platform || isAdmin)}
                          onOpen={() => void navigate({ search: { a: selectedKey, sk: `${selected.owner}/${s.name}` } })}
                        />
                      ))}
                      {selected.owner !== 'shared' &&
                        (shared?.skills ?? []).map((s) => (
                          <SkillRow
                            key={`shared/${s.name}`}
                            owner="shared"
                            skill={s}
                            owners={owners}
                            sharedBadge
                            canEdit={(shared?.canEdit ?? false) && (!s.platform || isAdmin)}
                            onOpen={() => void navigate({ search: { a: selectedKey, sk: `shared/${s.name}` } })}
                          />
                        ))}
                    </div>
                  </Panel>
                )}
              </section>

              {/* Routed here — workflows */}
              <section>
                <SectionTitle
                  title={selected.owner === 'shared' ? 'Routing everyone can serve' : `Work routed to ${firstName(selected)}`}
                  hint="Workflows — the which. When a ticket matches, its skills ride along on dispatch."
                />
                {routedTo(selected).length === 0 ? (
                  <Panel>
                    <p className="py-1 text-sm text-muted">
                      No routing yet.{selected.canEdit ? ' Add recognition rules in “Teach” and matching tickets will carry the flow automatically.' : ''}
                    </p>
                  </Panel>
                ) : (
                  <Panel className="p-0">
                    <div className="divide-y divide-line-subtle">
                      {routedTo(selected).map((w) => (
                        <button
                          key={w.id}
                          type="button"
                          onClick={() => void navigate({ search: { a: selectedKey, w: w.id } })}
                          className="flex w-full items-center gap-3 px-5 py-3 text-left transition-colors hover:bg-card/40"
                        >
                          <span className="min-w-0 flex-1">
                            <span className="block truncate text-sm font-medium text-fg">{w.name}</span>
                            <span className="block truncate text-sm text-muted">{describe(w)}</span>
                          </span>
                          {!w.enabled && <Chip>off</Chip>}
                        </button>
                      ))}
                    </div>
                  </Panel>
                )}
              </section>
            </main>
          ) : (
            <EmptyState icon="◍" title="No agents yet" hint="The Studio lights up once the fleet has agents." />
          )}
        </div>
      </div>

      {/* Overlays */}
      {skOwner && skName && skOwnerInfo && (
        <StudioSkillEditor
          owner={skOwner}
          ownerLabel={skOwnerInfo.label}
          name={skName}
          canEdit={skOwnerInfo.canEdit && (!skOwnerInfo.skills.find((x) => x.name === skName)?.platform || isAdmin)}
          onClose={closeOverlay}
        />
      )}
      {openWorkflow && (
        <Modal open onClose={closeOverlay} width="max-w-2xl" title={openWorkflow.name}>
          <WorkflowDetail
            key={openWorkflow.id}
            workflow={openWorkflow}
            onChanged={() => void qc.invalidateQueries({ queryKey: ['workflows'] })}
            onDelete={() => void removeWorkflow(openWorkflow)}
          />
        </Modal>
      )}
      {guide && (
        <StudioGuide
          open
          onClose={() => setGuide(null)}
          owners={owners}
          prefill={guide}
          onCreated={() => {
            // A gap resolves only once a human actually ratified a skill for
            // it — cancelling the guide leaves the suggestion in place.
            if (guide.gapId) void setGapStatus(guide.gapId, 'resolved').then(() => qc.invalidateQueries({ queryKey: ['gaps', 'open'] }))
          }}
        />
      )}
    </div>
  )
}

function SectionTitle({ title, hint, action }: { title: string; hint: string; action?: React.ReactNode }) {
  return (
    <div className="mb-2 flex items-baseline gap-2">
      <h3 className="text-sm font-semibold text-fg">{title}</h3>
      <span className="min-w-0 truncate text-xs text-muted">{hint}</span>
      {action && <span className="ml-auto">{action}</span>}
    </div>
  )
}

function SkillRow({
  owner,
  skill,
  owners,
  sharedBadge,
  canEdit,
  onOpen,
}: {
  owner: string
  skill: { name: string; description: string; platform?: boolean }
  owners: SkillLibraryOwner[]
  sharedBadge?: boolean
  canEdit: boolean
  onOpen: () => void
}) {
  const qc = useQueryClient()
  const refresh = () => qc.invalidateQueries({ queryKey: ['skill-library'] })
  const editableTargets = owners.filter((o) => o.canEdit && o.owner !== owner)
  const fail = (e: unknown) => void confirm({ title: 'That didn’t work', message: (e as Error).message, confirmLabel: 'OK' })

  const doRename = async () => {
    const to = await prompt({ title: 'Rename skill', message: 'Lowercase, dashes for spaces — agents load it by this name.', confirmLabel: 'Rename' })
    const name = to?.trim().toLowerCase().replace(/\s+/g, '-')
    if (!name || name === skill.name) return
    await renameSkill(owner, skill.name, name).then(refresh).catch(fail)
  }
  const doDelete = async () => {
    if (!(await confirm({ title: 'Delete skill', message: `Delete "${skill.name}"? Workflows bound to it will flag it as missing.`, confirmLabel: 'Delete', danger: true }))) return
    await deleteSkillReq(owner, skill.name).then(refresh).catch(fail)
  }

  return (
    <div className="group flex w-full items-center gap-3 px-5 py-3 transition-colors hover:bg-card/40">
      <button type="button" onClick={onOpen} className="min-w-0 flex-1 text-left">
        <span className="flex items-center gap-2">
          <span className="truncate text-sm font-medium text-fg">{skill.name}</span>
          {skill.platform && (
            <Chip title="Platform skill — essential plumbing, admins only" className="shrink-0">
              <Lock size={9} className="mr-0.5 inline" />
              platform
            </Chip>
          )}
          {sharedBadge && <Chip className="shrink-0">shared</Chip>}
        </span>
        <span className="mt-0.5 block text-sm leading-snug text-muted">{skill.description || '…'}</span>
      </button>
      {canEdit && (
        <DropdownMenu
          trigger={(open) => (
            <span
              className={cn(
                'rounded-md p-1 text-muted transition-opacity hover:text-fg',
                open ? 'opacity-100' : 'opacity-0 group-hover:opacity-100',
              )}
            >
              <MoreHorizontal size={16} />
            </span>
          )}
          items={[
            { label: 'Open', onSelect: onOpen },
            { label: 'Rename…', onSelect: () => void doRename() },
            ...(editableTargets.length
              ? [
                  {
                    label: 'Copy to',
                    children: editableTargets.map((o) => ({
                      label: o.owner === 'shared' ? 'Every agent' : o.label,
                      onSelect: () => void copySkillTo(owner, skill.name, o.owner).then(refresh).catch(fail),
                    })),
                  },
                ]
              : []),
            ...(owner !== 'shared' && editableTargets.some((o) => o.owner === 'shared')
              ? [{ label: 'Promote to every agent', onSelect: () => void moveSkillTo(owner, skill.name, 'shared').then(refresh).catch(fail) }]
              : []),
            'sep' as const,
            { label: 'Delete', danger: true, onSelect: () => void doDelete() },
          ]}
        />
      )}
    </div>
  )
}
