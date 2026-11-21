<script lang="ts">
  import Button from '@/components/ui/Button.svelte'
  import Checkbox from '@/components/ui/Checkbox.svelte'
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import Textarea from '@/components/ui/Textarea.svelte'
  import { confirm } from '@/components/ui/confirm.svelte'
  import { openTaskDiscussion, updateTask } from '@/lib/boards.svelte'
  import { sendChannelMessage } from '@/lib/channels.svelte'
  import { errorMessage, putJson } from '@/lib/fetch-json'
  import { approveConfirmMessage, askedLabel, evidenceLine, ticketHref, type DecidablePermissionRequest } from '@/lib/git-permission-requests'
  import { requestWord } from '@/lib/github-access'
  import { slide } from '@/lib/motion'
  import type { TaskStatus } from '@/lib/task-const'
  import { pushToast } from '@/lib/toast.svelte'
  import { useGithubAccessRefresh } from './queries'

  /** One GitHub permission an agent asked for (`request_git_permission`),
   *  decided beside what the agent holds and what the installation holds:
   *  approving adds it to the agent's set on that host (every repo it holds
   *  there), denying closes it, and either way the ticket gets a line and
   *  the agent's managers a notice. The thread post and Restart are this
   *  person's own moves, through the doors a person always uses: nothing
   *  here moves a ticket or starts an agent turn by itself. The post is
   *  checked by default, since it is how the agent hears the decision. A
   *  decision sends the level shown, so an agent that asked for more since
   *  gets a 409, never a wider grant. Admins only, like the route. */
  let { request: r }: { request: DecidablePermissionRequest } = $props()

  const refresh = useGithubAccessRefresh()
  let note = $state('')
  let postsNote = $state(true)
  let busy = $state(false)
  let error = $state<string | null>(null)
  const word = $derived(requestWord(r))

  /** The decider's note in the ticket's thread, as their own message. */
  const postNote = async (text: string) => {
    if (!r.ticket) return
    const { channelId } = await openTaskDiscussion(r.ticket.id)
    await sendChannelMessage(channelId, text)
  }

  const decide = async (action: 'approve' | 'deny') => {
    if (action === 'approve' && !(await confirm({ title: 'Approve this permission', message: approveConfirmMessage(r), confirmLabel: 'Approve' }))) return
    error = null
    busy = true
    const text = note.trim()
    try {
      const res = await putJson<{ ok: true; status: string }>('/api/workbench/permission-requests', {
        id: r.id,
        action,
        level: r.level,
        ...(text ? { note: text } : {}),
      })
      // A decided request leaves the list, and this card with it: the
      // outcome is said where it outlives the card.
      pushToast(
        res.status === 'granted'
          ? { title: 'Approved', body: `${r.agentName}'s set on ${r.hostName} now names ${askedLabel(r)}. Check token says what its next token carries.` }
          : res.status === 'awaiting_github'
            ? { title: 'Approved', body: `${r.agentName}'s ${askedLabel(r)} waits on GitHub: an owner of the account accepts it for the App.` }
            : { title: r.status === 'awaiting_github' ? 'Closed' : 'Denied', body: r.ticket ? 'The ticket says so.' : undefined },
      )
      if (text && postsNote && r.ticket) {
        try {
          await postNote(text)
        } catch (e) {
          pushToast({ title: 'Decided, but the note did not reach the ticket thread', body: errorMessage(e), tone: 'danger' })
        }
      }
      note = ''
    } catch (e) {
      error = errorMessage(e)
    }
    busy = false
    await refresh(r.agentId, [r.host])
  }

  /** Restart: the board's own human move, out of Blocked into the column
   *  work continues in. Only ever pressed by a person. */
  const restart = async () => {
    if (!r.ticket?.restartStatus) return
    error = null
    busy = true
    try {
      await updateTask(r.ticket.id, { status: r.ticket.restartStatus as TaskStatus })
      pushToast({ title: `${r.ticket.ref ?? 'The ticket'} moved out of Blocked.` })
    } catch (e) {
      error = errorMessage(e)
    }
    busy = false
    await refresh(r.agentId, [r.host])
  }
