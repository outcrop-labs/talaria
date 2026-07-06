import { createFileRoute } from '@tanstack/react-router'
import { useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { Play, Square, SlidersHorizontal, Archive, ArrowRightLeft, LayoutGrid, List, Loader2 } from 'lucide-react'
import { Panel } from '@/components/ui/panel'
import { Avatar } from '@/components/ui/avatar'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Modal } from '@/components/ui/modal'
import { useFleet } from '@/lib/fleet'
import { useSession } from '@/lib/session'
import { cn } from '@/lib/cn'
import { AgentManageModal } from '@/components/fleet/agent-manage-modal'
import { CreateAgentModal } from '@/components/fleet/create-agent-modal'
import {
  controlAgent,
  importFleet,
  useFleetContainers,
  useFleetDefs,
  type AgentContainers,
  type AgentDef,
  type FleetAction,
  type LlmEndpoint,
} from '@/lib/fleet-defs'

export const Route = createFileRoute('/_app/agents')({
  component: AgentsPage,
})

// ── Health: one word (up / degraded / down / retired) from container reality ──
type Health = 'up' | 'degraded' | 'down' | 'retired' | 'legacy'
const HEALTH_COLOR: Record<Health, string> = {
  up: 'var(--theme-success)',
  degraded: 'var(--theme-warning)',
  down: 'var(--theme-danger)',
  retired: 'var(--theme-line)',
  legacy: 'var(--theme-accent)',
}
function healthOf(d: AgentDef, c: AgentContainers | null): { health: Health; running: boolean } {
  if (!d.enabled) return { health: 'retired', running: false }
  const state = d.managed ? c?.managed ?? null : c?.legacy ?? null
  const running = state?.state === 'running'
  if (!d.managed) return { health: 'legacy', running }
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
  const qc = useQueryClient()
  const defs = defsData?.defs ?? []
  const endpoints = defsData?.endpoints ?? []
  const t = fleet?.totals

  const [view, setView] = useState<'grid' | 'list'>('grid')
  const [creating, setCreating] = useState(false)
  const [importing, setImporting] = useState(false)
  const [summary, setSummary] = useState<string | null>(null)

  const runImport = async () => {
    setImporting(true)
    setSummary(null)
    try {
      const r = await importFleet()
      if (!r) return setSummary('import failed')
      const created = r.agents.filter((a) => a.created).length
      setSummary(`${r.agents.length} scanned · ${created} new version${created === 1 ? '' : 's'}${r.errors.length ? ` · ${r.errors.length} errors` : ''}`)
      await qc.invalidateQueries({ queryKey: ['fleet-defs'] })
    } finally {
      setImporting(false)
    }
  }

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
                {defs.length > 0 && (
                  <Button size="sm" onClick={() => setCreating(true)}>
                    New agent
                  </Button>
                )}
                <Button variant="outline" size="sm" onClick={() => void runImport()} disabled={importing}>
                  {importing ? 'Importing…' : defs.length ? 'Re-import' : 'Import'}
                </Button>
              </>
            )}
          </div>
        </div>
        {summary && <div className="text-xs text-muted">{summary}</div>}

        {!isAdmin ? (
          <ReadOnlyRoster fleet={fleet?.agents ?? []} view={view} />
        ) : defs.length === 0 ? (
          <Panel>
            <div className="py-6 text-center text-sm text-muted">
              Nothing imported yet — pull in a Hermes stack to seed the fleet.
            </div>
          </Panel>
        ) : view === 'grid' ? (
          <div className="grid grid-cols-2 gap-3 sm:grid-cols-3">
            {defs.map((d) => (
              <AgentTile key={d.id} def={d} containers={byDept.get(d.department) ?? null} endpoints={endpoints} />
            ))}
          </div>
        ) : (
          <Panel className="p-0">
            <ul className="divide-y divide-line-subtle">
              {defs.map((d) => (
                <AgentListRow key={d.id} def={d} containers={byDept.get(d.department) ?? null} endpoints={endpoints} />
              ))}
            </ul>
          </Panel>
        )}

        {creating && <CreateAgentModal open={creating} onClose={() => setCreating(false)} templates={defs.filter((d) => d.enabled)} />}
      </div>
    </div>
  )
}

