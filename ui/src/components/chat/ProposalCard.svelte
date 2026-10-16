<script lang="ts">
  // One proposal, room to breathe: title + meta on top, full-width description,
  // and a "blocked by" row referencing sibling proposals (created as real
  // ticket dependencies).
  //
  // Three of the fields are the drafter's reading of the DISCUSSION rather than
  // of the ticket, and each one is a claim a reviewer has to be able to see and
  // undo here: the epic a proposal hangs under, the people it was pointed at,
  // and an open ticket it may be re-proposing.
  import Checkbox from '@/components/ui/Checkbox.svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Select from '@/components/ui/Select.svelte'
  import RichEditor from '@/components/ui/RichEditor.svelte'
  import { isEpic, parentIndex, type Proposal } from './plan-modal'
  // `p` is the proposal here, so the route builder comes in under its purpose.
  import { p as routePath } from '@/router'
  import type { BoardMember } from '@/lib/boards.svelte'
  import { humanAssigneeId, isHumanAssignee, type Effort, type Priority, type Task } from '@/lib/task-const'

  let {
    index,
    proposal: p,
    all,
    boardId,
    members,
    boardTasks,
    onPatch,
  }: {
    index: number
    proposal: Proposal
    all: Proposal[]
    /** The board these drafts are bound for — the duplicate's link needs it. */
    boardId: string
    /** Names for the user ids the drafter assigned; empty until (or unless)
     *  the board's member read lands. */
    members: BoardMember[]
    /** The board's open tickets, for resolving `duplicateOf` to a real one. */
    boardTasks: Task[]
    onPatch: (patch: Partial<Proposal>) => void
  } = $props()

  const short = (s: string, n = 36) => (s.length > n ? `${s.slice(0, n)}…` : s)
  const addable = $derived(
    all.map((x, j) => ({ x, j })).filter(({ j }) => j !== index && !p.dependsOn.includes(j)),
  )

  // Both read the batch, because both are relationships rather than fields:
  // this proposal is an epic if others name it, and a child if the one it
  // names survived the review.
  const epic = $derived(isEpic(all, index))
  const parent = $derived(parentIndex(all, index))

  // The drafter writes board member user ids; a created ticket spells the same
  // person `user:<id>`, so accept both and fall back to the raw id — an id on
  // screen is ugly, a wrong name is a lie about who was asked.
  const assigneeLabel = (a: string) => {
    const id = isHumanAssignee(a) ? humanAssigneeId(a) : a
    const m = members.find((x) => x.userId === id)
    return m?.name ?? m?.email ?? a
  }

  const duplicate = $derived(p.duplicateOf ? boardTasks.find((t) => t.id === p.duplicateOf) : undefined)
  // Named if the ticket read resolved it, honest if it did not: the claim is
  // worth showing either way, since the drafter saw the ticket even when this
  // client has not loaded the board.
  const duplicateLabel = $derived(
    duplicate ? [duplicate.ticketRef, duplicate.title].filter(Boolean).join(' ') : 'an open ticket',
  )
  // The draft lives on the server, so following the link costs nothing — the
  // header button on the conversation reopens the review where it stood.
  const duplicateHref = $derived(
    duplicate && boardId
      ? routePath('/boards/:boardId/:taskId', { params: { boardId, taskId: duplicate.id } })
      : null,
  )

  const priorities: Priority[] = ['low', 'medium', 'high', 'urgent']
  const efforts: Effort[] = ['xs', 's', 'm', 'l', 'xl']
</script>

