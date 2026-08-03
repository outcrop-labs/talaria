import { createFileRoute } from '@tanstack/react-router'
import { useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { Plus } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Textarea } from '@/components/ui/textarea'
import { Panel } from '@/components/ui/panel'
import { Markdown } from '@/components/ui/markdown'
import { EmptyState } from '@/components/ui/empty-state'
import { InfoTip } from '@/components/ui/info-tip'
import { SkeletonRows } from '@/components/ui/skeleton'
import { QueryError } from '@/components/ui/query-state'
import { confirm } from '@/components/ui/confirm'
import { useContextMenu, copyAppLink } from '@/components/ui/context-menu'
import { InternalEditorModal } from '@/components/fleet/internal-editor-modal'
import { cn } from '@/lib/cn'
import { relativeTime } from '@/lib/fleet'
import { useSession } from '@/lib/session'
import { createTemplate, deleteTemplate, updateTemplate, useTemplates, type Template, type TemplateKind } from '@/lib/templates'

// Templates — the skeletons work starts from, managed in one place. One tab
// per surface that consumes them (tickets, plans); the body edits in the
// full workspace editor (rich + Muse prompt-editing + version history).
const TEMPLATE_TABS: { id: TemplateKind; label: string; blurb: string }[] = [
  { id: 'ticket', label: 'Tickets', blurb: 'Seeds every ticket description: board defaults, agent bindings, and the QA judge scores against its sections.' },
  { id: 'plan', label: 'Plans', blurb: 'Seeds a new plan’s living document and shapes how the agent rewrites it as the conversation grows.' },
]

export const Route = createFileRoute('/_app/templates')({
  component: TemplatesPage,
  // /templates?tab=plan&t=<id> deep-links a tab + template.
  validateSearch: (search: Record<string, unknown>): { tab?: TemplateKind; t?: string } => ({
    ...(TEMPLATE_TABS.some((v) => v.id === search.tab) ? { tab: search.tab as TemplateKind } : {}),
    ...(typeof search.t === 'string' && search.t ? { t: search.t } : {}),
  }),
})

function TemplatesPage() {
  const { data: session } = useSession()
  const qc = useQueryClient()
  const templatesQuery = useTemplates()
  const { data: templates = [], isLoading } = templatesQuery
  // One read backs BOTH the list and the per-tab counters, and the `= []`
  // default turned its failure into a confident "Tickets 0 / Plans 0" —
  // a count is an assertion, so it must not be printed from a read that failed.
  const failed = templatesQuery.isError && templatesQuery.data === undefined
  const search = Route.useSearch()
  const navigate = Route.useNavigate()
  const tab: TemplateKind = search.tab ?? 'ticket'
  const selectedId = search.t ?? null
  const setTab = (t: TemplateKind) => void navigate({ search: t === 'ticket' ? {} : { tab: t } })
  const select = (id: string | null) => void navigate({ search: { ...(tab !== 'ticket' ? { tab } : {}), ...(id ? { t: id } : {}) } })
  const { openMenu, menu } = useContextMenu()

  const [newName, setNewName] = useState('')
  const [editorOpen, setEditorOpen] = useState(false)

  if (session && session.role !== 'admin') return <EmptyState icon="▣" title="Admins only" />

  const list = templates.filter((t) => t.kind === tab)
  const selected = templates.find((t) => t.id === selectedId && t.kind === tab) ?? null
  const meta = TEMPLATE_TABS.find((t) => t.id === tab)!

  const refresh = () => qc.invalidateQueries({ queryKey: ['templates'] })
  const create = async () => {
    const name = newName.trim()
    if (!name) return
    const { template } = await createTemplate({ name, kind: tab })
    setNewName('')
    await refresh()
    if (template) select(template.id)
  }
  const remove = async (t: Template) => {
    if (!(await confirm({ title: 'Delete template', message: `Delete "${t.name}"? Boards and agents bound to it fall back down the template chain.`, confirmLabel: 'Delete', danger: true }))) return
    await deleteTemplate(t.id)
    if (selectedId === t.id) select(null)
    await refresh()
  }

  return (
    <div className="h-full overflow-y-auto p-8">
      <div className="mx-auto max-w-5xl space-y-6">
        <div className="flex items-center gap-1.5">
          <h1 className="mercury-text text-2xl font-semibold">Templates</h1>
          <InfoTip text="The markdown skeletons work starts from. Resolution order everywhere: explicit pick → agent binding → board default → freeform." />
        </div>

        <div className="flex gap-1 border-b border-line-subtle">
          {TEMPLATE_TABS.map((t) => (
            <button
              key={t.id}
              type="button"
              onClick={() => setTab(t.id)}
              className={cn('relative flex items-center gap-1.5 px-3 py-2 text-sm transition-colors', tab === t.id ? 'text-fg' : 'text-muted hover:text-fg')}
            >
              {t.label}
              {/* An em dash where a number would be: unknown, not zero. */}
              <span className="text-[10px] text-muted">
                {isLoading || failed ? '—' : templates.filter((x) => x.kind === t.id).length}
              </span>
              {tab === t.id && <span className="absolute inset-x-2 -bottom-px h-0.5 rounded-full bg-accent" />}
            </button>
          ))}
        </div>

        <div className="grid gap-6 lg:grid-cols-[16rem_minmax(0,1fr)]">
          <aside className="space-y-3">
            {isLoading ? (
              <SkeletonRows rows={4} />
            ) : failed ? (
              <QueryError
                variant="compact"
                error={templatesQuery.error}
                title="Could not load templates"
                onRetry={() => void templatesQuery.refetch()}
              />
            ) : (
              <ul className="space-y-0.5">
                {list.map((t) => (
                  <li key={t.id}>
                    <button
                      type="button"
                      onClick={() => select(t.id)}
                      onContextMenu={(e) =>
                        openMenu(e, [
                          { label: 'Open', onSelect: () => select(t.id) },
                          { label: 'Copy link', onSelect: () => copyAppLink(`/templates?tab=${t.kind}&t=${t.id}`) },
                          'sep',
                          { label: 'Delete', danger: true, onSelect: () => void remove(t) },
                        ])
                      }
                      className={cn(
                        'flex w-full items-center gap-2 rounded-lg px-2.5 py-2 text-left text-sm transition-colors',
                        selected?.id === t.id ? 'bg-card text-fg' : 'text-muted hover:bg-card hover:text-fg',
                      )}
                    >
                      <span className="min-w-0 flex-1 truncate font-sans">{t.name}</span>
                    </button>
                  </li>
                ))}
                {list.length === 0 && <li className="px-2.5 py-2 text-xs text-muted">None yet.</li>}
              </ul>
            )}
            <div className="flex items-center gap-1.5 border-t border-line-subtle pt-3">
              <Input size="sm" value={newName} onChange={(e) => setNewName(e.target.value)} placeholder={`New ${tab} template`} onKeyDown={(e) => e.key === 'Enter' && void create()} />
              <Button size="sm" variant="outline" disabled={!newName.trim()} onClick={() => void create()}>
                <Plus size={14} />
              </Button>
            </div>
          </aside>

          {selected ? (
            <TemplateDetail key={selected.id} template={selected} blurb={meta.blurb} onChanged={refresh} onDelete={() => void remove(selected)} editorOpen={editorOpen} setEditorOpen={setEditorOpen} />
          ) : (
            <Panel>
              <EmptyState
                icon="▣"
                title={`No ${tab} template selected`}
                hint={
                  // "Create the first one" is the same zero-claim in prose.
                  failed
                    ? 'The template list could not be loaded — retry on the left.'
                    : list.length
                      ? 'Pick one on the left, or create a new one.'
                      : 'Create the first one on the left.'
                }
              />
            </Panel>
          )}
        </div>
      </div>
      {menu}
    </div>
  )
}