</script>

<div class="space-y-3 px-5 py-4 font-sans text-[13px] leading-5" data-testid="git-permission-request">
  <div class="flex flex-wrap items-center gap-x-3 gap-y-1">
    <span class="flex shrink-0 items-center gap-2 text-muted"><StatusDot status={word.tone} />{word.word}</span>
    <span class="min-w-0 text-fg">
      <span class="font-mono">{askedLabel(r)}</span> on {r.hostName}, where it worked on <span class="font-mono">{r.repo}</span>
    </span>
  </div>
  {#if r.why}
    <div>
      <div class="text-muted">The agent says</div>
      <!-- Plain text: the agent's words are data, never markup. -->
      <p class="whitespace-pre-wrap text-fg">{r.why}</p>
    </div>
  {/if}
  {#if r.evidence.length}
    <ul class="space-y-0.5 text-xs text-muted">
      {#each r.evidence as e, i (i)}<li>{evidenceLine(e)}</li>{/each}
    </ul>
  {/if}
  <div class="flex flex-wrap items-center gap-x-2 gap-y-1 text-muted">
    <span>Applies to <span class="font-mono text-fg">{r.appliesTo.join(', ') || r.repo}</span></span>
    {#if r.ticket}
      <span aria-hidden="true">·</span>
      <a class="text-accent underline-offset-2 hover:underline" href={ticketHref(r.ticket)}>{r.ticket.ref ?? 'The ticket'}{r.ticket.title ? `: ${r.ticket.title}` : ''}</a>
      <span aria-hidden="true">·</span>
      <span class="flex items-center gap-2" title="The ticket's column"><StatusDot status={r.ticket.blocked ? 'danger' : 'idle'} />{r.ticket.statusLabel}</span>
      {#if r.ticket.blocked && r.ticket.restartStatus}
        <Button size="sm" variant="outline" disabled={busy} onclick={() => void restart()} title="Move the ticket out of Blocked, as you would on its board">Restart</Button>
      {/if}
    {/if}
  </div>
  <p class="text-muted">
    {#if r.held === true}
      The installation holds it.
    {:else if r.held === false}
      The installation has not accepted it; an owner of the GitHub account accepts it
      {#if r.installationUrl}<a class="text-accent underline-offset-2 hover:underline" href={r.installationUrl} target="_blank" rel="noreferrer">on GitHub ↗</a>{:else}on GitHub{/if}.
      No token carries it until then.
    {:else}
      What the installation holds is not known yet.
    {/if}
  </p>
  <Textarea
    bind:value={note}
    rows={1}
    autoGrow
    maxlength={500}
    aria-label="Your note"
    placeholder={r.status === 'awaiting_github' ? 'Why close it? On the ticket (optional)' : 'Why? On the ticket and in list_repos for the agent (optional)'}
    class="w-full"
  />
  <div class="flex flex-wrap items-center gap-2">
    {#if r.ticket}
      <Checkbox
        checked={postsNote}
        onChange={(on) => (postsNote = on)}
        label="Post my note in the ticket thread"
        title="Your note as your own message in the ticket's discussion. The agent working it may answer, as it would any message there."
      />
    {/if}
    <span class="flex-1"></span>
    {#if r.status === 'pending'}
      <Button size="sm" variant="outline" disabled={busy} onclick={() => void decide('approve')}>Approve</Button>
      <Button size="sm" variant="ghost" disabled={busy} onclick={() => void decide('deny')}>Deny</Button>
    {:else}
      <Button
        size="sm"
        variant="ghost"
        disabled={busy}
        onclick={() => void decide('deny')}
        title="Stop waiting on GitHub. What is in the agent's permissions stays there; take it out below."
      >
        Close
      </Button>
    {/if}
  </div>
  {#if error}<p transition:slide={{ duration: 150 }} class="text-danger" role="alert">{error}</p>{/if}
</div>
