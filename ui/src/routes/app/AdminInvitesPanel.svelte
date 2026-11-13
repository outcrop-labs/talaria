<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import { Mail } from '@lucide/svelte'
  import SettingsField from '@/components/settings/SettingsField.svelte'
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import SettingsRowMenu from '@/components/settings/SettingsRowMenu.svelte'
  import SettingsSection from '@/components/settings/SettingsSection.svelte'
  import Button from '@/components/ui/Button.svelte'
  import type { DotStatus } from '@/components/ui/chip'
  import Input from '@/components/ui/Input.svelte'
  import Modal from '@/components/ui/Modal.svelte'
  import QueryState from '@/components/ui/QueryState.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import NoEmailBump from '@/components/setup/NoEmailBump.svelte'
  import { delJson, errorMessage, getList, postJson } from '@/lib/fetch-json'
  import { slide } from '@/lib/motion'
  import { relativeTime } from '@/lib/fleet'
  import RowStatus from '@/components/admin/RowStatus.svelte'

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

  /** The invite dialog. The page header's "Invite people" button opens it. */
  let { inviting = $bindable(false) }: { inviting?: boolean } = $props()

  const qc = useQueryClient()
  // "No invites yet." is a claim about who has been let in. A failed read must
  // not make it — an admin would re-invite people who already hold live links,
  // or believe a revoked invite is gone when it is still open.
  const query = createQuery(() => ({
    queryKey: ['invites'],
    queryFn: (): Promise<InviteRow[]> => getList<InviteRow>('/api/admin/invites', 'invites'),
  }))
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
    try {
      const j = await postJson<{ emailSent?: boolean; emailError?: string }>('/api/admin/invites', { email: draft.trim() })
      notice = j.emailSent
        ? `Invite sent to ${draft.trim()}.`
        : `Invite created, but the email failed: ${j.emailError ?? 'no email provider is set up'}`
      draft = ''
      inviting = false
    } catch (e) {
      error = errorMessage(e)
    }
    busy = false
    await refresh()
  }
  const revoke = async (id: string) => {
    try {
      await delJson<{ ok: true }>('/api/admin/invites', { id })
    } catch (e) {
      error = errorMessage(e)
    }
    await refresh()
  }
  // Named to dodge the `$state` rune: a local binding called `state` makes
  // every `$state(...)` in the file read as a store subscription.
  const inviteState = (i: InviteRow) =>
    i.acceptedAt ? 'accepted' : i.revokedAt ? 'revoked' : new Date(i.expiresAt) < new Date() ? 'expired' : 'pending'

  const STATUS: Record<ReturnType<typeof inviteState>, { dot: DotStatus; word: string }> = {
    pending: { dot: 'warn', word: 'Pending' },
    accepted: { dot: 'ok', word: 'Joined' },
    expired: { dot: 'idle', word: 'Expired' },
    revoked: { dot: 'idle', word: 'Revoked' },
  }
  const closeDialog = () => {
    inviting = false
    error = null
  }
</script>

<SettingsSection
  title="Invites"
  description="People you have asked to join by email."
  info="An invited person gets a link and joins the moment they sign in with Google on that address. An invite lasts two weeks. Inviting the same address again sends a fresh link, and revoking one stops its link at once."
>
  <SettingsGroup>
    <QueryState query={query} errorTitle="Could not load invites" errorVariant="compact">
      {#snippet skeleton()}<div class="px-5 py-4"><SkeletonRows rows={2} /></div>{/snippet}
      {#snippet empty()}
        <SettingsRow label="No invites yet" hint="Use Invite people at the top of the page." />
      {/snippet}
      {#snippet children(data)}
        {#each data as i (i.id)}
          {@const st = inviteState(i)}
          <SettingsRow label={i.email} hint={`Sent ${relativeTime(i.createdAt)}`}>
            {#snippet icon()}<Mail size={16} />{/snippet}
            <RowStatus status={STATUS[st].dot}>{STATUS[st].word}</RowStatus>
            <!-- Every row keeps the kebab's width, so the states line up. -->
            <span class="flex w-7 justify-end">
              {#if st === 'pending'}
                <SettingsRowMenu
                  label={`Actions for the invite to ${i.email}`}
                  items={[{ label: 'Revoke invite', danger: true, onSelect: () => void revoke(i.id) }]}
                />
              {/if}
            </span>
          </SettingsRow>
        {/each}
      {/snippet}
    </QueryState>
    {#if notice}
      <p transition:slide={{ duration: 150 }} class="px-5 py-3 font-sans text-[13px] leading-5 text-muted">{notice}</p>
    {/if}
    {#if error && !inviting}
      <p transition:slide={{ duration: 150 }} class="px-5 py-3 font-sans text-[13px] leading-5 text-danger">{error}</p>
    {/if}
  </SettingsGroup>
</SettingsSection>

<Modal open={inviting} onClose={closeDialog} title="Invite people">
  <div class="space-y-5">
    <!-- Said before the send, not after it fails: an invite that goes nowhere
         reads as a bug in invites rather than a missing transport. -->
    <NoEmailBump />
    <SettingsField
      label="Email address"
      for="invite-email"
      hint="They get a link by email and join when they sign in with Google on that address."
    >
      <Input
        id="invite-email"
        size="sm"
        type="email"
        autofocus
        bind:value={draft}
        onkeydown={(e) => e.key === 'Enter' && void invite()}
        placeholder="teammate@yourcompany.com"
      />
    </SettingsField>
    {#if error}<div class="font-sans text-[13px] leading-5 text-danger" role="alert">{error}</div>{/if}
  </div>
  {#snippet footer()}
    <div class="flex justify-end gap-2">
      <Button variant="ghost" size="sm" onclick={closeDialog}>Cancel</Button>
      <Button size="sm" disabled={busy || !draft.trim()} onclick={() => void invite()}>
        {busy ? 'Sending' : 'Send invite'}
      </Button>
    </div>
  {/snippet}
</Modal>