<div class={`rounded-lg border border-line bg-panel p-4 ${p.include ? '' : 'opacity-50'}`}>
  <div class="flex items-center gap-2">
    <Checkbox
      bare
      title="Include this proposal"
      checked={p.include}
      onChange={(checked) => onPatch({ include: checked })}
      class="shrink-0"
    />
    <span class="w-7 shrink-0 text-right text-xs text-muted">#{index + 1}</span>
    {#if epic}
      <Chip tone="accent" title="Other drafts in this batch are created under this one">Epic</Chip>
    {/if}
    <Input size="sm" value={p.title} oninput={(e) => onPatch({ title: e.currentTarget.value })} class="flex-1" />
    <Select value={p.priority} size="sm" onchange={(e) => onPatch({ priority: e.currentTarget.value as Priority })} class="w-24 shrink-0">
      {#each priorities as x (x)}
        <option value={x}>
          {x}
        </option>
      {/each}
    </Select>
    <Select
      value={p.effort ?? ''}
      size="sm"
      onchange={(e) => onPatch({ effort: (e.currentTarget.value || null) as Effort | null })}
      class="w-20 shrink-0"
    >
      <option value="">—</option>
      {#each efforts as x (x)}
        <option value={x}>
          {x.toUpperCase()}
        </option>
      {/each}
    </Select>
  </div>
  {#if p.duplicateOf}
    <!-- Outside the include gate, because a duplicate arrives DROPPED: the
         line is the reason the box above is unticked, and ticking it is the
         reviewer saying this is new work after all. -->
    <p class="mt-2 text-xs text-warning">
      Possible duplicate of
      {#if duplicateHref}
        <a href={duplicateHref} class="underline decoration-dotted">{duplicateLabel}</a>
      {:else}
        {duplicateLabel}
      {/if}
      — include it anyway if this is really separate work.
    </p>
  {/if}
  {#if p.include}
    <!-- Draft descriptions are ticket markdown — edit them the way the
         ticket will show them. Seeded per draft; autosave patches up. -->
    <div class="mt-3 max-h-64 overflow-y-auto">
      <RichEditor value={p.description} onSave={(md) => onPatch({ description: md })} autosave minHeight="8rem" />
    </div>
    {#if parent !== null}
      <div class="mt-2 flex flex-wrap items-center gap-1.5">
        <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Part of</span>
        <Chip onRemove={() => onPatch({ parent: null })}>
          {short(all[parent]?.title ?? '')}
        </Chip>
      </div>
    {/if}
    {#if p.assignees.length > 0}
      <!-- Who the discussion pointed this at, removable one by one. There is no
           picker: adding people is the ticket's own job, and a draft nobody
           has created yet cannot answer who is on the board. -->
      <div class="mt-2 flex flex-wrap items-center gap-1.5">
        <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Assignees</span>
        {#each p.assignees as a (a)}
          <Chip onRemove={() => onPatch({ assignees: p.assignees.filter((x) => x !== a) })}>
            {assigneeLabel(a)}
          </Chip>
        {/each}
      </div>
    {/if}
    <div class="mt-2 flex flex-wrap items-center gap-1.5">
      <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Blocked by</span>
      {#each p.dependsOn as d (d)}
        <span class="inline-flex items-center gap-1 rounded-md border border-line bg-raised px-2 py-0.5 text-xs text-fg">
          #{d + 1} {short(all[d]?.title ?? '')}
          <button
            type="button"
            class="text-muted hover:text-fg"
            onclick={() => onPatch({ dependsOn: p.dependsOn.filter((x) => x !== d) })}
            aria-label={`remove dependency on #${d + 1}`}
          >
            ×
          </button>
        </span>
      {/each}
      {#if addable.length > 0}
        <Select
          value=""
          size="sm"
          onchange={(e) => {
            const d = Number(e.currentTarget.value)
            if (Number.isInteger(d)) onPatch({ dependsOn: [...p.dependsOn, d] })
            // The React version was controlled back to ""; reset by hand here.
            e.currentTarget.value = ''
          }}
          class="w-40"
        >
          <option value="">+ add</option>
          {#each addable as { x, j } (j)}
            <option value={j}>
              #{j + 1} {short(x.title, 28)}
            </option>
          {/each}
        </Select>
      {/if}
      {#if p.dependsOn.length === 0 && addable.length === 0}<span class="text-xs text-muted">—</span>{/if}
    </div>
  {/if}
</div>
