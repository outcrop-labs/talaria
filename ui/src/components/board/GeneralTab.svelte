<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import Button from '@/components/ui/Button.svelte'
  import DangerLink from '@/components/ui/DangerLink.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Select from '@/components/ui/Select.svelte'
  import {
    archiveBoard,
    deleteBoard,
    moveBoardWithPreview,
    renameBoard,
    setBoardJudgeMode,
    type Board,
  } from '@/lib/boards.svelte'
  import { useTeamsDirectory } from '@/lib/teams'
  import { pushToast, toastError } from '@/lib/toast.svelte'
  import TemplatesSection from './TemplatesSection.svelte'

  // The General tab of BoardSettingsModal.svelte (module-private there in
  // React; its own file here because Svelte is one component per file).
  let {
    board,
    isOwner,
    onClose,
    onArchived,
    onDeleted,
  }: {
    board: Board
    isOwner: boolean
    onClose: () => void
    onArchived: () => void
    onDeleted: () => void
  } = $props()

  const qc = useQueryClient()
  // svelte-ignore state_referenced_locally -- reason: settings-modal instance per open; drafts seeded from the board, Save writes the record
  let name = $state(board.name)
  let confirmDelete = $state(false)
  // svelte-ignore state_referenced_locally -- reason: same one-time seed as the name draft; the modal instance is born from one board row
  let teamChoice = $state(board.teamId ?? '')
  const archived = $derived(!!board.archivedAt)
  const refreshBoards = () => qc.invalidateQueries({ queryKey: ['boards'] })
  // A getter, not the bare prop: isOwner cannot change for a modal
  // instance, but the directory read still follows the prop honestly.
  const teamsDirectory = useTeamsDirectory(() => isOwner)
  const boardIsTeamBoard = $derived(board.teamId != null)
  const currentTeamName = $derived(
    board.teamName ?? teamsDirectory.data?.find((t) => t.id === board.teamId)?.name ?? 'Personal',
  )

  const commitName = async () => {
    const n = name.trim()
    if (!n || n === board.name) {
      name = board.name
      return
    }
    try {
      await renameBoard(board.id, n)
    } catch (e) {
      // Rename rejects now; the field would otherwise snap back on refetch
      // with no sentence, which is the silent failure this sweep removes.
      toastError('Rename failed', e)
      return
    }
    void refreshBoards()
  }

  // Team reassignment (owner only): the confirm dialog shows who would lose
  // sight of the board BEFORE the move applies; cancel restores the choice.
  const commitTeam = async () => {
    const target = teamChoice || null
    if ((target ?? null) === (board.teamId ?? null)) return
    const label = target
      ? (teamsDirectory.data?.find((t) => t.id === target)?.name ?? 'the selected team')
      : 'Personal'
    const r = await moveBoardWithPreview(board.id, target, label)
    if (r?.error) {
      // r.error is the server's own refusal sentence (e.g. the
      // destination-membership rule) — body it verbatim, don't route it
      // through toastError, which expects a thrown unknown.
      pushToast({ title: 'Could not move board', body: r.error, tone: 'danger' })
      teamChoice = board.teamId ?? ''
      return
    }
    void refreshBoards()
  }
</script>

