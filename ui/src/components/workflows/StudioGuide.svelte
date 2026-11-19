<script lang="ts" module>
  // The Studio's guided flow — "Teach your agents" — for people who shouldn't
  // need to know that a workflow and a skill are different things. Four small
  // steps: name the work, say how to recognize it, explain how it's done (Muse
  // drafts the skill from plain words), pick who carries it. One Create at the
  // end writes the skill + the workflow together and shows what will happen.
  export interface GuidePrefill {
    name?: string
    describe?: string
    boardIds?: string[]
    /** Preselects who carries the skill (an owner slug or 'shared') — set when
     *  the guide is launched from a specific agent's dashboard. */
    owner?: string
  }

  const STEPS = ['The work', 'When it applies', 'How it’s done', 'Who does it'] as const

  const slugify = (v: string) =>
    v.trim().toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '').slice(0, 60) || 'workflow'
</script>

<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import { Check } from '@lucide/svelte'
  import SettingsField from '@/components/settings/SettingsField.svelte'
  import Button from '@/components/ui/Button.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Textarea from '@/components/ui/Textarea.svelte'
  import Modal from '@/components/ui/Modal.svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import { listQuery } from '@/components/ui/query-state'
  import Radio from '@/components/ui/Radio.svelte'
  import WaitingMark from '@/components/ui/WaitingMark.svelte'
  import Markdown from '@/components/ui/Markdown.svelte'
  import AutoHeight from '@/components/ui/AutoHeight.svelte'
  import { cn } from '@/lib/cn'
  import { focusRing } from '@/components/ui/control'
  import { errorMessage, putJson } from '@/lib/fetch-json'
  import { slide, staggerIn } from '@/lib/motion'
  import { useBoards } from '@/lib/boards.svelte'
  import { streamMuse } from '@/lib/muse.svelte'
  import type { SkillOwner } from '@/lib/skills'
  import { createWorkflow, updateWorkflow } from '@/lib/workflows'
  import TokenInput from './TokenInput.svelte'

  let {
    open,
    onClose,
    owners,
    prefill,
    onCreated,
  }: {
    open: boolean
    onClose: () => void
    owners: SkillOwner[]
    prefill?: GuidePrefill
    onCreated: (workflowId: string | null, skillRef: string) => void
  } = $props()

  const qc = useQueryClient()
  // "No boards yet." over a 500 — in a wizard whose next step asks you to pick
  // which of them this workflow matches.
  const boardsList = listQuery(useBoards(), { title: 'Could not load your boards', variant: 'inline' })
  const boards = $derived(boardsList.rows)
  let step = $state(0)

  // Step 1 — the work
  // svelte-ignore state_referenced_locally -- reason: step draft seeded from the prefill once at mount; the guide remounts per open
  let name = $state(prefill?.name ?? '')
  // Step 2 — recognition
  // svelte-ignore state_referenced_locally -- reason: step draft seeded from the prefill once at mount; the guide remounts per open
  let boardIds = $state<string[]>(prefill?.boardIds ?? [])
  let labels = $state<string[]>([])
  let keywords = $state<string[]>([])
  // Step 3 — the flow
  // svelte-ignore state_referenced_locally -- reason: step draft seeded from the prefill once at mount; the guide remounts per open
  let describe = $state(prefill?.describe ?? '')
  let skillMd = $state('')
  let drafting = $state(false)
  let museError = $state<string | null>(null)
  let editingMd = $state(false)
  let abort: AbortController | null = null
  // Step 4 — placement
  const editable = $derived(owners.filter((o) => o.canEdit))
  // svelte-ignore state_referenced_locally -- reason: step draft seeded from the prefill and owner list once at mount; the guide remounts per open
  let ownerPick = $state<string>(prefill?.owner && owners.some((o) => o.owner === prefill.owner && o.canEdit) ? prefill.owner : '')
  let creating = $state(false)
  let error = $state<string | null>(null)
  let done = $state<string | null>(null) // skill ref when created

  const hasRules = $derived(boardIds.length > 0 || labels.length > 0 || keywords.length > 0)
  const owner = $derived(ownerPick || (editable.some((o) => o.owner === 'shared') ? 'shared' : (editable[0]?.owner ?? '')))
  const boardName = (id: string) => boards.find((b) => b.id === id)?.name ?? 'a board'

  /** The plain-English promise of what this will do — shown from step 2 on. */
  const sentence = () => {
    const where = [
      boardIds.length ? `on ${boardIds.map(boardName).join(' or ')}` : '',
      labels.length ? `labeled ${labels.map((l) => `“${l}”`).join(' or ')}` : '',
      keywords.length ? `mentioning ${keywords.map((k) => `“${k}”`).join(' or ')}` : '',
    ]
      .filter(Boolean)
      .join(', ')
    const who = owner === 'shared' ? 'every agent' : (owners.find((o) => o.owner === owner)?.label ?? 'the agent')
    const work = name.trim()
    if (!where) return `No rules yet, so no ticket is routed. ${who.charAt(0).toUpperCase()}${who.slice(1)} can still use “${work}”.`
    return `For tickets ${where}, ${owner === 'shared' ? 'any agent that picks one up' : who} follows “${work}”.`
  }

  const draft = async () => {
    if (!describe.trim()) return
    drafting = true
    museError = null
    skillMd = ''
    abort?.abort()
    abort = new AbortController()
    try {
      const text = await streamMuse(
        {
          kind: 'skill',
          instruction: describe,
          context: `A skill named "${slugify(name)}" — how "${name}" work is done at this org. SKILL.md format: a "# ${slugify(name)}" heading, one "When to use:" line, then concrete numbered steps a competent agent can follow.`,
        },
        (piece) => (skillMd += piece),
        abort.signal,
      )
      skillMd = text
    } catch (e) {
      if ((e as Error).name !== 'AbortError') museError = (e as Error).message
    } finally {
      drafting = false
    }
  }

  const create = async () => {
    if (!owner || !skillMd.trim()) return
    creating = true
    error = null
    try {
      // A colliding name gets a numeric suffix — never overwrite an existing skill.
      const taken = new Set(owners.find((o) => o.owner === owner)?.skills.map((s) => s.name) ?? [])
      let skillName = slugify(name)
      for (let i = 2; taken.has(skillName); i++) skillName = `${slugify(name)}-${i}`
      // A failed save throws the server's own sentence; the catch below shows it.
      await putJson(`/api/skills/${owner}/${skillName}`, { content: skillMd })
      let workflowId: string | null = null
      if (hasRules) {
        const { workflow } = await createWorkflow({ name: name.trim() })
        workflowId = workflow?.id ?? null
        if (workflowId) {
          await updateWorkflow(workflowId, {
            match: {
              ...(boardIds.length ? { boards: boardIds } : {}),
              ...(labels.length ? { labels } : {}),
              ...(keywords.length ? { keywords } : {}),
            },
            skills: [skillName],
          })
        }
      }
      await qc.invalidateQueries({ queryKey: ['skill-library'] })
      await qc.invalidateQueries({ queryKey: ['workflows'] })
      done = `${owner}/${skillName}`
      onCreated(workflowId, `${owner}/${skillName}`)
    } catch (e) {
      error = errorMessage(e)
    } finally {
      creating = false
    }
  }

  const canNext = $derived([name.trim().length > 1, true, skillMd.trim().length > 0, !!owner][step])
  const choice = 'gap-2.5 font-sans text-[13px] leading-5 text-fg'