function TemplateDetail({
  template,
  blurb,
  onChanged,
  onDelete,
  editorOpen,
  setEditorOpen,
}: {
  template: Template
  blurb: string
  onChanged: () => void
  onDelete: () => void
  editorOpen: boolean
  setEditorOpen: (v: boolean) => void
}) {
  const [name, setName] = useState(template.name)
  const [guidance, setGuidance] = useState(template.guidance)
  const [busy, setBusy] = useState(false)

  const save = async (patch: { name?: string; guidance?: string; body?: string }) => {
    setBusy(true)
    try {
      await updateTemplate(template.id, patch)
      onChanged()
    } finally {
      setBusy(false)
    }
  }

  return (
    <Panel>
      <div className="mb-4 flex items-center gap-2">
        <Input size="sm" value={name} onChange={(e) => setName(e.target.value)} onBlur={() => name.trim() && name !== template.name && void save({ name: name.trim() })} className="max-w-xs font-sans" />
        <span className="rounded border border-line-subtle px-1.5 py-0.5 text-[10px] uppercase tracking-wide text-muted">{template.kind}</span>
        <span className="ml-auto text-xs text-muted">{relativeTime(template.updatedAt)}</span>
      </div>

      <div className="mb-4">
        <div className="mb-1 flex items-center gap-1.5">
          <span className="text-[11px] uppercase tracking-wide text-muted">Skeleton</span>
          <InfoTip text={blurb} />
          <Button size="sm" variant="outline" className="ml-auto" onClick={() => setEditorOpen(true)}>
            Edit
          </Button>
        </div>
        {template.body.trim() ? (
          <div className="max-h-80 overflow-y-auto rounded-xl border border-line-subtle bg-card px-4 py-3 text-sm">
            <Markdown className="tiptap">{template.body}</Markdown>
          </div>
        ) : (
          <button type="button" onClick={() => setEditorOpen(true)} className="w-full rounded-xl border border-dashed border-line-subtle px-4 py-6 text-center text-xs text-muted transition-colors hover:border-line hover:text-fg">
            Empty. Open the editor — or let Muse draft it from a description.
          </button>
        )}
      </div>

      <div>
        <div className="mb-1 flex items-center gap-1.5">
          <span className="text-[11px] uppercase tracking-wide text-muted">Agent guidance</span>
          <InfoTip text="Prompt-only: travels with the template into the model's instructions but is never shown on the ticket or plan itself." />
        </div>
        <Textarea autoGrow rows={2} value={guidance} onChange={(e) => setGuidance(e.target.value)} onBlur={() => guidance !== template.guidance && void save({ guidance })} className="max-h-40 text-xs" placeholder='e.g. "Always fill acceptance criteria; keep Out of scope honest."' />
      </div>

      {editorOpen && (
        <InternalEditorModal
          open
          onClose={() => setEditorOpen(false)}
          title={`${template.name} · skeleton`}
          subtitle="The sections every one of these keeps. Saves are versioned; Muse drafts from a description."
          value={template.body}
          editable
          saving={busy}
          onSave={(md) => void save({ body: md })}
          history={{ kind: 'template', id: template.id }}
          muse={{ kind: 'template', context: `A ${template.kind} template named "${template.name}" for a team workspace.` }}
          footerExtra={
            <Button variant="ghost" size="sm" onClick={onDelete}>
              Delete template
            </Button>
          }
        />
      )}
    </Panel>
  )
}
