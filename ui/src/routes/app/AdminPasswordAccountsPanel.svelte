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
  import { confirm } from '@/components/ui/confirm.svelte'
  import { cn } from '@/lib/cn'
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
      notice = `Account created for ${email.trim().toLowerCase()} — share the password over a channel other than the one it protects.`
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
          ? 'Password sign-in for YOUR account stops working immediately. You stay signed in, and the person row stays — but unless you sign in with Google, there will be no way back in as you.'
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
</script>

<Panel class="mb-4">
  <SectionHeader
    title="Password accounts"
    info="Email + password sign-in, stored as scrypt hashes in the database — created here, never read back. The login screen offers the password form only while at least one account exists. Someone who signs in with Google can still be given a password as a fallback."
  />
  <div class="mb-3 flex flex-wrap items-center gap-2">
    <Input size="sm" bind:value={email} placeholder="teammate@yourcompany.com" class="w-60" />
    <Input
      size="sm"
      type="password"
      bind:value={password}
      placeholder="initial password (min 8)"
      autocomplete="new-password"
      class="w-52"
      onkeydown={(e) => e.key === 'Enter' && void create()}
    />
    <Input size="sm" bind:value={name} placeholder="name (optional)" class="w-40" />
    <Button size="sm" disabled={busy || !email.trim() || password.length < 8} onclick={() => void create()}>
      {busy ? 'Working' : 'Add account'}
    </Button>
    {#if notice}<span class="min-w-0 truncate text-xs text-success">{notice}</span>{/if}
  </div>
  {#if error}<div transition:slide={{ duration: 150 }} class="mb-2 text-xs text-danger">{error}</div>{/if}
  {#if query.isPending}
    <SkeletonRows rows={2} />
  {:else if !data}
    <QueryError variant="inline" error={query.error} title="Could not load password accounts" onRetry={() => void query.refetch()} />
  {:else}
    <!-- The signed-in admin with no credential row: Google-only sign-in is one
         revoked OAuth grant away from a lockout. Offer the fallback here, on
         the surface that manages the other passwords. -->
    {#if !mine}
      <div class="mb-2 flex flex-wrap items-center gap-2 rounded-md border border-line-subtle px-2 py-2">
        <span class="min-w-0 flex-1 text-xs text-muted">
          You sign in with Google. Set a password as a fallback for this account.
        </span>
        {#if open === 'me'}
          <Input
            size="sm"
            type="password"
            bind:value={newPassword}
            placeholder="new password (min 8)"
            autocomplete="new-password"
            class="w-52"
            onkeydown={(e) => e.key === 'Enter' && me && void submitSet(me.id)}
          />
          <Button size="sm" disabled={busy || newPassword.length < 8} onclick={() => me && void submitSet(me.id)}>Save</Button>
          <Button variant="ghost" size="sm" onclick={() => openSet('me')}>Cancel</Button>
        {:else}
          <Button size="sm" variant="ghost" onclick={() => openSet('me')} disabled={busy}>Set your password</Button>
        {/if}
      </div>
    {/if}
    {#if data.length === 0}
      <EmptyState variant="inline" title="No password accounts — the login screen stays Google-only." class="font-sans" />
    {:else}
      <ul class="divide-y divide-line-subtle">
        {#each data as row (row.userId)}
          <li class="-mx-2 space-y-2 rounded-md px-2 py-2 transition-colors dither-fill">
            <div class="flex items-center gap-3 text-sm">
              <span class="min-w-0 flex-1">
                <span class="block truncate text-fg">{row.email}</span>
                {#if row.name && row.name !== row.email}
                  <span class="block truncate text-xs text-muted">{row.name}</span>
                {/if}
              </span>
              <span
                class={cn(
                  'shrink-0 rounded border px-1.5 py-0.5 font-mono text-[10px] uppercase tracking-[0.05em]',
                  row.role === 'admin' ? 'border-success/40 text-success' : 'border-line text-muted',
                )}
              >
                {row.role}
              </span>
              <span class="shrink-0 font-mono text-[10px] tracking-[0.05em] text-muted" title="Last used">
                {row.lastUsedAt ? `used ${relativeTime(row.lastUsedAt)}` : 'never used'}
              </span>
              <button
                type="button"
                class="shrink-0 text-xs text-muted underline-offset-2 transition-colors hover:text-fg hover:underline"
                onclick={() => openSet(row.userId)}
              >Set password</button>
              <button
                type="button"
                title="Remove the password account (the person stays)"
                onclick={() => void remove(row)}
                class="shrink-0 text-muted transition-colors hover:text-danger"
                disabled={busy}
              >
                <Trash2 size={13} />
              </button>
            </div>
            {#if open === row.userId}
              <div class="flex items-center gap-2 pl-1">
                <Input
                  size="sm"
                  type="password"
                  bind:value={newPassword}
                  placeholder="new password (min 8)"
                  autocomplete="new-password"
                  class="w-52"
                  onkeydown={(e) => e.key === 'Enter' && void submitSet(row.userId)}
                />
                <Button size="sm" disabled={busy || newPassword.length < 8} onclick={() => void submitSet(row.userId)}>Save</Button>
                <Button variant="ghost" size="sm" onclick={() => openSet(row.userId)}>Cancel</Button>
              </div>
            {/if}
          </li>
        {/each}
      </ul>
    {/if}
  {/if}
</Panel>
