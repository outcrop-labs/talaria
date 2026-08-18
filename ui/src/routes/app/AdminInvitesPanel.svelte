<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import { Trash2 } from '@lucide/svelte'
  import Button from '@/components/ui/Button.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Panel from '@/components/ui/Panel.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import SectionHeader from '@/components/ui/SectionHeader.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import NoEmailBump from '@/components/setup/NoEmailBump.svelte'
  import { cn } from '@/lib/cn'
  import { getList } from '@/lib/fetch-json'
  import { slide } from '@/lib/motion'
  import { relativeTime } from '@/lib/fleet'

  /** Invites: the third door in. Create + send, watch state, revoke. */
  interface InviteRow {
    id: string
    email: string
    invitedBy: string | null
    createdAt: string
    expiresAt: string
    acceptedAt: string | null
    revokedAt: string | null
  }

  const qc = useQueryClient()
  // "No invites yet." is a claim about who has been let in. A failed read must
  // not make it — an admin would re-invite people who already hold live links,
  // or believe a revoked invite is gone when it is still open.
  const query = createQuery(() => ({
    queryKey: ['invites'],
    queryFn: (): Promise<InviteRow[]> => getList<InviteRow>('/api/admin/invites', 'invites'),
  }))
  const data = $derived(query.data)
  let draft = $state('')
  let error = $state<string | null>(null)
  let notice = $state<string | null>(null)
  let busy = $state(false)
  const refresh = () => qc.invalidateQueries({ queryKey: ['invites'] })

  const invite = async () => {
    if (!draft.trim()) return
    busy = true
    error = null
    notice = null
    const r = await fetch('/api/admin/invites', {
      method: 'POST',
      credentials: 'same-origin',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ email: draft.trim() }),
    })
    busy = false
    const j = (await r.json().catch(() => ({}))) as { error?: string; emailSent?: boolean; emailError?: string }
    if (!r.ok || j.error) {
      error = j.error ?? 'failed'
    } else {
      draft = ''
      notice = j.emailSent ? 'invite sent' : `invite created, but the email failed: ${j.emailError ?? 'no email provider configured'}`
    }
    await refresh()
  }
  // Named to dodge the `$state` rune: a local binding called `state` makes
  // every `$state(...)` in the file read as a store subscription.
  const inviteState = (i: InviteRow) =>
    i.acceptedAt ? 'accepted' : i.revokedAt ? 'revoked' : new Date(i.expiresAt) < new Date() ? 'expired' : 'pending'
</script>

<Panel class="mb-4">
  <SectionHeader
    title="Invites"
    info="Invite by email — they get a join link and are admitted the moment they sign in with Google on that address. Invites expire after two weeks; re-inviting re-issues a fresh link; revoking shuts the door instantly. Needs the Email provider on the Org tab."
  />
  <!-- Said before the send, not after it fails: an invite that goes nowhere
       reads as a bug in invites rather than a missing transport. -->
  <NoEmailBump class="mb-3" />
  <div class="mb-3 flex items-center gap-2">
    <Input
      size="sm"
      bind:value={draft}
      onkeydown={(e) => e.key === 'Enter' && void invite()}
      placeholder="teammate@yourcompany.com"
      class="w-72"
    />
    <Button size="sm" disabled={busy || !draft.trim()} onclick={() => void invite()}>
      {busy ? 'Sending' : 'Invite'}
    </Button>
    {#if notice}<span class="min-w-0 truncate text-xs text-muted">{notice}</span>{/if}
  </div>
  {#if error}<div transition:slide={{ duration: 150 }} class="mb-2 text-xs text-danger">{error}</div>{/if}
  {#if query.isPending}
    <SkeletonRows rows={2} />
  {:else if !data}
    <QueryError variant="inline" error={query.error} title="Could not load invites" onRetry={() => void query.refetch()} />
  {:else if data.length === 0}
    <EmptyState variant="inline" title="No invites yet." class="font-sans" />
  {:else}
    <ul class="divide-y divide-line-subtle">
      {#each data as i (i.id)}
        {@const st = inviteState(i)}
        <li class="-mx-2 flex items-center gap-3 rounded-md px-2 py-2 text-sm transition-colors dither-fill">
          <span class="min-w-0 flex-1 truncate text-fg">{i.email}</span>
          <span
            class={cn(
              'shrink-0 rounded border px-1.5 py-0.5 font-mono text-[10px] uppercase tracking-[0.05em]',
              st === 'accepted' && 'border-success/40 text-success',
              st === 'pending' && 'border-line text-muted',
              (st === 'revoked' || st === 'expired') && 'border-line-subtle text-muted/60 line-through',
            )}
          >
            {st}
          </span>
          <span class="shrink-0 font-mono text-[10px] tracking-[0.05em] text-muted">{relativeTime(i.createdAt)}</span>
          {#if st === 'pending'}
            <button
              type="button"
              title="Revoke — the link stops working immediately"
              onclick={async () => {
                await fetch('/api/admin/invites', {
                  method: 'DELETE',
                  credentials: 'same-origin',
                  headers: { 'content-type': 'application/json' },
                  body: JSON.stringify({ id: i.id }),
                })
                await refresh()
              }}
              class="shrink-0 text-muted transition-colors hover:text-danger"
            >
              <Trash2 size={13} />
            </button>
          {/if}
        </li>
      {/each}
    </ul>
  {/if}
</Panel>
