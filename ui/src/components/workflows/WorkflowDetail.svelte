<script lang="ts">
  // Workflow editor — the detail pane of /workflows. A workflow classifies
  // tickets by match rules (boards / labels / keywords) and delivers
  // instructions + declared toolkits with the dispatched work.
  import Button from '@/components/ui/Button.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Textarea from '@/components/ui/Textarea.svelte'
  import Panel from '@/components/ui/Panel.svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import Toggle from '@/components/ui/Toggle.svelte'
  import InfoTip from '@/components/ui/InfoTip.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import { listQuery } from '@/components/ui/query-state'
  import { slide } from '@/lib/motion'
  import { useBoards } from '@/lib/boards.svelte'
  import { updateWorkflow, useSkillLibrary, type TaskWorkflow } from '@/lib/workflows'
  import TokenInput from './TokenInput.svelte'

  let { workflow, onChanged, onDelete }: { workflow: TaskWorkflow; onChanged: () => void; onDelete: () => void } = $props()

  // Both of these fed "there are none" copy. The skill one was worse than that:
  // with an empty library every skill this workflow is bound to falls out of
  // `known` below and gets drawn as "Bound but not in the library" — a warning
  // chip offering to unbind a skill that is perfectly fine.
  const boardsList = listQuery(useBoards(), { title: 'Could not load your boards', variant: 'inline' })
  const libraryList = listQuery(useSkillLibrary(), { title: 'Could not load the skill library', variant: 'inline' })
  const boards = $derived(boardsList.rows)
  const rawSkillOwners = $derived(libraryList.rows)
  // Platform plumbing skills aren't bindable flow content — hide them here too.
  const skillOwners = $derived(rawSkillOwners.map((o) => ({ ...o, skills: o.skills.filter((sk) => !sk.platform) })))
  let name = $state(workflow.name)
  let description = $state(workflow.description)
  let toolkitsText = $state(
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

  const orphans = $derived.by(() => {
    // Only meaningful when we actually READ the library. Without this
    // guard a failed read reports every bound skill as an orphan.
    if (libraryList.failed || libraryList.pending) return []
    const known = new Set(skillOwners.flatMap((o) => o.skills.map((sk) => sk.name)))
    return workflow.skills.filter((sk) => !known.has(sk))
  })
</script>

<Panel>
  <div class="mb-4 flex items-center gap-2">
    <Input size="sm" bind:value={name} onblur={() => name.trim() && name !== workflow.name && void save({ name: name.trim() })} class="max-w-xs font-sans" />
    <Toggle checked={workflow.enabled} onChange={(v) => void save({ enabled: v })} label={workflow.enabled ? 'Enabled' : 'Disabled'} />
    <Button variant="ghost" size="sm" class="ml-auto" onclick={onDelete}>
      Delete
    </Button>
  </div>

  <div class="mb-4">
    <Input size="sm" bind:value={description} onblur={() => description !== workflow.description && void save({ description })} placeholder="One-line description (shown to admins, not agents)" />
  </div>

  <div class="mb-4 space-y-3 rounded-lg border border-line bg-card/40 px-4 py-3">
    <div class="flex items-center gap-1.5">
      <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Matches when</span>
      <InfoTip text="OR within a rule, AND across the rules you set. A workflow with no rules matches nothing." />
    </div>
    <div class="space-y-1">
      <span class="text-xs text-muted">Boards</span>
      <div class="flex flex-wrap gap-1">
        {#each boards as b (b.id)}
          {@const on = workflow.match.boards?.includes(b.id) ?? false}
          <Chip
            selected={on}
            onSelect={() =>
              saveMatch({ boards: on ? (workflow.match.boards ?? []).filter((x) => x !== b.id) : [...(workflow.match.boards ?? []), b.id] })}
          >
            {b.name}
          </Chip>
        {/each}
        {#if boards.length === 0 && !boardsList.failed && !boardsList.pending}
          <span class="text-xs text-muted">No boards yet.</span>
        {/if}
      </div>
      {#if boardsList.notice}<div transition:slide={{ duration: 150 }}><QueryError {...boardsList.notice} /></div>{/if}
    </div>
    <div class="space-y-1">
      <span class="text-xs text-muted">Labels</span>
      <TokenInput value={workflow.match.labels ?? []} onChange={(labels) => saveMatch({ labels })} placeholder="Add label…" />
    </div>
    <div class="space-y-1">
      <span class="text-xs text-muted">Title/description keywords</span>
      <TokenInput value={workflow.match.keywords ?? []} onChange={(keywords) => saveMatch({ keywords })} placeholder="Add keyword…" />
    </div>
  </div>

  <div class="mb-4">
    <div class="mb-1 flex items-center gap-1.5">
      <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Skills</span>
      <InfoTip text="The flow lives in Hermes skills, the same library the agents already mount, edited on the agent view. The workflow only names which ones this kind of work follows; dispatch tells the agent to load them." />
    </div>
    <div class="space-y-2 rounded-lg border border-line bg-card/40 px-4 py-3">
      {#each skillOwners as o (o.owner)}
        {#if o.skills.length}
          <div class="space-y-1">
            <span class="text-xs text-muted">{o.label}</span>
            <div class="flex flex-wrap gap-1">
              {#each o.skills as sk (`${o.owner}/${sk.name}`)}
                {@const on = workflow.skills.includes(sk.name)}
                <Chip
                  title={sk.description}
                  selected={on}
                  onSelect={() =>
                    void save({ skills: on ? workflow.skills.filter((x) => x !== sk.name) : [...workflow.skills, sk.name] })}
                >
                  {sk.name}
                </Chip>
              {/each}
            </div>
          </div>
        {/if}
      {/each}
      {#if orphans.length}
        <div transition:slide={{ duration: 150 }} class="space-y-1">
          <span class="text-xs text-warning">Bound but not in the library</span>
          <div class="flex flex-wrap gap-1">
            {#each orphans as sk (sk)}
              <Chip tone="warn" onRemove={() => void save({ skills: workflow.skills.filter((x) => x !== sk) })}>
                {sk}
              </Chip>
            {/each}
          </div>
        </div>
      {/if}
      {#if libraryList.notice}<div transition:slide={{ duration: 150 }}><QueryError {...libraryList.notice} /></div>{/if}
      {#if !libraryList.failed && !libraryList.pending && skillOwners.every((o) => !o.skills.length) && !workflow.skills.length}
        <span class="text-xs text-muted">No skills in the library yet; create them on an agent's manage view.</span>
      {/if}
    </div>
  </div>

  <div>
    <div class="mb-1 flex items-center gap-1.5">
      <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Toolkits</span>
      <InfoTip text="MCP servers (optionally: specific tools) this work expects, one per line, like 'github: create_pr, get_diff'. Declarative; access is still granted in the MCP registry." />
    </div>
    <Textarea autoGrow rows={2} bind:value={toolkitsText} onblur={saveToolkits} class="max-h-40 font-mono text-xs" placeholder={'github: create_pr, get_diff\nsandbox'} />
  </div>
</Panel>
