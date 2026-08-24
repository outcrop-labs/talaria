<script lang="ts">
  // Plan chat: an agent drafts tickets from a conversation (a channel or a plan);
  // the human reviews/edits here and creates the keepers — into inbox, never
  // assigned. Board-first: picking the board up front lets its default ticket
  // template shape the drafts (resolution: explicit pick → agent → board default).
  //
  // The DRAFT itself is owned by the plan-drafts store (see its header): this
  // modal is a view of the job, so closing it mid-draft or mid-review loses
  // nothing — the header button on the plan surface reopens to wherever the
  // job is.
  import { useQueryClient } from '@tanstack/svelte-query'
  import { fly } from '@/lib/motion'
  import { cn } from '@/lib/cn'
  import Button from '@/components/ui/Button.svelte'
  import Modal from '@/components/ui/Modal.svelte'
  import Select from '@/components/ui/Select.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import { listQuery } from '@/components/ui/query-state'
  import ProposalCard from './ProposalCard.svelte'
  import DraftingTickets from './DraftingTickets.svelte'
  import { discardPlanDraft, patchPlanDraft, planDraft, startPlanDraft } from './plan-drafts.svelte'
  import type { Proposal } from './plan-modal'
  import { addDependency, createTask, useBoards } from '@/lib/boards.svelte'
  import { useTemplates } from '@/lib/templates'
  import type { AgentModel } from '@/lib/agents'

  let {
    open,
    onClose,
    planId,
    draftUrl,
    agents,
  }: {
    open: boolean
    onClose: () => void
    /** The conversation drafts pair to — the store key. */
    planId: string
    draftUrl: string
    agents: AgentModel[]
  } = $props()

  const qc = useQueryClient()
  // A 500 on either of these used to leave a Select reading "Pick a board" with
  // nothing under it — indistinguishable from an account with no boards, and
  // the Draft button stays disabled with no reason given.
  const boardsList = listQuery(useBoards(), { title: 'Could not load your boards', variant: 'inline' })
  const templatesList = listQuery(useTemplates(), { title: 'Could not load ticket templates', variant: 'inline' })
  const boards = $derived(boardsList.rows)
  const templates = $derived(templatesList.rows)
  const boardsLoading = $derived(boardsList.pending)
  const templatesLoading = $derived(templatesList.pending)
  let agentModel = $state(agents[0]?.id ?? '')
  let tier = $state('')
  let boardId = $state('')
  let templateId = $state('') // '' = automatic (agent → board default)
  let phase = $state<'idle' | 'creating' | 'done'>('idle')
  let note = $state<string | null>(null) // creation failures only; draft failures live on the job
  let createdCount = $state(0)

  const job = $derived(planDraft(planId))
  const proposals = $derived(job?.status === 'ready' ? job.proposals : null)

  const picked = $derived(agents.find((a) => a.id === (agentModel || agents[0]?.id)))
  const tiers = $derived(picked?.tiers ?? [])
  const editable = $derived(boards.filter((b) => b.role === 'owner' || b.role === 'editor'))
  const ticketTemplates = $derived(templates.filter((t) => t.kind === 'ticket'))
  const included = $derived(proposals?.filter((p) => p.include) ?? [])

  // The board picked when the draft started rides on the job; a modal that
  // reopens straight into review (or retry) adopts it, since creation and the
  // "→ board" footer read it. Only fills an empty pick — never overrules one.
  $effect(() => {
    if (job?.boardId && !boardId) boardId = job.boardId
  })

  // A fresh batch starts the walk at the first slide — including on reopen.
  $effect(() => {
    if (job?.status === 'ready') {
      slide = 0
      dir = 1
    }
  })

  const draft = () => {
    note = null
    startPlanDraft(planId, {
      draftUrl,
      agentModel: picked?.id,
      tier: tier || null,
      boardId: boardId || null,
      templateId: templateId || null,
    })
  }

  // Two passes: create every included ticket (collecting ids by proposal index),
  // then wire dependencies between the ones that were created. Each success
  // unticks its proposal, so a retry after a mid-loop failure only creates
  // what's still pending — never duplicates. Unticks go to the STORE, so even
  // a modal closed mid-loop keeps its place (the loop itself runs to completion
  // regardless — its writes are all store writes).
  const createAll = async () => {
    const batch = proposals
    if (!boardId || included.length === 0 || !batch) return
    phase = 'creating'
    note = null
    const createdIds = new Map<number, string>() // proposal index → task id
    let failed: string | null = null
    for (const [i, p] of batch.entries()) {
      if (!p.include) continue
      try {
        const res = (await createTask(boardId, {
          title: p.title,
          description: p.description || undefined,
          priority: p.priority,
          effort: p.effort,
          tags: p.tags.length ? p.tags : undefined,
        })) as { task?: { id: string } }
        if (res.task?.id) createdIds.set(i, res.task.id)
        createdCount += 1
        patchPlanDraft(planId, i, { include: false })
      } catch {
        failed = p.title
        break
      }
    }
    // Dependencies between just-created tickets (skipped/failed ones drop out).
    for (const [i, taskId] of createdIds) {
      for (const dep of batch[i]?.dependsOn ?? []) {
        const dependsOnId = createdIds.get(dep)
        if (dependsOnId) await addDependency(taskId, dependsOnId).catch(() => {})
      }
    }
    await qc.invalidateQueries({ queryKey: ['tasks', boardId] })
    if (failed) {
      note = `"${failed}" failed to create. The ones before it are done; retry creates only what's left`
      phase = 'idle'
    } else {
      discardPlanDraft(planId) // the batch is consumed; the button resets
      phase = 'done'
    }
  }

  const patch = (i: number, p: Partial<Proposal>) => {
    patchPlanDraft(planId, i, p)
  }

  // ── The review walk: one ticket per slide ─────────────────────────────────
  //
  // The stack made every draft compete for one scroll and offered a single
  // ending ("Create N"). The walk reads them in order — edit in place, keep
  // or drop via the card's checkbox, dots jump — and ends two ways: Finish
  // creates the keepers, Create all takes the express lane past the walking.
  let slide = $state(0)
  let dir = $state(1) // which way the last hop went, for the slide transition
  const atLast = $derived(proposals !== null && slide === proposals.length - 1)
  const goTo = (j: number) => {
    if (proposals === null) return
    dir = j >= slide ? 1 : -1
    slide = Math.min(Math.max(j, 0), proposals.length - 1)
  }
  const next = () => goTo(slide + 1)

  // Arrow keys walk the slides — but never out from under a field being
  // edited: ← and → inside an input, the description editor, or anything
  // contenteditable are cursor moves, not navigation. (Same guard the
  // app-wide context-menu suppression uses.)
  $effect(() => {
    if (proposals === null || phase !== 'idle') return
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== 'ArrowLeft' && e.key !== 'ArrowRight') return
      const t = e.target as HTMLElement | null
      if (t?.closest('input, textarea, [contenteditable="true"], [contenteditable=""]')) return
      e.preventDefault()
      if (e.key === 'ArrowRight') next()
      else goTo(slide - 1)
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  })
</script>