// Non-admins get a read-only glance (name + status) — no controls, no internals.
function ReadOnlyRoster({ fleet, view }: { fleet: Array<{ id: string; label: string; role: string; status: string }>; view: 'grid' | 'list' }) {
  const dot = (status: string) => (status === 'offline' ? 'var(--theme-line)' : status === 'error' ? 'var(--theme-danger)' : 'var(--theme-success)')
  if (view === 'list') {
    return (
      <Panel className="p-0">
        <ul className="divide-y divide-line-subtle">
          {fleet.map((a) => (
            <li key={a.id} className="flex items-center gap-3 px-4 py-3">
              <Avatar name={a.label} className="h-7 w-7" />
              <span className="text-sm text-fg">{a.label}</span>
              <span className="text-xs text-muted">{a.role}</span>
              <span className="ml-auto h-2 w-2 rounded-full" style={{ background: dot(a.status) }} title={a.status} />
            </li>
          ))}
        </ul>
      </Panel>
    )
  }
  return (
    <div className="grid grid-cols-2 gap-3 sm:grid-cols-3">
      {fleet.map((a) => (
        <Panel key={a.id} className="flex items-center gap-3">
          <Avatar name={a.label} className="h-9 w-9" />
          <div className="min-w-0 flex-1">
            <div className="truncate text-sm font-medium text-fg">{a.label}</div>
            <div className="truncate text-xs text-muted">{a.role}</div>
          </div>
          <span className="h-2.5 w-2.5 shrink-0 rounded-full" style={{ background: dot(a.status) }} title={a.status} />
        </Panel>
      ))}
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

// Shared control logic for a managed/legacy agent def.
function useAgentControls(d: AgentDef) {
  const qc = useQueryClient()
  const [pending, setPending] = useState<string | null>(null)
  const act = async (action: FleetAction, label: string, confirmMsg?: string) => {
    if (confirmMsg && !confirm(confirmMsg)) return
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

/** The control-icon cluster (start/stop · manage · retire/migrate). */
function Controls({ def: d, running, onManage }: { def: AgentDef; running: boolean; onManage: () => void }) {
  const { pending, act } = useAgentControls(d)
  const [retiring, setRetiring] = useState(false)
  if (!d.enabled) return <span className="text-xs text-muted">retired</span>
  if (pending) return <Loader2 size={15} className="animate-spin text-muted" />
  return (
    <div className="flex items-center">
      <IconBtn icon={<SlidersHorizontal size={15} />} title="Manage" onClick={onManage} />
      {d.managed ? (
        <>
          {running ? (
            <IconBtn icon={<Square size={15} />} title="Stop" onClick={() => void act('stop', 'stopping')} />
          ) : (
            <IconBtn icon={<Play size={15} />} title="Start" onClick={() => void act('up', 'starting')} />
          )}
          <IconBtn icon={<Archive size={15} />} title="Retire" danger onClick={() => setRetiring(true)} />
          {retiring && <RetireModal def={d} onClose={() => setRetiring(false)} onConfirm={() => void act('retire', 'retiring')} />}
        </>
      ) : (
        <IconBtn
          icon={<ArrowRightLeft size={15} />}
          title="Migrate to Talaria"
          onClick={() => void act('migrate', 'migrating', `Migrate ${d.displayName} to Talaria management? The legacy container stops; state carries over.`)}
        />
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

function AgentTile({ def: d, containers, endpoints }: { def: AgentDef; containers: AgentContainers | null; endpoints: LlmEndpoint[] }) {
  const [manage, setManage] = useState(false)
  const { running } = healthOf(d, containers)
  return (
    <>
      <Panel className="flex flex-col gap-3">
        <div className="flex items-center gap-2.5">
          <Avatar name={d.displayName} className="h-9 w-9" />
          <button type="button" onClick={() => setManage(true)} className="min-w-0 flex-1 text-left">
            <div className="truncate text-sm font-medium text-fg">{d.displayName}</div>
            <div className="truncate text-xs text-muted">v{d.currentVersion}</div>
          </button>
          <StatusDot def={d} containers={containers} />
        </div>
        <div className="flex justify-end">
          <Controls def={d} running={running} onManage={() => setManage(true)} />
        </div>
      </Panel>
      {manage && <AgentManageModal open={manage} onClose={() => setManage(false)} def={d} endpoints={endpoints} isAdmin />}
    </>
  )
}

function AgentListRow({ def: d, containers, endpoints }: { def: AgentDef; containers: AgentContainers | null; endpoints: LlmEndpoint[] }) {
  const [manage, setManage] = useState(false)
  const { running } = healthOf(d, containers)
  return (
    <>
      <li className="flex items-center gap-3 px-4 py-3">
        <StatusDot def={d} containers={containers} />
        <Avatar name={d.displayName} className="h-7 w-7" />
        <button type="button" onClick={() => setManage(true)} className="min-w-0 flex-1 text-left">
          <span className="text-sm font-medium text-fg">{d.displayName}</span>
          <span className="ml-2 text-xs text-muted">v{d.currentVersion}</span>
        </button>
        <Controls def={d} running={running} onManage={() => setManage(true)} />
      </li>
      {manage && <AgentManageModal open={manage} onClose={() => setManage(false)} def={d} endpoints={endpoints} isAdmin />}
    </>
  )
}
