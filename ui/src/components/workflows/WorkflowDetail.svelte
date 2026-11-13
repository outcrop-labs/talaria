<script lang="ts">
  // The body of the dialog a Studio "Routed work" row opens. A workflow
  // classifies tickets by match rules (boards / labels / keywords) and delivers
  // skills and declared toolkits with the dispatched work. The dialog's footer,
  // with Delete, is the caller's.
  import SettingsField from '@/components/settings/SettingsField.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Textarea from '@/components/ui/Textarea.svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import Toggle from '@/components/ui/Toggle.svelte'
  import InfoTip from '@/components/ui/InfoTip.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import { listQuery } from '@/components/ui/query-state'
  import { slide } from '@/lib/motion'
  import { useBoards } from '@/lib/boards.svelte'
  import { useSkills } from '@/lib/skills'
  import { updateWorkflow, type TaskWorkflow } from '@/lib/workflows'
  import { skillTitle } from '@/components/skills/skill-title'
  import TokenInput from './TokenInput.svelte'

  let { workflow, onChanged }: { workflow: TaskWorkflow; onChanged: () => void } = $props()

  const heading = 'font-sans text-sm font-semibold leading-5 text-fg'
  const note = 'font-sans text-[13px] leading-5 text-muted'

  // Both of these fed "there are none" copy. The skill one was worse than that:
  // with an empty library every skill this workflow is bound to falls out of
  // `known` below and gets drawn as "Bound but not in the library" — a warning
  // chip offering to unbind a skill that is perfectly fine.
  const boardsList = listQuery(useBoards(), { title: 'Could not load your boards', variant: 'inline' })
  const libraryList = listQuery(useSkills(), { title: 'Could not load the skill library', variant: 'inline' })
  const boards = $derived(boardsList.rows)
  const rawSkillOwners = $derived(libraryList.rows)
  // Platform plumbing skills aren't bindable flow content — hide them here too.
  const skillOwners = $derived(rawSkillOwners.map((o) => ({ ...o, skills: o.skills.filter((sk) => !sk.platform) })))
  // Edit drafts: keyed remount per workflow ({#key openWorkflow.id} at the
  // call site), and drafts deliberately survive the onChanged refetch —
  // clobbering in-progress edits would be the bug.
  // svelte-ignore state_referenced_locally -- reason: edit drafts; keyed remount per workflow, and drafts must survive the refetch
  let name = $state(workflow.name)
  // svelte-ignore state_referenced_locally -- reason: edit drafts; keyed remount per workflow, and drafts must survive the refetch
  let description = $state(workflow.description)
  // svelte-ignore state_referenced_locally -- reason: edit drafts; keyed remount per workflow, and drafts must survive the refetch
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

<!-- A dialog body: stacked fields under plain headings, no card inside the
     dialog. Each field saves when it is left. -->
<div class="space-y-8">
  <div class="space-y-5">
    <SettingsField label="Name">
      <Input size="sm" bind:value={name} onblur={() => name.trim() && name !== workflow.name && void save({ name: name.trim() })} />
    </SettingsField>
    <SettingsField label="Description" hint="For admins. Agents never see it.">
      <Input size="sm" bind:value={description} onblur={() => description !== workflow.description && void save({ description })} placeholder="What kind of work this is" />
    </SettingsField>
    <div class="flex items-center gap-6">
      <div class="min-w-0 flex-1">
        <div class="font-sans text-[13px] font-medium leading-5 text-fg">Route matching tickets</div>
        <p class="font-sans text-[13px] leading-5 text-muted">Off keeps the rules but sends nothing through them.</p>
      </div>
      <Toggle checked={workflow.enabled} onChange={(v) => void save({ enabled: v })} title={workflow.enabled ? 'Stop routing matching tickets' : 'Route matching tickets'} />
    </div>
  </div>

  <section class="space-y-5">
    <div class="flex items-center gap-2">
      <h3 class={heading}>Matches when</h3>
      <InfoTip text="A ticket matches a rule when it fits any of its values, and it must match every rule you set. With no rules, nothing matches." />
    </div>
    <SettingsField label="Boards">
      <div class="flex flex-wrap gap-1.5">
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
          <span class={note}>No boards yet.</span>
        {/if}
      </div>
      {#if boardsList.notice}<div transition:slide={{ duration: 150 }}><QueryError {...boardsList.notice} /></div>{/if}
    </SettingsField>
    <SettingsField label="Labels">
      <TokenInput value={workflow.match.labels ?? []} onChange={(labels) => saveMatch({ labels })} placeholder="Add a label" />
    </SettingsField>
    <SettingsField label="Keywords" hint="Words in the ticket's title or description.">
      <TokenInput value={workflow.match.keywords ?? []} onChange={(keywords) => saveMatch({ keywords })} placeholder="Add a keyword" />
    </SettingsField>
  </section>

  <section class="space-y-5">
    <div class="flex items-center gap-2">
      <h3 class={heading}>Skills</h3>
      <InfoTip text="The skills this kind of work follows. Dispatch tells the agent to load them. Edit a skill itself from the agent's skill list." />
    </div>
    {#each skillOwners as o (o.owner)}
      {#if o.skills.length}
        <SettingsField label={o.owner === 'shared' ? 'Every agent' : o.label}>
          <div class="flex flex-wrap gap-1.5">
            {#each o.skills as sk (`${o.owner}/${sk.name}`)}
              {@const on = workflow.skills.includes(sk.name)}
              <Chip
                title={sk.description}
                selected={on}
                onSelect={() =>
                  void save({ skills: on ? workflow.skills.filter((x) => x !== sk.name) : [...workflow.skills, sk.name] })}
              >
                {skillTitle(sk.name)}
              </Chip>
            {/each}
          </div>
        </SettingsField>
      {/if}
    {/each}
    {#if orphans.length}
      <div transition:slide={{ duration: 150 }}>
        <SettingsField label="No longer in the library" hint="These are still named here. Remove them, or add the skill back.">
          <div class="flex flex-wrap gap-1.5">
            {#each orphans as sk (sk)}
              <Chip tone="warn" onRemove={() => void save({ skills: workflow.skills.filter((x) => x !== sk) })}>
                {skillTitle(sk)}
              </Chip>
            {/each}
          </div>
        </SettingsField>
      </div>
    {/if}
    {#if libraryList.notice}<div transition:slide={{ duration: 150 }}><QueryError {...libraryList.notice} /></div>{/if}
    {#if !libraryList.failed && !libraryList.pending && skillOwners.every((o) => !o.skills.length) && !workflow.skills.length}
      <p class={note}>No skills yet. Teach an agent one from its skill list.</p>
    {/if}
  </section>

  <section class="space-y-3">
    <div class="flex items-center gap-2">
      <h3 class={heading}>Tools</h3>
      <InfoTip text="MCP servers this work expects, one per line, with tools after a colon if only some are needed. This only declares them. Access is still granted on the MCP page." />
    </div>
    <Textarea autoGrow rows={2} bind:value={toolkitsText} onblur={saveToolkits} class="max-h-40 font-mono text-xs" placeholder={'github: create_pr, get_diff\nsandbox'} />
  </section>
</div>
