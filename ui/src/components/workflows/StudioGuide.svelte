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
  import { slide, staggerIn } from '@/lib/motion'
  import { useBoards } from '@/lib/boards.svelte'
  import { streamMuse } from '@/lib/muse.svelte'
  import { createWorkflow, updateWorkflow, type SkillLibraryOwner } from '@/lib/workflows'
  import Tokens from './Tokens.svelte'

  let {
    open,
    onClose,
    owners,
    prefill,
    onCreated,
  }: {
    open: boolean
    onClose: () => void
    owners: SkillLibraryOwner[]
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
  let name = $state(prefill?.name ?? '')
  // Step 2 — recognition
  let boardIds = $state<string[]>(prefill?.boardIds ?? [])
  let labels = $state<string[]>([])
  let keywords = $state<string[]>([])
  // Step 3 — the flow
  let describe = $state(prefill?.describe ?? '')
  let skillMd = $state('')
  let drafting = $state(false)
  let museError = $state<string | null>(null)
  let editingMd = $state(false)
  let abort: AbortController | null = null
  // Step 4 — placement
  const editable = $derived(owners.filter((o) => o.canEdit))
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
    const who =
      owner === 'shared' ? 'any agent picking it up' : (owners.find((o) => o.owner === owner)?.label ?? 'the agent')
    return `Tickets ${where || '(no recognition rules yet)'} → ${who} follows “${slugify(name)}”.`
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
      const put = await fetch(`/api/skills/${owner}/${skillName}`, {
        method: 'PUT',
        credentials: 'same-origin',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ content: skillMd }),
      })
      if (!put.ok) throw new Error('could not save the skill')
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
      error = (e as Error).message
    } finally {
      creating = false
    }
  }

  const canNext = $derived([name.trim().length > 1, true, skillMd.trim().length > 0, !!owner][step])
</script>

