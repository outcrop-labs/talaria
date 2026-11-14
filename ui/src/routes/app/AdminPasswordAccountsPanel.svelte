<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import SaveAction from '@/components/admin/SaveAction.svelte'
  import { useExplicitSave } from '@/components/settings/autosave.svelte'
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsHeaderRow from '@/components/settings/SettingsHeaderRow.svelte'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import SettingsRowMenu from '@/components/settings/SettingsRowMenu.svelte'
  import SettingsSection from '@/components/settings/SettingsSection.svelte'
  import Button from '@/components/ui/Button.svelte'
  import Input from '@/components/ui/Input.svelte'
  import QueryState from '@/components/ui/QueryState.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import { confirm } from '@/components/ui/confirm.svelte'
  import { delJson, errorMessage, getList, postJson, putJson } from '@/lib/fetch-json'
  import { slide } from '@/lib/motion'
  import { relativeTime } from '@/lib/fleet'
  import { useSession } from '@/lib/session'

  /** Password accounts: email + scrypt hash rows in user_password_credentials.
   *  Offered on the login screen only while at least one exists; the password
   *  itself is never stored anywhere but as its hash, and never leaves this
   *  panel in either direction. */
  interface AccountRow {
    userId: string
    email: string
    name: string | null
    role: 'admin' | 'member'
    createdAt: string
    updatedAt: string
    lastUsedAt: string | null
  }

  // A password is typed and then committed with a button, so this tab's
  // header never says every change saves by itself.
  useExplicitSave()
  const qc = useQueryClient()
  const session = useSession()
  const me = $derived(session.data)
  // "No accounts yet." decides whether the login screen offers the password
  // form at all — a failed read must not make that claim (see the invites
  // panel for the same rule).
  const query = createQuery(() => ({
    queryKey: ['password-accounts'],
    queryFn: (): Promise<AccountRow[]> => getList<AccountRow>('/api/admin/password-accounts', 'accounts'),
  }))
  const data = $derived(query.data)
  const mine = $derived(data?.find((a) => a.userId === me?.id) ?? null)

  let email = $state('')
  let password = $state('')
  let name = $state('')
  let error = $state<string | null>(null)
  let notice = $state<string | null>(null)
  let busy = $state(false)

  // Which row has its inline set-password form open (a userId, or 'me' for
  // the admin's own first password).
  let open: string | null = $state(null)
  let newPassword = $state('')

  const refresh = async () => {
    await qc.invalidateQueries({ queryKey: ['password-accounts'] })
    await qc.invalidateQueries({ queryKey: ['admin-users'] })
    // The login screen offers the password form only while an account exists —
    // the providers query carries that verdict, so it follows every write.
    await qc.invalidateQueries({ queryKey: ['auth-providers'] })
  }

  const create = async () => {
    if (!email.trim() || password.length < 8) return
    busy = true
    error = null
    notice = null
    try {
      await postJson<{ ok: true }>('/api/admin/password-accounts', {
        email: email.trim(),
        password,
        ...(name.trim() ? { name: name.trim() } : {}),
      })
      notice = `Account created for ${email.trim().toLowerCase()}. Share the password somewhere other than that inbox.`
      email = ''
      password = ''
      name = ''
    } catch (e) {
      error = errorMessage(e)
    }
    busy = false
    await refresh()
  }

  const openSet = (id: string) => {
    open = open === id ? null : id
    newPassword = ''
  }

  const submitSet = async (userId: string) => {
    if (newPassword.length < 8) return
    busy = true
    error = null
    notice = null
    try {
      await putJson<{ ok: true }>('/api/admin/password-accounts', { userId, password: newPassword })
      notice = 'Password set.'
      open = null
      newPassword = ''
    } catch (e) {
      error = errorMessage(e)
    }
    busy = false
    await refresh()
  }

  const remove = async (row: AccountRow) => {
    const own = row.userId === me?.id
    if (
      !(await confirm({
        title: 'Remove this password account?',
        // The person stays — say so, or the button reads as "delete user".
        message: own
          ? 'Password sign-in for your own account stops working immediately. You stay signed in and your person row stays, but unless you sign in with Google there is no way back in as you.'
          : `${row.email} keeps their account and any Google sign-in, but can no longer sign in with email + password.`,
        confirmLabel: 'Remove account',
        danger: true,
      }))
    )
      return
    busy = true
    error = null
    notice = null
    try {
      await delJson<{ ok: true }>('/api/admin/password-accounts', { userId: row.userId })
      notice = `Password account removed for ${row.email}.`
    } catch (e) {
      error = errorMessage(e)
    }
    busy = false
    await refresh()
  }

  // The header's captions and each row's cells share these widths.
  const ROLE = 'w-[5.5rem] shrink-0'
  const USED = 'w-[5.5rem] shrink-0'
  const cell = 'font-sans text-[13px] leading-5 text-muted'
</script>

