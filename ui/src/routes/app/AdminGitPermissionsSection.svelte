<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import Button from '@/components/ui/Button.svelte'
  import Checkbox from '@/components/ui/Checkbox.svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import Textarea from '@/components/ui/Textarea.svelte'
  import { confirm } from '@/components/ui/confirm.svelte'
  import { openTaskDiscussion, updateTask } from '@/lib/boards.svelte'
  import { sendChannelMessage } from '@/lib/channels.svelte'
  import { errorMessage, getList, putJson } from '@/lib/fetch-json'
  import {
    approveConfirmMessage,
    askedLabel,
    evidenceLine,
    heldLabel,
    isDecidable,
    statusLabel,
    ticketHref,
    type DecidablePermissionRequest,
    type PermissionRequest,
  } from '@/lib/git-permission-requests'
  import type { TaskStatus } from '@/lib/task-const'
  import { slide } from '@/lib/motion'

  /** The GitHub permissions agents asked for (`request_git_permission`):
   *  approving adds the permission to the agent's set on that host (every
   *  repo it holds there), denying closes it, and either way the ticket gets
   *  a line and the agent's managers a notice. A request raised on a board
   *  this admin is not a member of shows only that it waits: the agent's
   *  words quote that board's work, and the server never sends them. The
   *  thread post and Restart are this person's own moves from here, through
   *  the doors a person always uses: nothing here moves a ticket or starts
   *  an agent turn by itself. */
  const qc = useQueryClient()
  const requestsQuery = createQuery(() => ({
    queryKey: ['permission-requests'],
    queryFn: (): Promise<PermissionRequest[]> =>
      getList<PermissionRequest>('/api/workbench/permission-requests', 'requests'),
    refetchInterval: 60_000,
  }))
  const requests = $derived(requestsQuery.data ?? [])

  let notes = $state<Record<string, string>>({})
  let threadPost = $state<Record<string, boolean>>({})
  let busy = $state<string | null>(null)
  let error = $state<string | null>(null)
  let said = $state<string | null>(null)

  const refresh = () => qc.invalidateQueries({ queryKey: ['permission-requests'] })

  /** The decider's note in the ticket's thread, as their own message: the
   *  room is the ticket's, the post is the chat door's, and the agent working
   *  it may answer, as it would any message there. */
  const postNote = async (r: DecidablePermissionRequest, note: string) => {
    if (!r.ticket) return
    const { channelId } = await openTaskDiscussion(r.ticket.id)
    await sendChannelMessage(channelId, note)
  }

  const decide = async (r: DecidablePermissionRequest, action: 'approve' | 'deny') => {
    if (action === 'approve' && !(await confirm({ title: 'Approve this permission', message: approveConfirmMessage(r), confirmLabel: 'Approve' }))) return
    error = null
    said = null
    busy = r.id
    const note = (notes[r.id] ?? '').trim()
    try {
      const res = await putJson<{ ok: true; status: string }>('/api/workbench/permission-requests', {
        id: r.id,
        action,
        ...(note ? { note } : {}),
      })
      said =
        res.status === 'granted'
          ? `${r.agentName} now carries ${askedLabel(r)} on ${r.hostName}.`
          : res.status === 'awaiting_github'
            ? `Approved. ${r.agentName}'s ${askedLabel(r)} waits on GitHub: an owner of the account accepts it for the App.`
            : `Closed. The ticket says so.`
      if (note && threadPost[r.id] && r.ticket) {
        try {
          await postNote(r, note)
        } catch (e) {
          error = `Decided, but the note did not reach the ticket thread: ${errorMessage(e)}`
        }
      }
      notes[r.id] = ''
    } catch (e) {
      error = errorMessage(e)
    }
    busy = null
    await refresh()
  }

  /** Restart: the board's own human move, out of Blocked into the column
   *  work continues in. Only ever pressed by a person. */
  const restart = async (r: DecidablePermissionRequest) => {
    if (!r.ticket?.restartStatus) return
    error = null
    busy = r.id
    try {
      await updateTask(r.ticket.id, { status: r.ticket.restartStatus as TaskStatus })
      said = `${r.ticket.ref ?? 'The ticket'} moved out of Blocked.`
    } catch (e) {
      error = errorMessage(e)
    }
    busy = null
    await refresh()
  }
</script>

