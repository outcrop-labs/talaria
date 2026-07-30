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
import { useBoards } from '@/lib/boards'
import { updateWorkflow, type TaskWorkflow } from '@/lib/workflows'

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
  const { data: boards = [] } = useBoards()
  const [name, setName] = useState(workflow.name)
  const [description, setDescription] = useState(workflow.description)
  const [instructions, setInstructions] = useState(workflow.instructions)
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
            {boards.length === 0 && <span className="text-xs text-muted">No boards yet.</span>}
          </div>
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
          <span className="text-[11px] uppercase tracking-wide text-muted">Instructions</span>
          <InfoTip text="The flow for this kind of work — delivered verbatim with the dispatched ticket. Write it to the agent." />
        </div>
        <Textarea
          autoGrow
          rows={6}
          value={instructions}
          onChange={(e) => setInstructions(e.target.value)}
          onBlur={() => instructions !== workflow.instructions && void save({ instructions })}
          className="max-h-96 font-mono text-xs"
          placeholder={'e.g. "Development work: reproduce first, write a failing test, keep the diff minimal, link the branch in your outcome report."'}
        />
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