{#snippet setForm(userId: string, key: string)}
  <div transition:slide={{ duration: 150 }}>
    <div class="flex items-center justify-end gap-3 border-t border-line-subtle px-5 py-3">
      <Input
        size="sm"
        type="password"
        aria-label="New password"
        bind:value={newPassword}
        placeholder="New password, 8 characters or more"
        autocomplete="new-password"
        class="w-[280px]"
        onkeydown={(e) => e.key === 'Enter' && void submitSet(userId)}
      />
      <SaveAction always size="md" dirty={newPassword.length >= 8} {busy} onclick={() => void submitSet(userId)}>Save password</SaveAction>
      <Button size="md" variant="ghost" onclick={() => openSet(key)}>Cancel</Button>
    </div>
  </div>
{/snippet}

<SettingsSection
  title="Password sign-in"
  description="People who can sign in with an email and password instead of Google."
  info="Passwords are stored only as hashes and cannot be read back. The sign-in page shows the password form only while at least one account exists. Someone who signs in with Google can also be given a password as a second way in."
>
  <SettingsGroup>
    <QueryState query={query} errorTitle="Could not load password accounts" errorVariant="compact">
      {#snippet skeleton()}<div class="px-5 py-4"><SkeletonRows rows={2} /></div>{/snippet}
      {#snippet children(data)}
        <!-- The signed-in admin with no credential row: Google-only sign-in is one
             revoked OAuth grant away from a lockout. Offer the fallback here, on
             the surface that manages the other passwords. And note a failed read
             never reaches any of this — "No accounts yet." decides whether the login
             screen offers the password form at all (the invites panel keeps the same
             rule), so QueryState's error branch owns that verdict. -->
        <SettingsHeaderRow label="Account" menu>
          <span class={ROLE}>Role</span>
          <span class={USED}>Last used</span>
        </SettingsHeaderRow>
        <!-- A second way in is a choice, not a fault, so the row opens the
             form and shows no button. -->
        {#if !mine}
          <div>
            <SettingsRow
              label="Your account"
              hint="You sign in with Google, and a password gives you a second way in."
              onOpen={() => openSet('me')}
              openLabel="Set a password for your account"
            >
              <SettingsRowMenu
                label="Actions for your account"
                items={[{ label: open === 'me' ? 'Close' : 'Set a password', disabled: busy, onSelect: () => openSet('me') }]}
              />
            </SettingsRow>
            {#if open === 'me' && me}{@render setForm(me.id, 'me')}{/if}
          </div>
        {/if}
        {#each data as row (row.userId)}
          <div>
            <SettingsRow
              label={row.email}
              hint={row.name && row.name !== row.email ? row.name : undefined}
              onOpen={() => openSet(row.userId)}
              openLabel={`Set a password for ${row.email}`}
            >
              <span class="{ROLE} {cell}">{row.role === 'admin' ? 'Admin' : 'Member'}</span>
              <span class="{USED} {cell} tabular-nums" title={row.lastUsedAt ? new Date(row.lastUsedAt).toLocaleString() : undefined}>
                {row.lastUsedAt ? relativeTime(row.lastUsedAt) : 'Never'}
              </span>
              <SettingsRowMenu
                label={`Actions for ${row.email}`}
                items={[
                  { label: open === row.userId ? 'Keep the password' : 'Set a password', onSelect: () => openSet(row.userId) },
                  'sep',
                  { label: 'Remove password sign-in', danger: true, disabled: busy, onSelect: () => void remove(row) },
                ]}
              />
            </SettingsRow>
            {#if open === row.userId}{@render setForm(row.userId, row.userId)}{/if}
          </div>
        {/each}
        {#if data.length === 0}
          <SettingsRow label="No password accounts" hint="The sign-in page offers Google only." />
        {/if}
      {/snippet}
    </QueryState>
    <SettingsRow label="Add an account" hint="Give someone an email and a first password, with a name if you like." stacked>
      <Input size="sm" type="email" aria-label="Email address" bind:value={email} placeholder="teammate@yourcompany.com" class="min-w-0 flex-1" />
      <Input
        size="sm"
        type="password"
        aria-label="First password"
        bind:value={password}
        placeholder="Password, 8 characters or more"
        autocomplete="new-password"
        class="min-w-0 flex-1"
        onkeydown={(e) => e.key === 'Enter' && void create()}
      />
      <Input size="sm" aria-label="Name" bind:value={name} placeholder="Name" class="min-w-0 flex-1" />
      <Button size="md" variant="outline" disabled={busy || !email.trim() || password.length < 8} onclick={() => void create()}>
        {busy ? 'Working' : 'Add account'}
      </Button>
    </SettingsRow>
    {#if notice}
      <p transition:slide={{ duration: 150 }} class="px-5 py-3 font-sans text-[13px] leading-5 text-success">{notice}</p>
    {/if}
    {#if error}
      <p transition:slide={{ duration: 150 }} class="px-5 py-3 font-sans text-[13px] leading-5 text-danger">{error}</p>
    {/if}
  </SettingsGroup>
</SettingsSection>