<Modal {open} {onClose} width="max-w-2xl" title={done ? 'Your agents learned something new' : 'Teach your agents'}>
  <!-- Step-to-step motion (ANIMATIONS.md wizard row): AutoHeight glides the
       panel between step heights, {#key step} + staggerIn brings each step's
       sections in 40ms apart. No exit on the outgoing step — the incoming
       stagger + gliding height IS the transition. Stepper and nav row stay
       mounted (outside the key) so focus survives step changes. -->
  <AutoHeight>
    {#if done}
      <div use:staggerIn class="space-y-4">
        <p class="text-sm text-fg">{sentence()}</p>
        <p class="text-sm text-muted">
          {hasRules
            ? 'From now on, matching tickets carry this flow when they’re dispatched. You can refine the skill or the rules any time in the Studio.'
            : 'The skill is in the library. Add recognition rules later (a workflow) and matching tickets will carry it automatically.'}
        </p>
        <div class="flex justify-end gap-2">
          <Button variant="outline" size="sm" onclick={onClose}>
            Done
          </Button>
        </div>
      </div>
    {:else}
      <div class="space-y-5">
        <!-- Stepper -->
        <div class="flex items-center gap-2">
          {#each STEPS as s, i (s)}
            <button
              type="button"
              onclick={() => {
                if (i < step) step = i
              }}
              class={cn(
                'flex items-center gap-1.5 text-xs transition-colors',
                i === step ? 'text-fg' : i < step ? 'text-accent' : 'text-muted',
                i < step && 'cursor-pointer',
              )}
            >
              <span
                class={cn(
                  'flex h-5 w-5 items-center justify-center rounded-full border font-mono text-[10px]',
                  i === step ? 'border-accent text-fg' : i < step ? 'border-accent text-accent' : 'border-line',
                )}
              >
                {i < step ? '✓' : i + 1}
              </span>
              {s}
              {#if i < STEPS.length - 1}<span class="mx-0.5 h-px w-4 bg-line"></span>{/if}
            </button>
          {/each}
        </div>

        {#key step}
          {#if step === 0}
            <div use:staggerIn class="space-y-3">
              <p class="text-sm text-muted">What kind of work are you teaching? Name it the way your team talks about it.</p>
              <Input
                autofocus
                bind:value={name}
                placeholder={'e.g. "Weekly metrics report" or "Bug triage"'}
                onkeydown={(e) => {
                  if (e.key === 'Enter' && name.trim().length > 1) step = 1
                }}
              />
            </div>
          {/if}

          {#if step === 1}
            <div use:staggerIn class="space-y-4">
              <p class="text-sm text-muted">
                How should Talaria recognize this work? Pick anything that applies — tickets matching it will carry your flow when an agent
                picks them up. You can also skip this and just add the know-how to the library.
              </p>
              <div class="space-y-1.5">
                <span class="text-xs font-medium text-fg">On these boards</span>
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
                    <span class="text-xs text-muted">No boards yet.</span>
                  {/if}
                </div>
                {#if boardsList.notice}<div transition:slide={{ duration: 150 }}><QueryError {...boardsList.notice} /></div>{/if}
              </div>
              <div class="space-y-1.5">
                <span class="text-xs font-medium text-fg">With these labels</span>
                <Tokens value={labels} onChange={(v) => (labels = v)} placeholder="Add a label…" />
              </div>
              <div class="space-y-1.5">
                <span class="text-xs font-medium text-fg">Or mentioning</span>
                <Tokens value={keywords} onChange={(v) => (keywords = v)} placeholder="Add a phrase…" />
              </div>
              <p class="rounded-lg border border-line bg-card/40 px-3 py-2 font-sans text-xs text-muted">{sentence()}</p>
            </div>
          {/if}

          {#if step === 2}
            <div use:staggerIn class="space-y-3">
              <p class="text-sm text-muted">
                Explain how this work should be done, like you would to a new teammate — what matters, what order, what “done” looks like.
                Muse turns it into a skill your agents follow.
              </p>
              <Textarea
                autoGrow
                rows={3}
                bind:value={describe}
                placeholder={'e.g. "Pull last week\'s numbers first, compare to the previous 4 weeks, lead with the one change that matters, keep it under a page…"'}
                class="max-h-40 text-sm"
              />
              <div class="flex items-center gap-2">
                <Button size="sm" onclick={() => void draft()} disabled={!describe.trim() || drafting}>
                  {skillMd ? 'Redraft' : 'Draft the skill'}
                </Button>
                {#if drafting}
                  <span class="flex items-center gap-1.5 text-xs text-muted">
                    Muse is drafting <WaitingMark site="workflows/muse-draft" size={12} class="text-accent" />
                  </span>
                {/if}
                {#if museError}<span class="text-xs text-danger">{museError}</span>{/if}
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
                    title="Click to edit"
                    class="block max-h-72 w-full overflow-y-auto rounded-lg border border-line bg-card/40 px-4 py-3 text-left text-sm"
                  >
                    <Markdown class="tiptap" children={skillMd} />
                  </button>
                {/if}
              {/if}
            </div>
          {/if}

          {#if step === 3}
            <div use:staggerIn class="space-y-4">
              <p class="text-sm text-muted">Who should know this?</p>
              <div class="space-y-2">
                {#if editable.some((o) => o.owner === 'shared')}
                  <Radio name="owner" checked={owner === 'shared'} onChange={() => (ownerPick = 'shared')} label="Every agent — shared know-how" />
                {/if}
                {#each editable.filter((o) => o.owner !== 'shared') as o (o.owner)}
                  <Radio name="owner" checked={owner === o.owner} onChange={() => (ownerPick = o.owner)} label={`Just ${o.label}`} />
                {/each}
                {#if editable.length === 0}
                  <p class="text-xs text-warning">You don’t have edit access to any agent’s skills — ask an admin for access.</p>
                {/if}
              </div>
              <p class="rounded-lg border border-line bg-card/40 px-3 py-2 font-sans text-xs text-muted">{sentence()}</p>
              {#if error}<p transition:slide={{ duration: 150 }} class="text-xs text-danger">{error}</p>{/if}
            </div>
          {/if}
        {/key}

        <div class="flex items-center justify-between border-t border-line pt-3">
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
              {step === 1 && !hasRules ? 'Skip for now' : 'Next'}
            </Button>
          {:else}
            <Button size="sm" onclick={() => void create()} disabled={!canNext || creating || !skillMd.trim()}>
              {creating ? 'Creating…' : hasRules ? 'Create workflow + skill' : 'Add to the library'}
            </Button>
          {/if}
        </div>
      </div>
    {/if}
  </AutoHeight>
</Modal>
