<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import { Trash2 } from '@lucide/svelte'
  import Button from '@/components/ui/Button.svelte'
  import DropdownMenu from '@/components/ui/DropdownMenu.svelte'
  import type { ContextMenuItem } from '@/components/ui/context-menu.svelte'
  import { confirm } from '@/components/ui/confirm.svelte'
  import Input from '@/components/ui/Input.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import { cn } from '@/lib/cn'
  import {
    useBoardLabels,
    createBoardLabel,
    updateBoardLabel,
    deleteBoardLabel,
    type Board,
    type LabelColor,
  } from '@/lib/boards.svelte'
  import { LABEL_CSS } from '@/components/board/field-pills'
  import ColorDot from './ColorDot.svelte'

  // ── Labels: the board's label registry — create, rename (cascades into
  //    tickets), recolor, delete (strips off tickets). ─────────────────────
  let { board }: { board: Board } = $props()

  const qc = useQueryClient()
  // A failed registry read used to render an empty label list under prose that
  // explains what labels do — read as "this board has none", which invites
  // creating duplicates of labels that already exist.
  const labelsQuery = useBoardLabels(() => board.id)
  const labels = $derived(labelsQuery.data ?? [])
  const canEdit = $derived(board.role === 'owner' || board.role === 'editor')
  let draft = $state('')
  const refresh = () => {
    void qc.invalidateQueries({ queryKey: ['board-labels', board.id] })
    void qc.invalidateQueries({ queryKey: ['board-tasks', board.id] })
  }

  const colorItems = (l: { id: string; color: LabelColor }): ContextMenuItem[] =>
    (Object.keys(LABEL_CSS) as LabelColor[]).map((c) => ({
      label: c,
      icon: [ColorDot, { class: 'h-2.5 w-2.5 rounded-full', color: LABEL_CSS[c] }],
      checked: l.color === c,
      onSelect: () => void updateBoardLabel(board.id, l.id, { color: c }).then(refresh),
    }))
</script>

<div class="space-y-3">
  <p class="font-sans text-xs text-muted">
    Labels are shared by everyone on this board. Renaming updates every ticket carrying the label; deleting removes
    it from tickets.
  </p>
  {#if labelsQuery.isError}
    <QueryError
      variant={labelsQuery.data === undefined ? 'compact' : 'inline'}
      title={labelsQuery.data === undefined ? 'Could not load labels' : 'Labels may be out of date'}
      error={labelsQuery.error}
      onRetry={() => void labelsQuery.refetch()}
    />
  {/if}
  <ul class="divide-y divide-line-subtle">
    {#each labels as l (l.id)}
      <li class="flex items-center gap-2 py-2">
        <DropdownMenu align="left" items={colorItems(l)}>
          {#snippet trigger(open)}
            <button
              title="Color"
              disabled={!canEdit}
              class={cn('h-4 w-4 shrink-0 rounded-full ring-2 transition-shadow', open ? 'ring-[var(--theme-accent-border)]' : 'ring-transparent')}
              style:background={LABEL_CSS[l.color]}
            ></button>
          {/snippet}
        </DropdownMenu>
        {#key `${l.id}-${l.name}`}
          <Input
            size="sm"
            value={l.name}
            disabled={!canEdit}
            onblur={(e) => {
              const v = (e.target as HTMLInputElement).value.trim()
              if (v && v !== l.name) void updateBoardLabel(board.id, l.id, { name: v }).then(refresh)
            }}
            onkeydown={(e) => e.key === 'Enter' && (e.target as HTMLInputElement).blur()}
            class="flex-1"
          />
        {/key}
        {#if canEdit}
          <button
            title="Delete label (removes it from tickets)"
            onclick={() =>
              void (async () => {
                if (await confirm({ title: `Delete label "${l.name}"?`, message: 'It is removed from every ticket carrying it.', danger: true })) {
                  await deleteBoardLabel(board.id, l.id)
                  refresh()
                }
              })()}
            class="shrink-0 text-muted transition-colors hover:text-danger"
          >
            <Trash2 size={14} />
          </button>
        {/if}
      </li>
    {/each}
    <!-- Only once the server actually SAID so — otherwise the error above
         and "No labels yet." sit on screen contradicting each other. -->
    {#if labels.length === 0 && labelsQuery.data !== undefined}
      <li class="py-3 font-sans text-xs text-muted">No labels yet.</li>
    {/if}
  </ul>
  <!-- Nothing to add against: the registry never loaded, so "Add" here is a
       coin flip on whether the label already exists. -->
  {#if canEdit && labelsQuery.data !== undefined}
    <div class="flex gap-2">
      <Input
        size="sm"
        bind:value={draft}
        placeholder="New label"
        onkeydown={(e) => {
          if (e.key !== 'Enter' || !draft.trim()) return
          void createBoardLabel(board.id, draft.trim()).then(() => {
            draft = ''
            refresh()
          })
        }}
        class="flex-1"
      />
      <Button
        size="sm"
        disabled={!draft.trim()}
        onclick={() =>
          void createBoardLabel(board.id, draft.trim()).then(() => {
            draft = ''
            refresh()
          })}
      >
        Add
      </Button>
    </div>
  {/if}
</div>
