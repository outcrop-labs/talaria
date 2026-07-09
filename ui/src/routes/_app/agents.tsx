import { createFileRoute } from '@tanstack/react-router'
import { useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { Play, Square, SlidersHorizontal, Archive, CalendarClock, Import, LayoutGrid, List, Loader2, Copy, UserPlus, Plus } from 'lucide-react'
import { Panel } from '@/components/ui/panel'
import { confirm } from '@/components/ui/confirm'
import { Avatar } from '@/components/ui/avatar'
import { Button } from '@/components/ui/button'
import { EmptyState } from '@/components/ui/empty-state'
import { Input } from '@/components/ui/input'
import { Modal } from '@/components/ui/modal'
import { useFleet } from '@/lib/fleet'
import { useSession } from '@/lib/session'
import { cn } from '@/lib/cn'
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

// ── Health: one word (up / degraded / down / retired) from container reality ──
type Health = 'up' | 'degraded' | 'down' | 'retired'
const HEALTH_COLOR: Record<Health, string> = {
  up: 'var(--theme-success)',
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
  return (
    <span
      title={detail}
      className={cn(
        'shrink-0 rounded-full px-1.5 py-0.5 text-[10px] font-semibold',
        !brain.ok
          ? 'bg-[color:var(--theme-danger)]/15 text-[color:var(--theme-danger)]'
          : 'bg-[color:var(--theme-warning)]/15 text-[color:var(--theme-warning)]',
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
  return { health: /unhealthy/i.test(state?.status ?? '') ? 'degraded' : 'up', running: true }
}

function AgentsPage() {
  const { data: session } = useSession()
  const isAdmin = session?.role === 'admin'
  const { data: fleet } = useFleet()
  const { data: defsData } = useFleetDefs(isAdmin)
  const { data: containers = [] } = useFleetContainers(isAdmin)
  const byDept = new Map(containers.map((c) => [c.department, c]))
  const defs = defsData?.defs ?? []
  const endpoints = defsData?.endpoints ?? []
  const brainByAgent = new Map((defsData?.brains ?? []).map((b) => [b.agent, b]))
  const t = fleet?.totals

  const [view, setView] = useState<'grid' | 'list'>('grid')
  const [creating, setCreating] = useState(false)
  const [duplicateFrom, setDuplicateFrom] = useState<AgentDef | null>(null)
  const [schedulesOpen, setSchedulesOpen] = useState(false)
  const [federateOpen, setFederateOpen] = useState(false)

  return (
    <div className="h-full overflow-y-auto p-8">
      <div className="mx-auto max-w-5xl space-y-6">
        <div className="flex flex-wrap items-center gap-3">
          <h1 className="mercury-text text-2xl font-semibold">Agents</h1>
          {t && <span className="text-sm text-muted">{t.online}/{t.agents} online · {t.activeToday} active today</span>}
          <div className="ml-auto flex items-center gap-2">
            {/* Grid / list toggle */}
            <div className="flex rounded-lg border border-line-subtle p-0.5">
              <button type="button" onClick={() => setView('grid')} className={cn('rounded-md p-1.5', view === 'grid' ? 'bg-card text-fg' : 'text-muted')} title="Grid">
                <LayoutGrid size={15} />
              </button>
              <button type="button" onClick={() => setView('list')} className={cn('rounded-md p-1.5', view === 'list' ? 'bg-card text-fg' : 'text-muted')} title="List">
                <List size={15} />
              </button>
            </div>
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
                  title="Schedules — crons across the fleet"
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

        {defs.length === 0 ? (
          <Panel>
            <EmptyState
              title="No agents yet"
              hint="Describe the first one and Muse designs it — identity, soul, and starter skills."
              action={
                <Button size="sm" onClick={() => setCreating(true)}>
                  Design your first agent
                </Button>
              }
            />
          </Panel>
        ) : view === 'grid' ? (
          <div className="grid grid-cols-2 gap-3 sm:grid-cols-3">
            {defs.map((d) => (
              <AgentTile key={d.id} def={d} containers={byDept.get(d.department) ?? null} endpoints={endpoints} brain={brainByAgent.get(d.model)} onDuplicate={() => setDuplicateFrom(d)} />
            ))}
          </div>
        ) : (
          <Panel className="p-0">
            <ul className="divide-y divide-line-subtle">
              {defs.map((d) => (
                <AgentListRow key={d.id} def={d} containers={byDept.get(d.department) ?? null} endpoints={endpoints} brain={brainByAgent.get(d.model)} onDuplicate={() => setDuplicateFrom(d)} />
              ))}
            </ul>
          </Panel>
        )}

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
        'grid h-8 w-8 place-items-center rounded-lg text-muted transition-colors hover:bg-card disabled:opacity-40',
        danger ? 'hover:text-[color:var(--theme-danger)]' : 'hover:text-fg',
      )}
    >
      {icon}
    </button>
  )
}

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

/** The control-icon cluster (start/stop · manage · retire · re-hire). */
function Controls({ def: d, running, onManage, onDuplicate }: { def: AgentDef; running: boolean; onManage: () => void; onDuplicate: () => void }) {
  const { pending, act } = useAgentControls(d)
  const [retiring, setRetiring] = useState(false)
  if (pending) return <Loader2 size={15} className="animate-spin text-muted" />
  // Retired agents: re-hire (re-enable + start) or duplicate as a template.
  if (!d.enabled)
    return (
      <div className="flex items-center">
        <IconBtn icon={<Copy size={15} />} title="Duplicate to a new agent" onClick={onDuplicate} />
        <IconBtn icon={<UserPlus size={15} />} title="Re-hire" onClick={() => void act('unretire', 're-hiring')} />
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
      <span aria-hidden className="mx-1.5 h-4 w-px bg-line-subtle" />
      {running ? (
        <IconBtn icon={<Square size={14} fill="currentColor" />} title="Stop" onClick={() => void act('stop', 'stopping')} />
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
        <div className="flex justify-end gap-2 border-t border-line-subtle pt-3">
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

function StatusDot({ def: d, containers }: { def: AgentDef; containers: AgentContainers | null }) {
  const { health } = healthOf(d, containers)
  return <span className="h-2.5 w-2.5 shrink-0 rounded-full" style={{ background: HEALTH_COLOR[health] }} title={health} />
}

function AgentTile({ def: d, containers, endpoints, brain, onDuplicate }: { def: AgentDef; containers: AgentContainers | null; endpoints: LlmEndpoint[]; brain?: AgentBrainHealth; onDuplicate: () => void }) {
  const [manage, setManage] = useState(false)
  const { running } = healthOf(d, containers)
  return (
    <>
      <Panel className={cn('flex flex-col gap-3', !d.enabled && 'opacity-60')}>
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
      {manage && <AgentManageModal open={manage} onClose={() => setManage(false)} def={d} endpoints={endpoints} isAdmin />}
    </>
  )
}

function AgentListRow({ def: d, containers, endpoints, brain, onDuplicate }: { def: AgentDef; containers: AgentContainers | null; endpoints: LlmEndpoint[]; brain?: AgentBrainHealth; onDuplicate: () => void }) {
  const [manage, setManage] = useState(false)
  const { running } = healthOf(d, containers)
  return (
    <>
      <li className={cn('flex items-center gap-3 px-4 py-3', !d.enabled && 'opacity-60')}>
        <StatusDot def={d} containers={containers} />
        <Avatar name={d.displayName} className="h-7 w-7" />
        <button type="button" onClick={() => setManage(true)} className="min-w-0 flex-1 text-left">
          <span className="text-sm font-medium text-fg">{d.displayName}</span>
          {d.role && <span className="ml-2 text-xs text-muted">{d.role}</span>}
        </button>
        <BrainChip brain={brain} />
        <Controls def={d} running={running} onManage={() => setManage(true)} onDuplicate={onDuplicate} />
      </li>
      {manage && <AgentManageModal open={manage} onClose={() => setManage(false)} def={d} endpoints={endpoints} isAdmin />}
    </>
  )
}
