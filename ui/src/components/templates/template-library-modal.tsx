import { useEffect, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Modal } from '@/components/ui/modal'
import { RichEditor } from '@/components/ui/rich-editor'
import { Textarea } from '@/components/ui/textarea'
import { confirm } from '@/components/ui/confirm'
import {
  createTemplate,
  deleteTemplate,
  updateTemplate,
  useTemplates,
  type Template,
  type TemplateKind,
} from '@/lib/templates'

// The org's template library: markdown skeletons + agent guidance for tickets
// and plan documents. Boards bind ticket templates (default per board); agents
// may carry overrides. Managed here; consumed everywhere tickets/plans form.
export function TemplateLibraryModal({ open, onClose }: { open: boolean; onClose: () => void }) {
  const qc = useQueryClient()
  const { data: templates = [] } = useTemplates()
  const [selectedId, setSelectedId] = useState<string | null>(null)
  const selected = templates.find((t) => t.id === selectedId) ?? null

  const refresh = () => qc.invalidateQueries({ queryKey: ['templates'] })

  const add = async (kind: TemplateKind) => {
    const { template } = await createTemplate({
      name: kind === 'ticket' ? 'New ticket template' : 'New plan template',
      kind,
      body: kind === 'ticket' ? TICKET_STARTER : PLAN_STARTER,
    })
    await refresh()
    setSelectedId(template.id)
  }

  const remove = async (t: Template) => {
    if (!(await confirm({ title: 'Delete template', message: `Delete "${t.name}"? Boards and agents bound to it fall back to freeform.`, confirmLabel: 'Delete', danger: true }))) return
    await deleteTemplate(t.id)
    if (selectedId === t.id) setSelectedId(null)
    await refresh()
  }

  const byKind = (kind: TemplateKind) => templates.filter((t) => t.kind === kind)

  return (
    <Modal open={open} onClose={onClose} title="Template library" takeover>
      <div className="flex min-h-[26rem] gap-4">
        <div className="w-56 shrink-0 space-y-4 overflow-y-auto border-r border-line-subtle pr-3">
          {(['ticket', 'plan'] as const).map((kind) => (
            <div key={kind}>
              <div className="mb-1 flex items-center">
                <span className="text-[11px] uppercase tracking-wide text-muted">{kind} templates</span>
                <button type="button" className="ml-auto text-xs text-accent hover:underline" onClick={() => void add(kind)}>
                  + new
                </button>
              </div>
              {byKind(kind).length === 0 && <div className="text-xs text-muted">None yet.</div>}
              {byKind(kind).map((t) => (
                <button
                  key={t.id}
                  type="button"
                  onClick={() => setSelectedId(t.id)}
                  className={`block w-full truncate rounded-lg px-2 py-1.5 text-left text-sm transition-colors ${
                    t.id === selectedId ? 'bg-card text-fg' : 'text-muted hover:text-fg'
                  }`}
                >
                  {t.name}
                </button>
              ))}
            </div>
          ))}
        </div>

        <div className="min-w-0 flex-1">
          {selected ? (
            <TemplateEditor key={selected.id} template={selected} onDelete={() => void remove(selected)} onSaved={() => void refresh()} />
          ) : (
            <div className="grid h-full place-items-center text-sm text-muted">
              Pick a template to edit, or create one. The skeleton's headings are the schema; guidance tells agents how to fill it.
            </div>
          )}
        </div>
      </div>
      <div className="mt-4 flex justify-end border-t border-line-subtle pt-3">
        <Button size="sm" onClick={onClose}>
          Done
        </Button>
      </div>
    </Modal>
  )
}

function TemplateEditor({ template, onDelete, onSaved }: { template: Template; onDelete: () => void; onSaved: () => void }) {
  const [name, setName] = useState(template.name)
  const [body, setBody] = useState(template.body)
  const [guidance, setGuidance] = useState(template.guidance)
  const [saved, setSaved] = useState(false)
  const dirty = name !== template.name || body !== template.body || guidance !== template.guidance

  useEffect(() => {
    setSaved(false)
  }, [name, body, guidance])

  const save = async () => {
    await updateTemplate(template.id, { name: name.trim() || template.name, body, guidance })
    onSaved()
    setSaved(true)
  }

  return (
    <div className="flex h-full flex-col gap-3">
      <div className="flex items-center gap-2">
        <Input size="sm" value={name} onChange={(e) => setName(e.target.value)} className="flex-1" />
        <span className="shrink-0 rounded border border-line-subtle px-1.5 py-0.5 text-[10px] uppercase tracking-wide text-muted">
          {template.kind}
        </span>
      </div>
      <div className="min-h-0 flex-1">
        <label className="mb-1 block text-[11px] uppercase tracking-wide text-muted">Skeleton (the sections every {template.kind} keeps)</label>
        {/* autosave keeps `body` (and the dirty flag) fresh while typing;
            key remounts when switching templates. */}
        <div className="h-[16rem] overflow-y-auto">
          <RichEditor key={template.id} value={body} onSave={setBody} autosave minHeight="15rem" placeholder="## Sections every one of these keeps…" />
        </div>
      </div>
      <div>
        <label className="mb-1 block text-[11px] uppercase tracking-wide text-muted">Agent guidance (prompt-only, never shown on the {template.kind})</label>
        <Textarea value={guidance} onChange={(e) => setGuidance(e.target.value)} rows={3} className="w-full text-xs" placeholder='e.g. "Always fill acceptance criteria; link the plan doc; keep Out of scope honest."' />
      </div>
      <div className="flex items-center gap-2">
        <Button variant="ghost" size="sm" onClick={onDelete}>
          Delete
        </Button>
        <span className="ml-auto" />
        {saved && !dirty && <span className="text-xs text-[color:var(--theme-success)]">Saved</span>}
        <Button size="sm" onClick={() => void save()} disabled={!dirty}>
          Save
        </Button>
      </div>
    </div>
  )
}

const TICKET_STARTER = `## Problem
<what's broken or needed, and the user impact>

## Proposal
<the approach — call out decisions>

## Acceptance criteria
- [ ] <verifiable outcome>

## Out of scope
<explicitly not this ticket>
`

const PLAN_STARTER = `# <Plan title>

## Goal
<what done looks like>

## Scope
<in / out>

## Decisions
<agreed choices and why>

## Open questions
<unresolved items>

## Next steps
<ordered, actionable>
`