<Modal {open} {onClose} title={proposals ? 'Review tickets' : 'Drafting tickets'} takeover={!!proposals} width="max-w-lg">
  <div class="space-y-4">
    {#if phase === 'done'}
      <p class="text-sm text-fg">
        Created {createdCount} ticket{createdCount === 1 ? '' : 's'} in
        <span class="font-medium">{editable.find((b) => b.id === boardId)?.name}</span>. They're in the
        inbox, ready to assign.
      </p>
      <div class="flex justify-end border-t border-line-subtle pt-3">
        <Button size="sm" onclick={onClose}>
          Done
        </Button>
      </div>
    {:else if job?.status === 'drafting'}
      <DraftingTickets label={picked?.label ?? 'The agent'} />
      <div class="flex justify-end border-t border-line-subtle pt-3">
        <!-- Not "Cancel": closing the modal no longer cancels anything. The
             draft keeps running; the button in the header brings you back. -->
        <Button variant="ghost" size="sm" onclick={onClose}>
          Keep working
        </Button>
      </div>
    {:else if proposals === null}
      <p class="text-sm text-muted">
        An agent reads the conversation and drafts tickets for the board you pick, formatted on the board's
        ticket template. You review before anything is created.
      </p>
      <div class="grid grid-cols-2 gap-3">
        <div>
          <label class="mb-1 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Planner</label>
          <Select value={picked?.id ?? ''} size="sm" onchange={(e) => (agentModel = e.currentTarget.value)} class="w-full">
            {#each agents as a (a.id)}
              <option value={a.id}>
                {a.label}
              </option>
            {/each}
          </Select>
        </div>
        <div>
          <label class="mb-1 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Model tier</label>
          <Select bind:value={tier} size="sm" class="w-full">
            <option value="">main</option>
            {#each tiers as t (t)}
              <option value={t}>
                {t}
              </option>
            {/each}
          </Select>
        </div>
        <div>
          <label class="mb-1 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Board</label>
          {#if boardsLoading}
            <!-- Select-shaped stand-in (sm control = h-9) so the field —
                 and why Draft is still disabled — is visible on open. -->
            <Skeleton class="h-9 w-full rounded-xl" />
          {:else}
            <Select bind:value={boardId} size="sm" class="w-full">
              <option value="">Pick a board</option>
              {#each editable as b (b.id)}
                <option value={b.id}>
                  {b.name}
                </option>
              {/each}
            </Select>
          {/if}
          {#if boardsList.notice}<QueryError {...boardsList.notice} />{/if}
        </div>
        <div>
          <label class="mb-1 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Ticket template</label>
          {#if templatesLoading}
            <Skeleton class="h-9 w-full rounded-xl" />
          {:else}
            <Select bind:value={templateId} size="sm" class="w-full">
              <option value="">Automatic (agent → board default)</option>
              {#each ticketTemplates as t (t.id)}
                <option value={t.id}>
                  {t.name}
                </option>
              {/each}
            </Select>
          {/if}
          {#if templatesList.notice}<QueryError {...templatesList.notice} />{/if}
        </div>
      </div>
      {#if job?.status === 'failed' && job.note}
        <div class="text-sm" style:color="var(--theme-danger)">
          {job.note}
        </div>
      {/if}
      <div class="flex justify-end gap-2 border-t border-line-subtle pt-3">
        <Button variant="ghost" size="sm" onclick={onClose}>
          Cancel
        </Button>
        <Button size="sm" onclick={draft} disabled={!picked || !boardId}>
          Draft tickets
        </Button>
      </div>
    {:else}
      <!-- The walk. Review header: where you are, and a dot per draft — the
           current one pill-shaped, a dropped one hollowed to a hairline —
           each dot a jump. -->
      <div class="flex items-center justify-between gap-3">
        <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">
          Review · {slide + 1} of {proposals.length}
        </span>
        <div class="flex items-center gap-1.5">
          {#each proposals as q, j (j)}
            <button
              type="button"
              aria-label={`Ticket ${j + 1}${q.include ? '' : ' (dropped)'}`}
              onclick={() => goTo(j)}
              class={cn(
                'h-1.5 rounded-full transition-all',
                j === slide ? 'w-4 bg-accent' : q.include ? 'w-1.5 bg-fg/40 hover:bg-fg/70' : 'w-1.5 bg-line',
              )}
            ></button>
          {/each}
        </div>
      </div>
      <!-- One ticket per slide, keyed so each hop plays the walk's direction. -->
      {#key slide}
        <div class="mt-3" in:fly={{ x: 24 * dir, duration: 200 }}>
          <ProposalCard
            index={slide}
            proposal={proposals[slide]!}
            all={proposals}
            onPatch={(patchP) => patch(slide, patchP)}
          />
        </div>
      {/key}
      <div class="mt-4 flex items-center gap-2 border-t border-line-subtle pt-3">
        <span class="text-xs text-muted">
          → <span class="font-medium text-fg">{editable.find((b) => b.id === boardId)?.name}</span>
        </span>
        {#if note}
          <span class="text-xs" style:color="var(--theme-danger)">
            {note}
          </span>
        {/if}
        <span class="ml-auto"></span>
        <Button variant="ghost" size="sm" onclick={() => discardPlanDraft(planId)}>
          Back
        </Button>
        <!-- The express lane: create every kept draft without walking the
             rest. Disabled reads (creating / nothing kept) match Finish. -->
        <Button variant="outline" size="sm" onclick={() => void createAll()} disabled={phase === 'creating' || !boardId || included.length === 0}>
          {phase === 'creating' ? `Creating ${included.length} left` : 'Create all'}
        </Button>
        <Button
          size="sm"
          onclick={() => (atLast ? void createAll() : next())}
          disabled={phase === 'creating' || (atLast && included.length === 0)}
        >
          {atLast ? 'Finish' : 'Next'}
        </Button>
      </div>
    {/if}
  </div>
</Modal>
