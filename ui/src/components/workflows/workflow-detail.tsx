// Workflow editor — the detail pane of /workflows. A workflow classifies
// tickets by match rules (boards / labels / keywords) and delivers
// instructions + declared toolkits with the dispatched work.
import { useState } from 'react'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Textarea } from '@/components/ui/textarea'
import { Panel } from '@/components/ui/panel'
import { Chip } from '@/components/ui/chip'
import { Toggle } from '@/components/ui/checkbox'
import { InfoTip } from '@/components/ui/info-tip'
import { listQuery } from '@/components/ui/query-state'
import { useBoards } from '@/lib/boards'
import { updateWorkflow, useSkillLibrary, type TaskWorkflow } from '@/lib/workflows'

/** Free-entry token list: type, Enter adds, ✕ removes. */
function TokenInput({ value, onChange, placeholder }: { value: string[]; onChange: (v: string[]) => void; placeholder: string }) {
  const [draft, setDraft] = useState('')
  const add = () => {
    const t = draft.trim()
    if (!t || value.includes(t)) return setDraft('')
    onChange([...value, t])
    setDraft('')
  }
  return (
    <div className="flex flex-wrap items-center gap-1">
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
        className="w-40"
      />
    </div>
  )
}

export function WorkflowDetail({ workflow, onChanged, onDelete }: { workflow: TaskWorkflow; onChanged: () => void; onDelete: () => void }) {
  // Both of these fed "there are none" copy. The skill one was worse than that:
  // with an empty library every skill this workflow is bound to falls out of
  // `known` below and gets drawn as "Bound but not in the library" — a warning
  // chip offering to unbind a skill that is perfectly fine.
  const boardsList = listQuery(useBoards(), { title: 'Could not load your boards', variant: 'inline' })
  const libraryList = listQuery(useSkillLibrary(), { title: 'Could not load the skill library', variant: 'inline' })
  const boards = boardsList.rows
  const rawSkillOwners = libraryList.rows
  // Platform plumbing skills aren't bindable flow content — hide them here too.
  const skillOwners = rawSkillOwners.map((o) => ({ ...o, skills: o.skills.filter((sk) => !sk.platform) }))
  const [name, setName] = useState(workflow.name)
  const [description, setDescription] = useState(workflow.description)
  const [toolkitsText, setToolkitsText] = useState(
    workflow.toolkits.map((t) => (t.tools?.length ? `${t.server}: ${t.tools.join(', ')}` : t.server)).join('\n'),
  )

  const save = async (patch: Parameters<typeof updateWorkflow>[1]) => {
    await updateWorkflow(workflow.id, patch)
    onChanged()
  }
  const saveMatch = (m: Partial<TaskWorkflow['match']>) => void save({ match: { ...workflow.match, ...m } })
  const saveToolkits = () => {
    const toolkits = toolkitsText
      .split('\n')
      .map((l) => l.trim())
      .filter(Boolean)
      .map((l) => {
        const [server, rest] = l.split(':', 2) as [string, string?]
        const tools = rest
          ?.split(',')
          .map((t) => t.trim())
          .filter(Boolean)
        return { server: server.trim(), ...(tools?.length ? { tools } : {}) }
      })
    void save({ toolkits })
  }

  return (
    <Panel>
      <div className="mb-4 flex items-center gap-2">
        <Input size="sm" value={name} onChange={(e) => setName(e.target.value)} onBlur={() => name.trim() && name !== workflow.name && void save({ name: name.trim() })} className="max-w-xs font-sans" />
        <Toggle checked={workflow.enabled} onChange={(v) => void save({ enabled: v })} label={workflow.enabled ? 'Enabled' : 'Disabled'} />
        <Button variant="ghost" size="sm" className="ml-auto" onClick={onDelete}>
          Delete
        </Button>
      </div>

      <div className="mb-4">
        <Input size="sm" value={description} onChange={(e) => setDescription(e.target.value)} onBlur={() => description !== workflow.description && void save({ description })} placeholder="One-line description (shown to admins, not agents)" />
      </div>

      <div className="mb-4 space-y-3 rounded-xl border border-line-subtle bg-card/40 px-4 py-3">
        <div className="flex items-center gap-1.5">
          <span className="text-[11px] uppercase tracking-wide text-muted">Matches when</span>
          <InfoTip text="OR within a rule, AND across the rules you set. A workflow with no rules matches nothing." />
        </div>
        <div className="space-y-1">
          <span className="text-xs text-muted">Boards</span>
          <div className="flex flex-wrap gap-1">
            {boards.map((b) => {
              const on = workflow.match.boards?.includes(b.id) ?? false
              return (
                <Chip
                  key={b.id}
                  selected={on}
                  onSelect={() =>
                    saveMatch({ boards: on ? (workflow.match.boards ?? []).filter((x) => x !== b.id) : [...(workflow.match.boards ?? []), b.id] })
                  }
                >
                  {b.name}
                </Chip>
              )
            })}
            {boards.length === 0 && !boardsList.failed && !boardsList.pending && (
              <span className="text-xs text-muted">No boards yet.</span>
            )}
          </div>
          {boardsList.notice}
        </div>
        <div className="space-y-1">
          <span className="text-xs text-muted">Labels</span>
          <TokenInput value={workflow.match.labels ?? []} onChange={(labels) => saveMatch({ labels })} placeholder="Add label…" />
        </div>
        <div className="space-y-1">
          <span className="text-xs text-muted">Title/description keywords</span>
          <TokenInput value={workflow.match.keywords ?? []} onChange={(keywords) => saveMatch({ keywords })} placeholder="Add keyword…" />
        </div>
      </div>

      <div className="mb-4">
        <div className="mb-1 flex items-center gap-1.5">
          <span className="text-[11px] uppercase tracking-wide text-muted">Skills</span>
          <InfoTip text="The flow lives in Hermes skills — the same library the agents already mount, edited on the agent view. The workflow only names which ones this kind of work follows; dispatch tells the agent to load them." />
        </div>
        <div className="space-y-2 rounded-xl border border-line-subtle bg-card/40 px-4 py-3">
          {skillOwners.map((o) =>
            o.skills.length ? (
              <div key={o.owner} className="space-y-1">
                <span className="text-xs text-muted">{o.label}</span>
                <div className="flex flex-wrap gap-1">
                  {o.skills.map((sk) => {
                    const on = workflow.skills.includes(sk.name)
                    return (
                      <Chip
                        key={`${o.owner}/${sk.name}`}
                        title={sk.description}
                        selected={on}
                        onSelect={() =>
                          void save({ skills: on ? workflow.skills.filter((x) => x !== sk.name) : [...workflow.skills, sk.name] })
                        }
                      >
                        {sk.name}
                      </Chip>
                    )
                  })}
                </div>
              </div>
            ) : null,
          )}
          {(() => {
            // Only meaningful when we actually READ the library. Without this
            // guard a failed read reports every bound skill as an orphan.
            if (libraryList.failed || libraryList.pending) return null
            const known = new Set(skillOwners.flatMap((o) => o.skills.map((sk) => sk.name)))
            const orphans = workflow.skills.filter((sk) => !known.has(sk))
            return orphans.length ? (
              <div className="space-y-1">
                <span className="text-xs text-[color:var(--theme-warning)]">Bound but not in the library</span>
                <div className="flex flex-wrap gap-1">
                  {orphans.map((sk) => (
                    <Chip key={sk} tone="warn" onRemove={() => void save({ skills: workflow.skills.filter((x) => x !== sk) })}>
                      {sk}
                    </Chip>
                  ))}
                </div>
              </div>
            ) : null
          })()}
          {libraryList.notice}
          {!libraryList.failed && !libraryList.pending && skillOwners.every((o) => !o.skills.length) && !workflow.skills.length && (
            <span className="text-xs text-muted">No skills in the library yet — create them on an agent's manage view.</span>
          )}
        </div>
      </div>

      <div>
        <div className="mb-1 flex items-center gap-1.5">
          <span className="text-[11px] uppercase tracking-wide text-muted">Toolkits</span>
          <InfoTip text="MCP servers (optionally: specific tools) this work expects, one per line — 'github: create_pr, get_diff'. Declarative; access is still granted in the MCP registry." />
        </div>
        <Textarea autoGrow rows={2} value={toolkitsText} onChange={(e) => setToolkitsText(e.target.value)} onBlur={saveToolkits} className="max-h-40 font-mono text-xs" placeholder={'github: create_pr, get_diff\nsandbox'} />
      </div>
    </Panel>
  )
}