<div class="space-y-2" data-testid="git-permission-requests">
  <div class="flex flex-wrap items-baseline gap-x-2">
    <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">GitHub permissions agents asked for</span>
    <span class="text-xs text-muted">approving adds it to the agent's permissions on that host, for every repo it holds there</span>
  </div>
  {#if requestsQuery.isError && requestsQuery.data === undefined}
    <!-- A failed poll must never read as "nothing waiting": an agent's
         request would sit unseen. -->
    <QueryError
      variant="inline"
      error={requestsQuery.error}
      title="Could not load the permission requests"
      onRetry={() => void requestsQuery.refetch()}
    />
  {:else if requestsQuery.data === undefined}
    <SkeletonRows rows={2} />
  {:else if requests.length === 0}
    <p class="text-xs text-muted">Nothing waiting on you.</p>
  {/if}
  {#each requests as r (r.id)}
    {#if isDecidable(r)}
      <div class="space-y-2 rounded-md border border-warning/30 px-3 py-2 text-sm" data-testid="git-permission-request">
        <div class="flex flex-wrap items-center gap-2">
          <span class="min-w-0 flex-1">
            <span class="text-fg">{r.agentName}</span>
            <span class="text-muted"> needs </span>
            <span class="font-mono text-[13px] text-fg">{askedLabel(r)}</span>
            <span class="font-mono text-[11px] text-muted"> on {r.hostName}, where it worked on {r.repo}</span>
          </span>
          <Chip tone={r.status === 'awaiting_github' ? 'warn' : 'accent'}>{statusLabel(r.status)}</Chip>
          <Chip tone={r.held === true ? 'success' : r.held === false ? 'warn' : 'neutral'} title="what the installation that mints this repo's token holds, from the last refresh">
            {heldLabel(r.held)}
          </Chip>
        </div>
        {#if r.held === false && r.installationUrl}
          <p class="text-xs text-muted">
            An owner of the GitHub account accepts it for the App on
            <a class="text-accent underline-offset-2 hover:underline" href={r.installationUrl} target="_blank" rel="noreferrer">the installation's settings page</a>; no token carries it until then.
          </p>
        {/if}
        {#if r.why}
          <div class="text-xs">
            <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">the agent says</span>
            <!-- Plain text: the agent's words are data, never markup. -->
            <p class="whitespace-pre-wrap font-sans text-fg">{r.why}</p>
          </div>
        {/if}
        {#if r.evidence.length}
          <ul class="space-y-0.5 text-[11px] text-muted">
            {#each r.evidence as e, i (i)}
              <li>{evidenceLine(e)}</li>
            {/each}
          </ul>
        {/if}
        <div class="flex flex-wrap items-center gap-2 text-xs text-muted">
          <span>applies to: <span class="font-mono text-fg">{r.appliesTo.join(', ') || r.repo}</span></span>
          {#if r.ticket}
            <span>·</span>
            <a class="text-accent underline-offset-2 hover:underline" href={ticketHref(r.ticket)}>{r.ticket.ref ?? 'the ticket'}{r.ticket.title ? `: ${r.ticket.title}` : ''}</a>
            <Chip tone={r.ticket.blocked ? 'danger' : 'neutral'} title="the ticket's column">{r.ticket.statusLabel}</Chip>
            {#if r.ticket.blocked && r.ticket.restartStatus}
              <Button size="xs" variant="outline" disabled={busy === r.id} onclick={() => void restart(r)} title="Move the ticket out of Blocked, as you would on its board">Restart</Button>
            {/if}
          {/if}
        </div>
        <Textarea
          bind:value={notes[r.id]}
          rows={1}
          autoGrow
          maxlength={500}
          placeholder={r.status === 'awaiting_github' ? 'Why close it? On the ticket (optional)' : 'Why? On the ticket and in list_repos for the agent (optional)'}
          class="w-full text-xs"
        />
        <div class="flex flex-wrap items-center gap-2">
          {#if r.ticket}
            <Checkbox
              checked={!!threadPost[r.id]}
              onChange={(on) => (threadPost[r.id] = on)}
              label="post my note in the ticket thread"
              title="Your note as your own message in the ticket's discussion. The agent working it may answer, as it would any message there."
            />
          {/if}
          <span class="flex-1"></span>
          {#if r.status === 'pending'}
            <Button size="sm" disabled={busy === r.id} onclick={() => void decide(r, 'approve')}>Approve</Button>
            <Button size="sm" variant="ghost" disabled={busy === r.id} onclick={() => void decide(r, 'deny')}>Deny</Button>
          {:else}
            <Button size="sm" variant="ghost" disabled={busy === r.id} onclick={() => void decide(r, 'deny')} title="Stop waiting on GitHub. What is in the agent's permissions stays there; take it out in the agent's editor.">Close</Button>
          {/if}
        </div>
      </div>
    {:else}
      <div class="rounded-md border border-line px-3 py-2 text-xs text-muted" data-testid="git-permission-fact">
        A GitHub permission request waits on {r.hostName}. It was raised on a board you are not a member of, so what the agent wrote is not shown here; an admin who is a member decides it.
      </div>
    {/if}
  {/each}
  {#if error}<div transition:slide={{ duration: 150 }} class="text-xs text-danger">{error}</div>{/if}
  {#if said}<div transition:slide={{ duration: 150 }} class="text-xs text-muted" data-testid="git-permission-decided">{said}</div>{/if}
</div>