</script>

<!-- What the choices so far add up to, in one sentence. An inset well, not a card: the dialog is already a panel. -->
{#snippet outcome()}
  <p class="rounded-md border border-line bg-surface px-3 py-2 font-sans text-[13px] leading-5 text-muted">{sentence()}</p>
{/snippet}

<Modal {open} {onClose} width="max-w-2xl" title={done ? 'Your agents learned something new' : 'Teach your agents'}>
  <!-- Step-to-step motion (ANIMATIONS.md wizard row): AutoHeight glides the
       panel between step heights, {#key step} + staggerIn brings each step's
       sections in 40ms apart. No exit on the outgoing step — the incoming
       stagger + gliding height IS the transition. The stepper and the footer
       stay mounted (outside the key) so focus survives step changes. -->
  <AutoHeight>
    {#if done}
      <div use:staggerIn class="space-y-3">
        <p class="font-sans text-sm leading-5 text-fg">{sentence()}</p>
        <p class="font-sans text-[13px] leading-5 text-muted">
          {hasRules
            ? 'Matching tickets carry this skill from now on. Change the skill or its rules any time in Agent Studio.'
            : 'The skill is in the library. Add routing rules later and matching tickets will carry it.'}
        </p>
      </div>
    {:else}
      <div class="space-y-6">
        <!-- The look of `Steps`, as buttons: a step already passed is a way back to it. -->
        <div class="flex flex-wrap items-center gap-x-2 gap-y-2">
          {#each STEPS as s, i (s)}
            {#if i > 0}<span aria-hidden="true" class="h-px w-6 bg-line"></span>{/if}
            <button
              type="button"
              disabled={i >= step}
              aria-current={i === step ? 'step' : undefined}
              onclick={() => (step = i)}
              class={cn('flex items-center gap-2 rounded-sm', focusRing, i < step && 'cursor-pointer')}
            >
              <span
                class={cn(
                  'grid h-5 w-5 place-items-center rounded-full font-sans text-xs font-medium tabular-nums',
                  i < step ? 'bg-accent text-surface' : i === step ? 'border border-accent text-accent' : 'border border-line text-muted',
                )}
              >
                {#if i < step}<Check size={11} />{:else}{i + 1}{/if}
              </span>
              <span class={cn('font-sans text-[13px] leading-5', i === step ? 'font-medium text-fg' : 'text-muted')}>{s}</span>
            </button>
          {/each}
        </div>

        {#key step}
          {#if step === 0}
            <div use:staggerIn>
              <SettingsField label="What work are you teaching?" for="guide-name" hint="Name it the way your team talks about it.">
                <Input
                  id="guide-name"
                  autofocus
                  size="sm"
                  bind:value={name}
                  placeholder="Weekly metrics report"
                  onkeydown={(e) => {
                    if (e.key === 'Enter' && name.trim().length > 1) step = 1
                  }}
                />
              </SettingsField>
            </div>
          {/if}

          {#if step === 1}
            <div use:staggerIn class="space-y-5">
              <p class="font-sans text-[13px] leading-5 text-muted">
                Tickets that match carry this skill when an agent picks them up. Skip this to add the skill without routing.
              </p>
              <SettingsField label="Boards">
                <div class="flex flex-wrap gap-1.5">
                  {#each boards as b (b.id)}
                    <Chip
                      selected={boardIds.includes(b.id)}
                      onSelect={() => (boardIds = boardIds.includes(b.id) ? boardIds.filter((x) => x !== b.id) : [...boardIds, b.id])}
                    >
                      {b.name}
                    </Chip>
                  {/each}
                  {#if boards.length === 0 && !boardsList.failed && !boardsList.pending}
                    <span class="font-sans text-[13px] leading-5 text-muted">No boards yet.</span>
                  {/if}
                </div>
                {#if boardsList.notice}<div transition:slide={{ duration: 150 }}><QueryError {...boardsList.notice} /></div>{/if}
              </SettingsField>
              <SettingsField label="Labels">
                <TokenInput value={labels} onChange={(v) => (labels = v)} placeholder="Add a label" />
              </SettingsField>
              <SettingsField label="Keywords" hint="Words in the ticket's title or description.">
                <TokenInput value={keywords} onChange={(v) => (keywords = v)} placeholder="Add a keyword" />
              </SettingsField>
              {@render outcome()}
            </div>
          {/if}

          {#if step === 2}
            <div use:staggerIn class="space-y-3">
              <SettingsField
                label="How is it done?"
                for="guide-describe"
                hint="Explain it as you would to a new teammate. Muse, the writing assistant, drafts the skill from it."
              >
                <Textarea
                  id="guide-describe"
                  autoGrow
                  rows={3}
                  bind:value={describe}
                  placeholder="Pull last week's numbers first, compare them to the four weeks before, and lead with the one change that matters."
                  class="max-h-40 text-sm"
                />
              </SettingsField>
              <div class="flex items-center gap-3">
                <Button size="sm" variant="outline" onclick={() => void draft()} disabled={!describe.trim() || drafting}>
                  {skillMd ? 'Draft again' : 'Draft the skill'}
                </Button>
                {#if drafting}
                  <span class="flex items-center gap-1.5 font-sans text-[13px] leading-5 text-muted">
                    Muse is drafting <WaitingMark site="workflows/muse-draft" size={12} class="text-accent" />
                  </span>
                {/if}
                {#if museError}<span class="font-sans text-[13px] leading-5 text-danger">{museError}</span>{/if}
              </div>
              {#if skillMd}
                {#if editingMd}
                  <Textarea autoGrow rows={8} bind:value={skillMd} class="max-h-72 font-mono text-xs" />
                {:else}
                  <button
                    type="button"
                    onclick={() => {
                      if (!drafting) editingMd = true
                    }}
                    title="Edit the draft"
                    class={cn('block max-h-72 w-full overflow-y-auto rounded-md border border-line bg-surface px-4 py-3 text-left text-sm', focusRing)}
                  >
                    <Markdown class="tiptap" children={skillMd} />
                  </button>
                {/if}
              {/if}
            </div>
          {/if}

          {#if step === 3}
            <div use:staggerIn class="space-y-5">
              <SettingsField label="Who should know this?">
                <div class="space-y-2.5 pt-1">
                  {#if editable.some((o) => o.owner === 'shared')}
                    <Radio name="owner" checked={owner === 'shared'} onChange={() => (ownerPick = 'shared')} label="Every agent" class={choice} />
                  {/if}
                  {#each editable.filter((o) => o.owner !== 'shared') as o (o.owner)}
                    <Radio name="owner" checked={owner === o.owner} onChange={() => (ownerPick = o.owner)} label={`Only ${o.label}`} class={choice} />
                  {/each}
                  {#if editable.length === 0}
                    <p class="font-sans text-[13px] leading-5 text-warning">You cannot edit any agent's skills. Ask an admin for access.</p>
                  {/if}
                </div>
              </SettingsField>
              {@render outcome()}
              {#if error}<p transition:slide={{ duration: 150 }} class="font-sans text-[13px] leading-5 text-danger">{error}</p>{/if}
            </div>
          {/if}
        {/key}
      </div>
    {/if}
  </AutoHeight>
  {#snippet footer()}
    <div class="flex justify-end gap-2">
      {#if done}
        <Button variant="outline" size="sm" onclick={onClose}>Done</Button>
      {:else}
        <Button
          variant="ghost"
          size="sm"
          onclick={() => {
            if (step === 0) onClose()
            else step = step - 1
          }}
        >
          {step === 0 ? 'Cancel' : 'Back'}
        </Button>
        {#if step < STEPS.length - 1}
          <Button size="sm" onclick={() => (step = step + 1)} disabled={!canNext}>
            {step === 1 && !hasRules ? 'Skip routing' : 'Next'}
          </Button>
        {:else}
          <Button size="sm" onclick={() => void create()} disabled={!canNext || creating || !skillMd.trim()}>
            {creating ? 'Creating' : hasRules ? 'Create skill and routing' : 'Create skill'}
          </Button>
        {/if}
      {/if}
    </div>
  {/snippet}
</Modal>
