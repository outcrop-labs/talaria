import { createFileRoute } from '@tanstack/react-router'
import { Skeleton, SkeletonCard } from '@/components/ui/skeleton'
import { useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { Play, Square, SlidersHorizontal, Archive, CalendarClock, Import, LayoutGrid, List, Copy, RotateCw, Repeat, Trash2, UserPlus, Plus } from 'lucide-react'
import { GeneratingBars } from '@/components/ui/generating'
import { Panel } from '@/components/ui/panel'
import { Segmented } from '@/components/ui/segmented'
import { confirm } from '@/components/ui/confirm'
import { Avatar } from '@/components/ui/avatar'
import { Button } from '@/components/ui/button'
import { EmptyState } from '@/components/ui/empty-state'
import { Input } from '@/components/ui/input'
import { Modal } from '@/components/ui/modal'
import { QueryError, QueryState } from '@/components/ui/query-state'
import { errorMessage } from '@/lib/fetch-json'
import { useFleet } from '@/lib/fleet'
import { useSession } from '@/lib/session'
import { cn } from '@/lib/cn'
import { useContextMenu, type ContextMenuEntry } from '@/components/ui/context-menu'
import { AgentManageModal } from '@/components/fleet/agent-manage-modal'
import { CreateAgentModal } from '@/components/fleet/create-agent-modal'
import { FleetCronsModal } from '@/components/fleet/agent-crons'
import { FederateModal } from '@/components/fleet/federate-modal'
import {
  controlAgent,
  useFleetContainers,
  useFleetDefs,
  type AgentBrainHealth,
  type AgentContainers,
  type AgentDef,
  type FleetAction,
  type LlmEndpoint,
} from '@/lib/fleet-defs'

export const Route = createFileRoute('/_app/agents')({
  component: AgentsPage,
})

// ── Health: one word from container reality. 'warming' = the healthcheck's
// start_period — the container is up but its gateway isn't serving yet. ──
type Health = 'up' | 'warming' | 'degraded' | 'down' | 'retired'
const HEALTH_COLOR: Record<Health, string> = {
  up: 'var(--theme-success)',
  warming: 'var(--theme-warning)',
  degraded: 'var(--theme-warning)',
  down: 'var(--theme-danger)',
  retired: 'var(--theme-line)',
}
/** "brain unroutable" chip — the agent's configured model lost its gateway
 *  route (provider-pool churn). Main = red, tier/fallback only = amber. */
function BrainChip({ brain }: { brain?: AgentBrainHealth }) {
  if (!brain) return null
  const bad = brain.targets.filter((t) => !t.ok)
  if (bad.length === 0) return null
  const detail = bad.map((t) => `${t.kind}${t.name ? ` "${t.name}"` : ''} → ${t.endpoint}/${t.model}: ${t.reason}`).join('\n')
  // Signal reads as an OUTLINE chip (spec §8: orange is never a fill).
  return (
    <span
      title={detail}
      className={cn(
        'shrink-0 rounded border px-1.5 py-0.5 font-mono text-[10px] uppercase tracking-[0.05em]',
        !brain.ok ? 'border-danger/40 text-danger' : 'border-warning/40 text-warning',
      )}
    >
      {!brain.ok ? 'brain unroutable' : `${bad.length} tier${bad.length === 1 ? '' : 's'} down`}
    </span>
  )
}

function healthOf(d: AgentDef, c: AgentContainers | null): { health: Health; running: boolean } {
  if (!d.enabled) return { health: 'retired', running: false }
  const state = c?.managed ?? null
  const running = state?.state === 'running'
  if (!running) return { health: 'down', running: false }
  if (state?.health === 'starting') return { health: 'warming', running: true }
  return { health: state?.health === 'unhealthy' ? 'degraded' : 'up', running: true }
}

// The roster's placeholder — same tile grid the content lands in.
function RosterSkeleton() {
  return (
    <div className="grid grid-cols-2 gap-3 sm:grid-cols-3">
      {[0, 1, 2, 3, 4, 5].map((i) => (
        <SkeletonCard key={i} delay={i * 0.1} />
      ))}
    </div>
  )
}

function AgentsPage() {
  const { data: session } = useSession()
  const isAdmin = session?.role === 'admin'
  const fleetQuery = useFleet()
  const defsQuery = useFleetDefs(isAdmin)
  const containersQuery = useFleetContainers(isAdmin)
  const { data: containers = [], isLoading: containersLoading } = containersQuery
  const byDept = new Map(containers.map((c) => [c.department, c]))
  // The roster itself renders through QueryState below (a 500 must never read as
  // "no agents"). This copy is only for the create/duplicate modals' template
  // list, where an empty list is a harmless degradation.
  const defs = defsQuery.data?.defs ?? []
  const t = fleetQuery.data?.totals

  const [view, setView] = useState<'grid' | 'list'>('grid')
  const [creating, setCreating] = useState(false)
  const [duplicateFrom, setDuplicateFrom] = useState<AgentDef | null>(null)
  const [schedulesOpen, setSchedulesOpen] = useState(false)
  const [federateOpen, setFederateOpen] = useState(false)

  return (
    <div className="h-full overflow-y-auto p-8">
      <div className="mx-auto max-w-5xl space-y-6">
        <div className="flex flex-wrap items-center gap-3">
          <h1 className="font-sans text-2xl font-semibold tracking-tight text-fg">Agents</h1>
          {t ? (
            <span className="font-mono text-[11px] tracking-[0.05em] text-muted">{t.online}/{t.agents} online · {t.activeToday} active today</span>
          ) : fleetQuery.isError ? (
            // Missing totals with no explanation read as "nothing is running".
            <span className="font-mono text-[11px] tracking-[0.05em] text-danger" title={errorMessage(fleetQuery.error)}>
              Fleet status unavailable
            </span>
          ) : fleetQuery.isLoading ? (
            // Hold the slot so the toolbar doesn't jog when the totals land.
            <Skeleton className="h-3 w-40" />
          ) : null}
          <div className="ml-auto flex items-center gap-2">
            {/* Grid / list toggle — the §8 segmented tile pair. */}
            <Segmented
              options={[
                { id: 'grid', label: <LayoutGrid size={14} />, title: 'Grid' },
                { id: 'list', label: <List size={14} />, title: 'List' },
              ]}
              value={view}
              onChange={setView}
            />
            {isAdmin && (
              <>
                <Button size="sm" className="w-9 px-0" onClick={() => setCreating(true)} title="New agent" aria-label="New agent">
                  <Plus size={16} />
                </Button>
                <Button
                  variant="outline"
                  size="sm"
                  className="w-9 px-0"
                  onClick={() => setSchedulesOpen(true)}
                  title="Schedules: crons across the fleet"
                  aria-label="Schedules"
                >
                  <CalendarClock size={15} />
                </Button>
                <Button
                  variant="outline"
                  size="sm"
                  className="w-9 px-0"
                  onClick={() => setFederateOpen(true)}
                  title="Federate outside agents into Talaria"
                  aria-label="Federate agents"
                >
                  <Import size={15} />
                </Button>
              </>
            )}
          </div>
        </div>

        {/* "No agents yet" is a claim about the fleet, and for months a 500 from
            /api/fleet/defs made this surface state it — the owner reading that
            his fleet was empty while the box was down. It is now reachable ONLY
            from a 200 that really carried zero defs; the failure renders as a
            failure, and a disabled read (no manage access) says that instead. */}
        <QueryState
          query={defsQuery}
          errorTitle="Could not load the agent roster"
          errorVariant="compact"
          isEmpty={(d) => d.defs.length === 0}
          skeleton={<RosterSkeleton />}
          idle={
            <Panel>
              <EmptyState
                title="Roster not available to you"
                hint="Agent definitions are served to accounts that manage the fleet — ask an admin for access."
              />
            </Panel>
          }
          empty={
            <Panel>
              <EmptyState
                title="No agents yet"
                hint="Describe the first one and Muse designs it: identity, soul, and starter skills."
                action={
                  <Button size="sm" onClick={() => setCreating(true)}>
                    Design your first agent
                  </Button>
                }
              />
            </Panel>
          }
        >
          {(data) => {
            const endpoints = data.endpoints
            const brainByAgent = new Map((data.brains ?? []).map((b) => [b.agent, b]))
            // Defs AND containers gate the grid together: tiles built from
            // `containers: null` read every agent as stopped, then flip.
            if (containersLoading) return <RosterSkeleton />
            return (
              <div className="space-y-3">
                {/* Docker unreachable ≠ every agent stopped. Without this the
                    dots all go red and the roster quietly libels the fleet. */}
                {containersQuery.isError && (
                  <QueryError
                    variant="inline"
                    title={containersQuery.data === undefined ? 'Could not read container status' : 'Container status may be out of date'}
                    error={containersQuery.error}
                    onRetry={() => void containersQuery.refetch()}
                  />
                )}
                {view === 'grid' ? (
                  <div className="grid grid-cols-2 gap-3 sm:grid-cols-3">
                    {data.defs.map((d) => (
                      <AgentTile key={d.id} def={d} containers={byDept.get(d.department) ?? null} endpoints={endpoints} brain={brainByAgent.get(d.model)} onDuplicate={() => setDuplicateFrom(d)} />
                    ))}
                  </div>
                ) : (
                  <Panel className="p-0">
                    <ul className="divide-y divide-line">
                      {data.defs.map((d) => (
                        <AgentListRow key={d.id} def={d} containers={byDept.get(d.department) ?? null} endpoints={endpoints} brain={brainByAgent.get(d.model)} onDuplicate={() => setDuplicateFrom(d)} />
                      ))}
                    </ul>
                  </Panel>
                )}
              </div>
            )
          }}
        </QueryState>

        {schedulesOpen && <FleetCronsModal onClose={() => setSchedulesOpen(false)} />}
        {federateOpen && <FederateModal onClose={() => setFederateOpen(false)} />}
        {creating && <CreateAgentModal open={creating} onClose={() => setCreating(false)} templates={defs.filter((d) => d.enabled)} />}
        {duplicateFrom && (
          <CreateAgentModal open onClose={() => setDuplicateFrom(null)} templates={defs} templateId={duplicateFrom.id} />
        )}
      </div>
    </div>
  )
}


// A tiny icon button used for the row/tile controls.
function IconBtn({ icon, title, onClick, danger, disabled }: { icon: React.ReactNode; title: string; onClick: () => void; danger?: boolean; disabled?: boolean }) {
  return (
    <button
      type="button"
      title={title}
      onClick={onClick}
      disabled={disabled}
      className={cn(
        'grid h-8 w-8 place-items-center rounded-md text-muted transition-colors hover:bg-hover disabled:opacity-40',
        danger ? 'hover:text-danger' : 'hover:text-fg',
      )}
    >
      {icon}
    </button>
  )
}

// Confirm copy shared by the Controls buttons and the right-click menu, so the
// two paths can never drift apart.
const RESTART_CONFIRM = 'Restart this agent now? Any reply it is mid-way through will be dropped.'
const deleteForeverConfirm = (d: AgentDef) =>
  `Permanently delete ${d.displayName}? This removes its definition, version history, secrets, and (for Talaria-created agents) its memory volume. Chats, tickets, and ledger history it produced are kept. This cannot be undone.`

// Shared control logic for one agent def.
function useAgentControls(d: AgentDef) {
  const qc = useQueryClient()
  const [pending, setPending] = useState<string | null>(null)
  const act = async (action: FleetAction, label: string, confirmMsg?: string) => {
    if (confirmMsg && !(await confirm({ title: label, message: confirmMsg, confirmLabel: label }))) return
    setPending(label)
    try {
      await controlAgent(d.id, action)
      await qc.invalidateQueries({ queryKey: ['fleet-containers'] })
      await qc.invalidateQueries({ queryKey: ['fleet-defs'] })
    } finally {
      setPending(null)
    }
  }
  return { pending, act }
}

/** The control-icon cluster (start/stop/restart/roll · manage · retire · re-hire). */
function Controls({ def: d, running, onManage, onDuplicate }: { def: AgentDef; running: boolean; onManage: () => void; onDuplicate: () => void }) {
  const { data: session } = useSession()
  const isAdmin = session?.role === 'admin'
  const { pending, act } = useAgentControls(d)
  const [retiring, setRetiring] = useState(false)
  if (pending) return <GeneratingBars bars={3} variant="weave" step={0.15} className="text-muted" />
  // Retired agents: re-hire (re-enable + start), duplicate as a template, or
  // delete forever (admin) — the only truly destructive lifecycle action.
  if (!d.enabled)
    return (
      <div className="flex items-center">
        <IconBtn icon={<Copy size={15} />} title="Duplicate to a new agent" onClick={onDuplicate} />
        <IconBtn icon={<UserPlus size={15} />} title="Re-hire" onClick={() => void act('unretire', 're-hiring')} />
        {isAdmin && (
          <IconBtn
            icon={<Trash2 size={15} />}
            title="Delete forever"
            danger
            onClick={() => void act('delete', 'Delete forever', deleteForeverConfirm(d))}
          />
        )}
      </div>
    )
  return (
    <div className="flex items-center">
      <IconBtn icon={<Copy size={15} />} title="Duplicate to a new agent" onClick={onDuplicate} />
      <IconBtn icon={<SlidersHorizontal size={15} />} title="Manage" onClick={onManage} />
      <IconBtn icon={<Archive size={15} />} title="Retire" danger onClick={() => setRetiring(true)} />
      {retiring && <RetireModal def={d} onClose={() => setRetiring(false)} onConfirm={() => void act('retire', 'retiring')} />}
      {/* Start/stop stands apart from the rest — it's the lifecycle switch,
          not another management action. Filled glyphs so they read at 14px. */}
      <span aria-hidden className="mx-1.5 h-4 w-px bg-line" />
      {running ? (
        <>
          <IconBtn icon={<Square size={14} fill="currentColor" />} title="Stop" onClick={() => void act('stop', 'stopping')} />
          <IconBtn
            icon={<RotateCw size={14} />}
            title="Restart (quick bounce — drops any in-flight reply)"
            onClick={() => void act('restart', 'restarting', RESTART_CONFIRM)}
          />
          {isAdmin && (
            <IconBtn
              icon={<Repeat size={14} />}
              title="Roll (zero-downtime replacement — fresh container, old one finishes its replies)"
              onClick={() => void act('roll', 'rolling')}
            />
          )}
        </>
      ) : (
        <IconBtn icon={<Play size={14} fill="currentColor" />} title="Start" onClick={() => void act('up', 'starting')} />
      )}
    </div>
  )
}

// Retiring removes the container and drops the agent from the fleet — a
// destructive action, so it's a double opt-in: type the agent's slug to confirm.
function RetireModal({ def: d, onClose, onConfirm }: { def: AgentDef; onClose: () => void; onConfirm: () => void }) {
  const [typed, setTyped] = useState('')
  const match = typed.trim() === d.slug
  return (
    <Modal open onClose={onClose} title={`Retire ${d.displayName}?`} width="max-w-md">
      <div className="space-y-4">
        <p className="text-sm text-muted">
          The running container is removed and <span className="text-fg">{d.displayName}</span> leaves the fleet. Its state
          volume (memories, plans) and version history are kept, so it can be brought back later.
        </p>
        <div>
          <label className="mb-1 block text-xs text-muted">
            Type <code className="text-fg">{d.slug}</code> to confirm
          </label>
          <Input value={typed} onChange={(e) => setTyped(e.target.value)} placeholder={d.slug} autoFocus />
        </div>
        <div className="flex justify-end gap-2 border-t border-line pt-3">
          <Button variant="ghost" size="sm" onClick={onClose}>
            Cancel
          </Button>
          <Button
            size="sm"
            variant="danger"
            disabled={!match}
            onClick={() => {
              onConfirm()
              onClose()
            }}
          >
            Retire agent
          </Button>
        </div>
      </div>
    </Modal>
  )
}

// Right-click menu for a tile / list row — the same actions as the Controls
// cluster (same admin guards, same confirms), reachable from anywhere on the
// row. Its own useAgentControls instance drives the acts; RetireModal keeps
// its double opt-in.
function useAgentMenu(d: AgentDef, running: boolean, onManage: () => void, onDuplicate: () => void) {
  const { data: session } = useSession()
  const isAdmin = session?.role === 'admin'
  const { act } = useAgentControls(d)
  const [retiring, setRetiring] = useState(false)
  const { openMenu, menu } = useContextMenu()

  const onContextMenu = (e: React.MouseEvent) => {
    if (!d.enabled) {
      // Retired: re-hire, duplicate as a template, delete forever (admin).
      openMenu(e, [
        { label: 'Duplicate to a new agent', icon: <Copy size={14} />, onSelect: onDuplicate },
        { label: 'Re-hire', icon: <UserPlus size={14} />, onSelect: () => void act('unretire', 're-hiring') },
        ...(isAdmin
          ? ([
              'sep',
              {
                label: 'Delete forever',
                icon: <Trash2 size={14} />,
                danger: true,
                onSelect: () => void act('delete', 'Delete forever', deleteForeverConfirm(d)),
              },
            ] as ContextMenuEntry[])
          : []),
      ])
      return
    }
    openMenu(e, [
      { label: 'Manage', icon: <SlidersHorizontal size={14} />, onSelect: onManage },
      { label: 'Duplicate to a new agent', icon: <Copy size={14} />, onSelect: onDuplicate },
      'sep',
      ...(running
        ? ([
            { label: 'Stop', icon: <Square size={13} fill="currentColor" />, onSelect: () => void act('stop', 'stopping') },
            { label: 'Restart', icon: <RotateCw size={13} />, onSelect: () => void act('restart', 'restarting', RESTART_CONFIRM) },
            ...(isAdmin ? [{ label: 'Roll', icon: <Repeat size={13} />, onSelect: () => void act('roll', 'rolling') }] : []),
          ] as ContextMenuEntry[])
        : ([
            { label: 'Start', icon: <Play size={13} fill="currentColor" />, onSelect: () => void act('up', 'starting') },
          ] as ContextMenuEntry[])),
      'sep',
      { label: 'Retire', icon: <Archive size={14} />, danger: true, onSelect: () => setRetiring(true) },
    ])
  }

  const overlays = (
    <>
      {menu}
      {retiring && <RetireModal def={d} onClose={() => setRetiring(false)} onConfirm={() => void act('retire', 'retiring')} />}
    </>
  )
  return { onContextMenu, overlays }
}

function StatusDot({ def: d, containers }: { def: AgentDef; containers: AgentContainers | null }) {
  const { health } = healthOf(d, containers)
  // Spec §8 status dot: 6–7px round, signal color carries the meaning.
  return (
    <span
      className={cn('h-[7px] w-[7px] shrink-0 rounded-full', health === 'warming' && 'gd-breathe')}
      style={{ background: HEALTH_COLOR[health] }}
      title={health === 'warming' ? 'warming up' : health}
    />
  )
}

function AgentTile({ def: d, containers, endpoints, brain, onDuplicate }: { def: AgentDef; containers: AgentContainers | null; endpoints: LlmEndpoint[]; brain?: AgentBrainHealth; onDuplicate: () => void }) {
  const [manage, setManage] = useState(false)
  const { running } = healthOf(d, containers)
  const { onContextMenu, overlays } = useAgentMenu(d, running, () => setManage(true), onDuplicate)
  return (
    <>
      <Panel onContextMenu={onContextMenu} className={cn('flex flex-col gap-3', !d.enabled && 'opacity-60')}>
        <div className="flex items-center gap-2.5">
          <Avatar name={d.displayName} className="h-9 w-9" />
          <button type="button" onClick={() => setManage(true)} className="min-w-0 flex-1 text-left">
            <div className="truncate text-sm font-medium text-fg">{d.displayName}</div>
            <div className="truncate text-xs text-muted">{d.role ?? `v${d.currentVersion}`}</div>
          </button>
          <BrainChip brain={brain} />
          <StatusDot def={d} containers={containers} />
        </div>
        <div className="flex justify-end">
          <Controls def={d} running={running} onManage={() => setManage(true)} onDuplicate={onDuplicate} />
        </div>
      </Panel>
      {overlays}
      {manage && <AgentManageModal open={manage} onClose={() => setManage(false)} def={d} endpoints={endpoints} isAdmin />}
    </>
  )
}

function AgentListRow({ def: d, containers, endpoints, brain, onDuplicate }: { def: AgentDef; containers: AgentContainers | null; endpoints: LlmEndpoint[]; brain?: AgentBrainHealth; onDuplicate: () => void }) {
  const [manage, setManage] = useState(false)
  const { running } = healthOf(d, containers)
  const { onContextMenu, overlays } = useAgentMenu(d, running, () => setManage(true), onDuplicate)
  return (
    <>
      <li onContextMenu={onContextMenu} className={cn('flex items-center gap-3 px-4 py-3 transition-colors hover:bg-hover', !d.enabled && 'opacity-60')}>
        <StatusDot def={d} containers={containers} />
        <Avatar name={d.displayName} className="h-7 w-7" />
        <button type="button" onClick={() => setManage(true)} className="min-w-0 flex-1 text-left">
          <span className="text-sm font-medium text-fg">{d.displayName}</span>
          {d.role && <span className="ml-2 text-xs text-muted">{d.role}</span>}
        </button>
        <BrainChip brain={brain} />
        <Controls def={d} running={running} onManage={() => setManage(true)} onDuplicate={onDuplicate} />
      </li>
      {overlays}
      {manage && <AgentManageModal open={manage} onClose={() => setManage(false)} def={d} endpoints={endpoints} isAdmin />}
    </>
  )
}
