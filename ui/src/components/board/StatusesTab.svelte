<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import { GripVertical, Trash2 } from '@lucide/svelte'
  import Button from '@/components/ui/Button.svelte'
  import DropdownMenu from '@/components/ui/DropdownMenu.svelte'
  import type { ContextMenuItem } from '@/components/ui/context-menu.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Select from '@/components/ui/Select.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import { cn } from '@/lib/cn'
  import type { Board } from '@/lib/boards.svelte'
  import { LABEL_CSS } from '@/components/board/field-pills'
  import {
    useBoardStatuses,
    createBoardStatus,
    updateBoardStatus,
    reorderBoardStatuses,
    deleteBoardStatus,
    type BoardStatus,
  } from '@/lib/statuses'
  import { listStagger } from '@/lib/motion'
  import ColorDot from './ColorDot.svelte'
  import WorkflowDiagnostics from './WorkflowDiagnostics.svelte'

  // ── Statuses: the board's workflow columns. Order = column order; category
  //    carries the semantics; agentStart = "agents may pick up work here".
  //    Blocked is system — pinned, not editable. ─────────────────────────────
  let { board }: { board: Board } = $props()

  const qc = useQueryClient()
  // Worse here than anywhere: an empty list under "Columns and their meaning"
  // reads as a board with no workflow at all, and the Add box beside it will
  // happily create a duplicate of a status the server already has.
  const statusesQuery = useBoardStatuses(() => board.id)
  const statuses = $derived(statusesQuery.data ?? [])
  const canEdit = $derived(board.role === 'owner' || board.role === 'editor')
  let draft = $state('')
  let err = $state<string | null>(null)
  const refresh = () => {
    void qc.invalidateQueries({ queryKey: ['board-statuses', board.id] })
    void qc.invalidateQueries({ queryKey: ['board-status-diagnostics', board.id] })
    // Recategorising a populated review or done column MOVES its tickets into a
    // surviving column of the same category, so the board itself changed.
    void qc.invalidateQueries({ queryKey: ['board-tasks', board.id] })
  }
  const editable = $derived(statuses.filter((st) => !st.system))
  const run = (fn: () => Promise<unknown>) => {
    err = null
    void fn()
      .then(refresh)
      .catch((e: Error) => (err = e.message))
  }
  // Drag-to-reorder — the same grammar as column reordering (grip handle,
  // before/after indicator, drop commits). Blocked is system: not draggable,
  // not a target (the server places it automatically).
  let dragKey = $state<string | null>(null)
  let overKey = $state<string | null>(null)
  let overPos = $state<'before' | 'after'>('before')
  const dropStatus = (target: BoardStatus) => {
    if (dragKey && dragKey !== target.key && !target.system) {
      const next = editable.map((x) => x.key).filter((k) => k !== dragKey)
      const idx = next.indexOf(target.key) + (overPos === 'after' ? 1 : 0)
      next.splice(idx, 0, dragKey)
      run(() => reorderBoardStatuses(board.id, next))
    }
    dragKey = null
    overKey = null
  }

  const colorItems = (st: BoardStatus): ContextMenuItem[] =>
    (Object.keys(LABEL_CSS) as Array<keyof typeof LABEL_CSS>).map((c) => ({
      label: c,
      icon: [ColorDot, { class: 'h-2.5 w-2.5 rounded-full', color: LABEL_CSS[c] }],
      checked: st.color === c,
      onSelect: () => run(() => updateBoardStatus(board.id, st.key, { color: c })),
    }))

  const deleteItems = (st: BoardStatus): ContextMenuItem[] =>
    statuses
      .filter((o) => o.key !== st.key && !o.system)
      .map((o) => ({
        label: `Move tickets to ${o.label}`,
        onSelect: () => run(() => deleteBoardStatus(board.id, st.key, o.key)),
      }))
</script>