<div class="space-y-5">
  <div>
    <!-- svelte-ignore a11y_label_has_associated_control -->
    <label class="mb-1 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Name</label>
    <Input
      bind:value={name}
      onblur={() => void commitName()}
      onkeydown={(e) => e.key === 'Enter' && (e.target as HTMLInputElement).blur()}
      class="w-full"
    />
  </div>

  <div>
    <!-- svelte-ignore a11y_label_has_associated_control -->
    <label class="mb-1 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">QA judge</label>
    <Select
      value={board.judgeMode ?? 'inherit'}
      onchange={async (e) => {
        try {
          await setBoardJudgeMode(board.id, e.currentTarget.value as 'inherit' | 'off' | 'advisory' | 'enforcing')
        } catch (err) {
          toastError('Could not change judge mode', err)
          return
        }
        void refreshBoards()
      }}
      class="w-full"
    >
      <option value="inherit">Default (follow the org setting)</option>
      <option value="off">Off: no automated review</option>
      <option value="advisory">Advisory: judge posts a verdict, human decides</option>
      <option value="enforcing">Enforcing: auto-send failing work back to the agent (up to 3×), then a human</option>
    </Select>
    <div class="mt-1 font-sans text-[11px] text-muted">
      When an agent hands a ticket to quality review, the judge reviews it. Enforcing bounces “revise” verdicts back to the agent with the issues before a human sees them.
    </div>
  </div>

  {#if isOwner}
    <div>
      <label class="mb-1 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim" for="board-team-select">Team</label>
      <Select
        id="board-team-select"
        value={teamChoice}
        onchange={() => void commitTeam()}
        class="w-full"
      >
        <option value="">Personal (no team)</option>
        {#each teamsDirectory.data ?? [] as t (t.id)}
          <option value={t.id}>{t.name}</option>
        {/each}
      </Select>
      <div class="mt-1 font-sans text-[11px] text-muted">
        {#if boardIsTeamBoard}
          Currently “{currentTeamName}”. Moving the board changes who can see it — the next dialog shows who loses access, and you must be a member of the team you move it to.
        {:else}
          Currently Personal. Moving the board into a team gives that team sight of it, and you must be a member of the destination team.
        {/if}
      </div>
    </div>
  {/if}

  <TemplatesSection {board} />

  <!-- DANGER ZONE (spec §8): orange hairline panel, orange mono label +
       right-aligned mono meta, muted body, destructive outline actions. -->
  <div class="rounded-lg border border-danger/40 p-3">
    <div class="mb-2 flex items-center font-mono text-[10px] uppercase tracking-[0.08em]">
      <span class="text-danger">Danger zone</span>
      <span class="ml-auto text-ink-dim">Delete is irreversible</span>
    </div>
    <div class="flex items-center justify-between gap-3">
      <div class="min-w-0">
        <div class="font-sans text-sm text-fg">{archived ? 'Restore board' : 'Archive board'}</div>
        <div class="font-sans text-xs text-muted">
          {archived ? 'Make it active and visible again.' : 'Hide it from the sidebar and boards list. Reversible.'}
        </div>
      </div>
      <Button
        variant="outline"
        size="sm"
        onclick={async () => {
          try {
            await archiveBoard(board.id, !archived)
          } catch (e) {
            toastError(archived ? 'Could not restore board' : 'Could not archive board', e)
            return
          }
          void refreshBoards()
          void qc.invalidateQueries({ queryKey: ['boards', 'archived'] })
          onClose()
          if (!archived) onArchived()
        }}
      >
        {archived ? 'Restore' : 'Archive'}
      </Button>
    </div>

    {#if isOwner}
      <div class="mt-3 flex items-center justify-between gap-3 border-t border-line-subtle pt-3">
        <div class="min-w-0">
          <div class="font-sans text-sm text-fg">Delete board</div>
          <div class="font-sans text-xs text-muted">Permanently removes the board and all its tickets.</div>
        </div>
        {#if confirmDelete}
          <div class="flex shrink-0 gap-2">
            <Button variant="ghost" size="sm" onclick={() => (confirmDelete = false)}>
              Cancel
            </Button>
            <Button
              variant="danger"
              size="sm"
              onclick={async () => {
                try {
                  await deleteBoard(board.id)
                } catch (e) {
                  toastError('Delete failed', e)
                  return
                }
                void refreshBoards()
                onClose()
                onDeleted()
              }}
            >
              Confirm delete
            </Button>
          </div>
        {:else}
          <DangerLink class="shrink-0" onClick={() => (confirmDelete = true)}>
            Delete board
          </DangerLink>
        {/if}
      </div>
    {/if}
  </div>
</div>
