<script lang="ts">
  // Plan chat: an agent drafts tickets from a conversation (a channel or a plan);
  // the human reviews/edits here and creates the keepers — into inbox, never
  // assigned. Board-first: picking the board up front lets its default ticket
  // template shape the drafts (resolution: explicit pick → agent → board default).
  import { useQueryClient } from '@tanstack/svelte-query'
  import { listStagger } from '@/lib/motion'
  import Button from '@/components/ui/Button.svelte'
  import Modal from '@/components/ui/Modal.svelte'
  import Select from '@/components/ui/Select.svelte'
  import Generating from '@/components/ui/Generating.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import { listQuery } from '@/components/ui/query-state'
  import ProposalCard from './ProposalCard.svelte'
  import type { Proposal } from './plan-modal'
  import { addDependency, createTask, useBoards } from '@/lib/boards.svelte'
  import { useTemplates } from '@/lib/templates'
  import type { AgentModel } from '@/lib/agents'

  let {
    open,
    onClose,
    draftUrl,
    agents,
  }: {
    open: boolean
    onClose: () => void
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
  let proposals = $state<Proposal[] | null>(null)
  let phase = $state<'idle' | 'drafting' | 'creating' | 'done'>('idle')
  let note = $state<string | null>(null)

  const picked = $derived(agents.find((a) => a.id === (agentModel || agents[0]?.id)))
  const tiers = $derived(picked?.tiers ?? [])
  const editable = $derived(boards.filter((b) => b.role === 'owner' || b.role === 'editor'))
  const ticketTemplates = $derived(templates.filter((t) => t.kind === 'ticket'))
  const included = $derived(proposals?.filter((p) => p.include) ?? [])

  const draft = async () => {
    phase = 'drafting'
    note = null
    proposals = null
    try {
      const r = await fetch(draftUrl, {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({
          agentModel: picked?.id,
          tier: tier || null,
          boardId: boardId || null,
          templateId: templateId || null,
        }),
      })
      const j = (await r.json()) as { proposals?: Omit<Proposal, 'include'>[]; note?: string; error?: string }
      if (!r.ok || j.error) note = j.error ?? 'planning failed'
      else if (!j.proposals?.length) note = j.note ?? 'no tickets came back'
      else proposals = j.proposals.map((p) => ({ ...p, dependsOn: p.dependsOn ?? [], tags: p.tags ?? [], include: true }))
    } catch {
      note = 'planning failed. Is the gateway up?'
    } finally {
      phase = 'idle'
    }
  }

  let createdCount = $state(0)

  // Two passes: create every included ticket (collecting ids by proposal index),
  // then wire dependencies between the ones that were created. Each success
  // unticks its proposal, so a retry after a mid-loop failure only creates
  // what's still pending — never duplicates.
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
        proposals = proposals?.map((x, j) => (j === i ? { ...x, include: false } : x)) ?? null
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
      phase = 'done'
    }
  }

  const patch = (i: number, p: Partial<Proposal>) => {
    proposals = proposals?.map((x, j) => (j === i ? { ...x, ...p } : x)) ?? null
  }
</script>

<Modal {open} {onClose} title="Plan from this conversation" takeover={!!proposals} width="max-w-lg">
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
    {:else if phase === 'drafting'}
      <!-- The result replaces these: skeleton proposal cards, sized like the real ones. -->
      <div class="flex items-center gap-2 text-sm text-muted">
        <span class="font-medium text-fg">{picked?.label ?? 'The agent'}</span> is reading the conversation and
        drafting tickets{templateId || boardId ? ' on your template' : ''}
      </div>
      <div class="space-y-3">
        <Generating lines={2} />
        <Generating lines={3} />
        <Generating lines={2} />
      </div>
      <div class="flex justify-end border-t border-line-subtle pt-3">
        <Button variant="ghost" size="sm" onclick={onClose}>
          Cancel
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
      {#if note}
        <div class="text-sm" style:color="var(--theme-danger)">
          {note}
        </div>
      {/if}
      <div class="flex justify-end gap-2 border-t border-line-subtle pt-3">
        <Button variant="ghost" size="sm" onclick={onClose}>
          Cancel
        </Button>
        <Button size="sm" onclick={() => void draft()} disabled={!picked || !boardId}>
          Draft tickets
        </Button>
      </div>
    {:else}
      <div class="max-h-[65vh] space-y-3 overflow-y-auto pr-1" use:listStagger>
        {#each proposals as p, i (i)}
          <ProposalCard index={i} proposal={p} all={proposals} onPatch={(patchP) => patch(i, patchP)} />
        {/each}
      </div>
      <div class="flex items-center gap-2 border-t border-line-subtle pt-3">
        <span class="text-xs text-muted">
          → <span class="font-medium text-fg">{editable.find((b) => b.id === boardId)?.name}</span>
        </span>
        {#if note}
          <span class="text-xs" style:color="var(--theme-danger)">
            {note}
          </span>
        {/if}
        <span class="ml-auto"></span>
        <Button variant="ghost" size="sm" onclick={() => (proposals = null)}>
          Back
        </Button>
        <Button size="sm" onclick={() => void createAll()} disabled={phase === 'creating' || !boardId || included.length === 0}>
          {phase === 'creating'
            ? `Creating ${included.length} left`
            : `Create ${included.length} ticket${included.length === 1 ? '' : 's'}`}
        </Button>
      </div>
    {/if}
  </div>
</Modal>