<div class="space-y-3">
  <p class="font-sans text-xs text-muted">
    Columns and their meaning. <strong>Category</strong> drives the workflow: intake statuses receive new tickets,
    review is where agent work lands for sign-off, done completes. <strong>Agent start</strong> marks the columns
    where assignment counts as approval: agents only pick up work sitting there. Blocked is always present.
  </p>
  <!-- The one rule that is not visible from the controls: a review column is
       the agent hand-off target, so it is required only while agents are
       allowed here. Saying it beside the Category control is what stops the
       refusal on the last review column reading as an arbitrary rule — and
       tells the owner of a human-only board that the sign-off step is
       theirs to drop. -->
  <p class="font-sans text-xs text-muted">
    Every board needs an intake column and a done column. A <strong>review</strong> column is required only while
    agents are allowed on this board (Agents tab): an agent may not sign off its own work, so it hands finished
    tickets there for a person. A board that runs no agents can go straight from active to done.
  </p>
  <WorkflowDiagnostics boardId={board.id} />
  <!-- Every refusal from the statuses route lands here: the server sends
       `{ error }`, lib/statuses rejects with that sentence, and `run` shows
       it verbatim. They are written as instructions ("clear agent start on
       this column"), so quoting them beats paraphrasing. -->
  {#if err}<div class="rounded-lg border border-danger/40 p-2.5 font-sans text-xs text-fg">{err}</div>{/if}
  {#if statusesQuery.isError}
    <QueryError
      variant={statusesQuery.data === undefined ? 'compact' : 'inline'}
      title={statusesQuery.data === undefined ? 'Could not load this board’s columns' : 'Columns may be out of date'}
      error={statusesQuery.error}
      onRetry={() => void statusesQuery.refetch()}
    />
  {/if}
  <ul class="divide-y divide-line-subtle" use:listStagger>
    {#each statuses as st (st.key)}
      <li
        draggable={canEdit && !st.system}
        ondragstart={(e) => {
          if (e.dataTransfer) e.dataTransfer.effectAllowed = 'move'
          dragKey = st.key
        }}
        ondragend={() => {
          dragKey = null
          overKey = null
        }}
        ondragover={(e) => {
          if (!dragKey || st.system) return
          e.preventDefault()
          const rect = e.currentTarget.getBoundingClientRect()
          overKey = st.key
          overPos = e.clientY > rect.top + rect.height / 2 ? 'after' : 'before'
        }}
        ondrop={(e) => {
          e.preventDefault()
          dropStatus(st)
        }}
        class={cn('relative flex items-center gap-2 py-2', st.system && 'opacity-70', dragKey === st.key && 'opacity-40')}
      >
        {#if overKey === st.key && dragKey !== st.key}
          <span
            class={cn(
              'pointer-events-none absolute inset-x-0 h-0.5 rounded-full bg-accent',
              overPos === 'before' ? '-top-px' : '-bottom-px',
            )}
          ></span>
        {/if}
        <GripVertical
          size={13}
          class={cn('shrink-0', canEdit && !st.system ? 'cursor-grab text-muted' : 'text-muted/30')}
        />
        <DropdownMenu align="left" items={colorItems(st)}>
          {#snippet trigger(open)}
            <button
              title="Color"
              disabled={!canEdit || st.system}
              class={cn('h-4 w-4 shrink-0 rounded-full ring-2 transition-shadow', open ? 'ring-[var(--theme-accent-border)]' : 'ring-transparent')}
              style:background={LABEL_CSS[st.color as keyof typeof LABEL_CSS] ?? 'var(--theme-muted)'}
            ></button>
          {/snippet}
        </DropdownMenu>
        {#if st.system}
          <span class="flex-1 font-sans text-sm text-fg">Blocked <span class="ml-1 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">system</span></span>
        {:else}
          {#key `${st.key}-${st.label}`}
            <Input
              size="sm"
              value={st.label}
              disabled={!canEdit}
              onblur={(e) => {
                const v = (e.target as HTMLInputElement).value.trim()
                if (v && v !== st.label) run(() => updateBoardStatus(board.id, st.key, { label: v }))
              }}
              onkeydown={(e) => e.key === 'Enter' && (e.target as HTMLInputElement).blur()}
              class="flex-1"
            />
          {/key}
        {/if}
        {#if !st.system}
          <Select
            size="sm"
            value={st.category}
            disabled={!canEdit}
            title="Workflow category"
            onchange={(e) => {
              // A sign-off column (review / done) may not also be an
              // agent-start pickup queue, so on a column carrying the flag
              // this control alone is a write the server can only refuse.
              // It sends BOTH halves: the operator picked "review", and a
              // review column is by definition not one agents pick up from
              // — there is exactly one legal outcome and no guess in it.
              // The checkbox beside this select shows the result.
              const value = e.currentTarget.value
              run(() =>
                updateBoardStatus(board.id, st.key, {
                  category: value,
                  ...(st.agentStart && (value === 'review' || value === 'done')
                    ? { agentStart: false }
                    : {}),
                }),
              )
            }}
            class="w-28 shrink-0"
          >
            <option value="open">intake</option>
            <option value="active">active</option>
            <option value="review">review</option>
            <option value="done">done</option>
          </Select>
          <label
            title="Agents may pick up work in this column (assignment here = approval to start)"
            class="flex shrink-0 cursor-pointer items-center gap-1 font-mono text-[10px] uppercase tracking-[0.05em] text-muted"
          >
            <input
              type="checkbox"
              checked={st.agentStart}
              disabled={!canEdit}
              onchange={(e) => run(() => updateBoardStatus(board.id, st.key, { agentStart: e.currentTarget.checked }))}
              class="accent-[var(--theme-accent)]"
            />
            agent start
          </label>
          {#if canEdit}
            <DropdownMenu align="right" items={deleteItems(st)}>
              {#snippet trigger()}
                <button title="Delete status (tickets move to another column)" class="shrink-0 text-muted transition-colors hover:text-danger">
                  <Trash2 size={14} />
                </button>
              {/snippet}
            </DropdownMenu>
          {/if}
        {/if}
      </li>
    {/each}
  </ul>
  <!-- Same rule as Labels: adding a column while the column set is unknown
       risks duplicating one the server already has. -->
  {#if canEdit && statusesQuery.data !== undefined}
    <div class="flex gap-2">
      <Input
        size="sm"
        bind:value={draft}
        placeholder="New status"
        onkeydown={(e) => {
          if (e.key !== 'Enter' || !draft.trim()) return
          run(() => createBoardStatus(board.id, { label: draft.trim() }))
          draft = ''
        }}
        class="flex-1"
      />
      <Button
        size="sm"
        disabled={!draft.trim()}
        onclick={() => {
          run(() => createBoardStatus(board.id, { label: draft.trim() }))
          draft = ''
        }}
      >
        Add
      </Button>
    </div>
  {/if}
</div>
