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
import { cn } from '@/lib/cn'
import { createWorkflow, deleteWorkflow, useWorkflows, type TaskWorkflow } from '@/lib/workflows'

// Workflows — which work is which, bound to how it gets done. Match rules
// (boards / labels / keywords) classify tickets; a match rides with the work
// when it's dispatched to an agent: the Hermes skills that define the flow
// plus the toolkits the work expects. The flow content itself lives in the
// skill library the agents mount. Growing area: the Studio (skill authoring,
// gap suggestions, runtime profiles) lands here next.
export const Route = createFileRoute('/_app/workflows')({
  component: WorkflowsPage,
  validateSearch: (search: Record<string, unknown>): { w?: string } => ({
    ...(typeof search.w === 'string' && search.w ? { w: search.w } : {}),
  }),
})

function WorkflowsPage() {
  const qc = useQueryClient()
  const { data: workflows = [], isLoading } = useWorkflows()
  const search = Route.useSearch()
  const navigate = Route.useNavigate()
  const selectedId = search.w ?? null
  const select = (id: string | null) => void navigate({ search: id ? { w: id } : {} })
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
    if (!(await confirm({ title: 'Delete workflow', message: `Delete "${h.name}"? Matching tickets dispatch without its instructions.`, confirmLabel: 'Delete', danger: true }))) return
    await deleteWorkflow(h.id)
    if (selectedId === h.id) select(null)
    await refresh()
  }

  return (
    <div className="h-full overflow-y-auto p-8">
      <div className="mx-auto max-w-5xl space-y-6">
        <div className="flex items-center gap-1.5">
          <h1 className="mercury-text text-2xl font-semibold">Workflows</h1>
          <InfoTip text="Which work is which. Match rules classify tickets; when one is dispatched to an agent, every matching workflow tells it which skills to load and follow, and which toolkits the work expects. The flow content itself lives in the skill library." />
        </div>

        <div className="grid gap-6 lg:grid-cols-[16rem_minmax(0,1fr)]">
          <aside className="space-y-3">
            {isLoading ? (
              <SkeletonRows rows={4} />
            ) : (
              <ul className="space-y-0.5">
                {workflows.map((h) => (
                  <li key={h.id}>
                    <button
                      type="button"
                      onClick={() => select(h.id)}
                      onContextMenu={(e) =>
                        openMenu(e, [
                          { label: 'Open', onSelect: () => select(h.id) },
                          { label: 'Copy link', onSelect: () => copyAppLink(`/workflows?w=${h.id}`) },
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
            )}
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
        </div>
      </div>
      {menu}
    </div>
  )
}
