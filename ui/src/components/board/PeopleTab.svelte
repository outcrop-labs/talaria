<script lang="ts">
  import Button from '@/components/ui/Button.svelte'
  import { useQueryClient } from '@tanstack/svelte-query'
  import Select from '@/components/ui/Select.svelte'
  import Avatar from '@/components/ui/Avatar.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import UserPicker from '@/components/app/UserPicker.svelte'
  import { shareBoard, unshareBoard, useBoardMembers, type Board } from '@/lib/boards.svelte'
  import { errorMessage } from '@/lib/fetch-json'
  import { pushToast } from '@/lib/toast.svelte'
  import { listStagger } from '@/lib/motion'

  // The People tab of BoardSettingsModal.svelte.
  let { board }: { board: Board } = $props()

  const qc = useQueryClient()
  // This list IS the access-control answer. `= []` under it meant a 500 on
  // /api/boards/:id/members rendered "People with access" over nothing —
  // telling an owner the board is private to them, from a read that failed.
  const membersQuery = useBoardMembers(() => board.id)
  const members = $derived(membersQuery.data ?? [])
  const membersLoading = $derived(membersQuery.isLoading)
  let role = $state<'editor' | 'viewer'>('editor')
  let err = $state<string | null>(null)
  const refresh = () => qc.invalidateQueries({ queryKey: ['board-members', board.id] })

  const invite = async (email: string) => {
    err = null
    // shareBoard used to hand back a raw Response for this parse — now the
    // door rejects with the server's sentence and this is just the catch.
    try {
      await shareBoard(board.id, email, role)
    } catch (e) {
      err = errorMessage(e)
      return
    }
    void refresh()
  }
</script>

<div>
  <div class="flex items-center gap-2">
    <UserPicker
      class="min-w-0 flex-1"
      size="sm"
      exclude={members.map((m) => m.userId)}
      onPick={(u) => {
        if (u.email) void invite(u.email)
      }}
    />
    <Select bind:value={role} size="sm" class="shrink-0">
      <option value="editor">Editor</option>
      <option value="viewer">Viewer</option>
    </Select>
  </div>
  {#if err}<div class="mt-2 font-sans text-xs text-danger">{err}</div>{/if}

  <div class="mt-4 space-y-1" use:listStagger>
    <div class="mb-1 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">People with access</div>
    {#if membersLoading}<SkeletonRows rows={3} avatar class="px-1 py-1.5" />{/if}
    {#if membersQuery.isError}
      <QueryError
        variant={membersQuery.data === undefined ? 'compact' : 'inline'}
        class="px-1 py-1.5"
        title={membersQuery.data === undefined ? 'Could not load who has access' : 'This list may be out of date'}
        error={membersQuery.error}
        onRetry={() => void membersQuery.refetch()}
      />
    {/if}
    {#each members as m (m.userId)}
      <div class="flex items-center gap-2 rounded-md px-1 py-1.5">
        <Avatar name={m.name ?? m.email} class="h-7 w-7" />
        <span class="min-w-0 flex-1 truncate font-sans text-sm text-fg">
          {m.name ?? m.email ?? m.userId}
          {#if m.name && m.email}<span class="ml-1.5 font-mono text-[11px] text-muted">{m.email}</span>{/if}
        </span>
        <span class="font-mono text-[10px] uppercase tracking-[0.05em] text-muted">{m.role}</span>
        {#if m.role !== 'owner'}
          <Button
            variant="ghost"
            size="xs"
            class="hover:text-danger"
            onclick={() =>
              void unshareBoard(board.id, m.userId)
                .then(refresh)
                .catch((e) => pushToast({ title: 'Could not remove member', body: errorMessage(e), tone: 'danger' }))}
          >
            Remove
          </Button>
        {/if}
      </div>
    {/each}
  </div>
</div>
