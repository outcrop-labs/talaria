<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import { secretHint } from '@/components/admin/secret-hint'
  import NoticeRow from '@/components/admin/NoticeRow.svelte'
  import { useAutosave } from '@/components/settings/autosave.svelte'
  import SettingsField from '@/components/settings/SettingsField.svelte'
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import SettingsRowAction from '@/components/settings/SettingsRowAction.svelte'
  import SettingsRowMenu from '@/components/settings/SettingsRowMenu.svelte'
  import SettingsSaveBar from '@/components/settings/SettingsSaveBar.svelte'
  import SettingsSection from '@/components/settings/SettingsSection.svelte'
  import CopyButton from '@/components/ui/CopyButton.svelte'
  import Disclosure from '@/components/ui/Disclosure.svelte'
  import Input from '@/components/ui/Input.svelte'
  import QueryState from '@/components/ui/QueryState.svelte'
  import Segmented from '@/components/ui/Segmented.svelte'
  import type { SegmentedOption } from '@/components/ui/segmented'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import Toggle from '@/components/ui/Toggle.svelte'
  import { confirm } from '@/components/ui/confirm.svelte'
  import { useSavedFlash } from '@/components/ui/save-button.svelte'
  import { delJson, errorMessage, getJson, postJson, putJson } from '@/lib/fetch-json'
  import { relativeTime } from '@/lib/fleet'
  import { slide } from '@/lib/motion'
  import { GOOGLE_API_LIBRARY as GOOGLE_APIS } from '@/lib/google-apis'
  import RowStatus from '@/components/admin/RowStatus.svelte'

  // The Google OAuth CLIENTS, by role. The CONNECTIONS client is the
  // credential every Google connection (personal, org, per-agent) runs on;
  // the optional SIGN-IN client is what Google login runs on, and while it is
  // empty login rides the connections client. Registered here instead of
  // ui/.env; secrets are sealed and never shown again.
  type Role = 'connections' | 'signin'
  interface Agentic {
    /** What the operator says Google Cloud shows for "Use this client for an
     *  AI-powered agent": ticked, unticked, or not said (null). */
    declared: boolean | null
    /** The env var pinning it, when one does: read-only here. */
    pinnedBy: string | null
  }
  interface ClientStatus {
    /** The slot has its OWN usable client. */
    configured: boolean
    source: 'admin' | 'env' | null
    clientId: string
    secretSet: boolean
    /** The domain restriction. On the sign-in slot, the one it inherits from
     *  the connections client. */
    hd: string | null
    agentic: Agentic
    /** Google's answer to the last "Check with Google" for this client:
     *  `ok` only when Google's Gmail MCP server accepted the call; `text` is
     *  Google's own words when it refused. */
    lastCheck: { ok: boolean; status: number; text: string | null; at: string } | null
  }
  interface Payload {
    status: ClientStatus
    signin: ClientStatus
    /** The client Google login runs on now. */
    loginRole: Role | null
    loginEnabled: boolean
    /** Env pinned login on — the toggle can't turn it back off. */
    loginPinnedByEnv: boolean
    warnings: Array<{ code: string; message: string }>
    /** The signed-in admin's OWN Google connection (Settings → Connections). */
    personalConnected: boolean
    /** The client that made that connection: "Check with Google" calls with
     *  it, so only that client's rows can run the check. */
    personalClientId: string | null
    /** Each role's list is what that client must have registered. */
    redirectUris: Array<{ uri: string; what: string; role: Role }>
    /** Every stored grant, counted per OAuth client that minted it. `role` is
     *  the slot that client sits in now (null: configured nowhere, so those
     *  grants need a reconnect); `shadowed` marks the ui/.env pair behind that
     *  slot's Admin record, which refreshes the grants it minted and mints
     *  none; a null clientId is grants not yet recorded, which record their
     *  client on their next refresh. */
    census: Array<{
      clientId: string | null
      role: Role | null
      source: 'admin' | 'env' | null
      shadowed: boolean
      personal: number
      org: number
      agent: number
      /** Of those, the ones Google last refused the client for. */
      flagged: number
    }>
  }

  const censusCounts = (c: Payload['census'][number]) =>
    [c.personal ? `${c.personal} personal` : '', c.org ? `${c.org} shared` : '', c.agent ? `${c.agent} agent` : ''].filter(Boolean).join(' · ')
  const censusSlot = (c: Payload['census'][number]) =>
    c.shadowed
      ? `The server environment's ${c.role === 'signin' ? 'sign-in' : 'connections'} client, behind the one set up here. It only refreshes.`
      : c.role === 'connections'
        ? `The connections client${c.source === 'env' ? ', set in the server environment' : ''}.`
        : c.role === 'signin'
          ? `The sign-in client${c.source === 'env' ? ', set in the server environment' : ''}.`
          : c.clientId
            ? 'Set up nowhere, so these need a reconnect.'
            : 'Recorded on their next refresh.'

  type AgenticChoice = 'ticked' | 'unticked' | 'unsure'
  const AGENTIC: ReadonlyArray<SegmentedOption<AgenticChoice>> = [
    { id: 'ticked', label: 'Ticked', title: 'Marked for AI agents in Google Cloud' },
    { id: 'unticked', label: 'Unticked', title: 'Not marked for AI agents in Google Cloud' },
    { id: 'unsure', label: 'Not said', title: 'Nobody has checked yet' },
  ]
  const choiceOf = (declared: boolean | null): AgenticChoice =>
    declared === true ? 'ticked' : declared === false ? 'unticked' : 'unsure'
  const AGENTIC_INFO =
    "Google Auth Platform → Clients → this client → “Use this client for an AI-powered agent”. Only clients with it ticked may call Google's Workspace MCP servers, and Google says to keep sign-in and other non-agent features on a client without it. Google offers no way to read the box, so record what it shows. Recording it changes no REST flow; a client recorded as unticked keeps its connections away from Google's MCP servers until Google accepts a check."
  const CHECK_INFO =
    "Calls Gmail's MCP server once (list_labels, 1 unit of its quota) with your own Google connection, which must have been made with this client. Google's MCP servers refuse a client not marked for AI agents, so an accepted call is Google confirming the mark. A refusal is shown in Google's words and proves nothing about the mark: the same call fails for a project not in the Workspace Developer Preview, or without the Gmail MCP service enabled."

  const qc = useQueryClient()
  const autosave = useAutosave()
  const query = createQuery(() => ({
    queryKey: ['google-client'],
    queryFn: (): Promise<Payload> => getJson<Payload>('/api/admin/google-client'),
  }))
  const data = $derived(query.data)
  // The form seeds from the loaded status once, then belongs to the admin —
  // saving is explicit, and a refetch (e.g. window refocus) must not wipe a
  // half-typed secret.
  let clientId = $state('')
  let secret = $state('')
  let hd = $state('')
  let seeded = false
  $effect(() => {
    if (seeded || !data) return
    clientId = data.status.clientId
    hd = data.status.hd ?? ''
    seeded = true
  })
  let signinClientId = $state('')
  let signinSecret = $state('')
  let signinSeeded = false
  $effect(() => {
    if (signinSeeded || !data) return
    signinClientId = data.signin.clientId
    signinSeeded = true
  })
  let error = $state<string | null>(null)
  // The sign-in switch sits outside the setup form, so its failure shows beside it.
  let loginError = $state<string | null>(null)
  let notice = $state<string | null>(null)
  let busy = $state(false)
  let signinError = $state<string | null>(null)
  let signinNotice = $state<string | null>(null)
  const clientSaved = useSavedFlash()
  const signinSaved = useSavedFlash()
  // Back to what is stored, with the half-typed secret dropped.
  const discard = () => {
    clientId = data?.status.clientId ?? ''
    hd = data?.status.hd ?? ''
    secret = ''
  }
  const discardSignin = () => {
    signinClientId = data?.signin.clientId ?? ''
    signinSecret = ''
  }
  // Nothing is stored here while the client comes from the server
  // environment, so a filled form is itself something to save.
  const dirty = $derived(
    !!data &&
      !!clientId.trim() &&
      (!!secret.trim() || data.status.secretSet) &&
      (data.status.source !== 'admin' ||
        clientId.trim() !== data.status.clientId ||
        !!secret.trim() ||
        (hd.trim() || null) !== data.status.hd),
  )
  const signinDirty = $derived(
    !!data &&
      !!signinClientId.trim() &&
      (!!signinSecret.trim() || data.signin.secretSet) &&
      (data.signin.source !== 'admin' || signinClientId.trim() !== data.signin.clientId || !!signinSecret.trim()),
  )

  const save = async () => {
    busy = true
    error = null
    notice = null
    try {
      // Omit the secret field entirely when untouched — the server keeps the
      // stored one, so rotating the id or domain never requires re-entering it.
      await putJson<{ ok: true }>('/api/admin/google-client', {
        clientId: clientId.trim(),
        ...(secret.trim() ? { clientSecret: secret.trim() } : {}),
        hd: hd.trim() || null,
      })
      // The next step, said once: the first time a client is stored.
      if (!data?.status.configured) notice = 'Client saved. Connect the shared account below.'
      secret = ''
      clientSaved.flash()
    } catch (e) {
      error = errorMessage(e)
    }
    busy = false
    await qc.invalidateQueries({ queryKey: ['google-client'] })
    // The org panel's "available" flag and every Google surface follow the
    // client — refresh them together.
    await qc.invalidateQueries({ queryKey: ['org-google'] })
  }

  const clear = async () => {
    // Login takes its domain from this client even when a sign-in client runs
    // it, so removing this one stops login too (the server refuses to run the
    // sign-in client with no domain to take) — say so before, not after.
    const loginToo = data?.signin.configured
      ? ' Sign in with Google stops too: the sign-in client takes its domain from this client, so it stays off until a connections client is set up again, unless AUTH_GOOGLE_HD in the server environment names a domain.'
      : ''
    if (
      !(await confirm({
        title: 'Remove the Google client',
        message: `Remove the Google client credentials stored here? Connections made with this client stop refreshing until it is set up again, as either client, or until they reconnect on another client.${loginToo}`,
        confirmLabel: 'Remove',
        danger: true,
      }))
    )
      return
    busy = true
    error = null
    notice = null
    try {
      await delJson<{ ok: true }>('/api/admin/google-client')
      seeded = false
      notice = 'Client removed.'
    } catch (e) {
      error = errorMessage(e)
    }
    busy = false
    await qc.invalidateQueries({ queryKey: ['google-client'] })
    await qc.invalidateQueries({ queryKey: ['org-google'] })
  }

  const saveSignin = async () => {
    busy = true
    signinError = null
    signinNotice = null
    try {
      await putJson<{ ok: true }>('/api/admin/google-client?role=signin', {
        clientId: signinClientId.trim(),
        ...(signinSecret.trim() ? { clientSecret: signinSecret.trim() } : {}),
      })
      if (!data?.signin.configured) signinNotice = 'Sign-in client saved. Add its redirect address to it in Google Cloud Console.'
      signinSecret = ''
      signinSaved.flash()
    } catch (e) {
      signinError = errorMessage(e)
    }
    busy = false
    await qc.invalidateQueries({ queryKey: ['google-client'] })
    await qc.invalidateQueries({ queryKey: ['auth-providers'] })
  }

  const clearSignin = async () => {
    if (
      !(await confirm({
        title: 'Remove the sign-in client',
        message:
          'Remove the sign-in client stored here? Sign in with Google goes back to the connections client, unless AUTH_GOOGLE_SIGNIN_CLIENT_ID is set in the server environment.',
        confirmLabel: 'Remove',
        danger: true,
      }))
    )
      return
    busy = true
    signinError = null
    signinNotice = null
    try {
      await delJson<{ ok: true }>('/api/admin/google-client?role=signin')
      signinSeeded = false
      signinNotice = 'Sign-in client removed.'
    } catch (e) {
      signinError = errorMessage(e)
    }
    busy = false
    await qc.invalidateQueries({ queryKey: ['google-client'] })
    await qc.invalidateQueries({ queryKey: ['auth-providers'] })
  }

  // A declaration saves on change, like any picker: it records a fact about
  // the client and changes no REST flow (one declared unticked keeps its
  // grants off Google's MCP servers until Google accepts a check).
  const declare = async (role: Role, id: string, choice: AgenticChoice) => {
    if (busy) return
    // The refusal lands in the section that was clicked.
    const fail = (m: string | null) => (role === 'signin' ? (signinError = m) : (error = m))
    busy = true
    fail(null)
    try {
      await putJson<{ ok: true }>('/api/admin/google-client/agentic', {
        clientId: id,
        agentic: choice === 'ticked' ? true : choice === 'unticked' ? false : null,
      })
      autosave.flash(`agentic-${role}`)
    } catch (e) {
      fail(errorMessage(e))
    }
    busy = false
    await qc.invalidateQueries({ queryKey: ['google-client'] })
  }

  // "Check with Google": one call to Gmail's MCP server with the admin's own
  // connection made by this client; Google's answer is stored on the client.
  // Only the rows of the client that made that connection can run it; the
  // others say why, in the server's terms.
  let checking = $state<Role | null>(null)
  const checkUnavailable = (role: Role): string => {
    const lead = 'Check with Google calls with your own Google connection made with this client.'
    const signinNote =
      role === 'signin' ? ' A sign-in client that has only ever run sign-in made none, and needs no check: signing in is not an agent.' : ''
    if (!data?.personalConnected) return `${lead} Connect your account first.${signinNote}`
    if (!data.personalClientId) return `${lead} Yours records its client on its next refresh, within the hour.`
    return `${lead} Yours was made with ${data.personalClientId}.${signinNote}`
  }
  const checkWithGoogle = async (role: Role) => {
    if (busy) return
    const fail = (m: string | null) => (role === 'signin' ? (signinError = m) : (error = m))
    busy = true
    checking = role
    fail(null)
    try {
      // The answer (the panel's shape plus `check`) is read back through the query.
      await postJson<unknown>(`/api/admin/google-client/check?role=${role}`)
    } catch (e) {
      fail(errorMessage(e))
    }
    busy = false
    checking = null
    await qc.invalidateQueries({ queryKey: ['google-client'] })
  }

  // The login switch saves itself — it is one deliberate flip, not a form
  // field, and the login screen follows it immediately.
  const toggleLogin = async (next: boolean) => {
    busy = true
    loginError = null
    notice = null
    try {
      await putJson<{ ok: true }>('/api/admin/google-client/login', { enabled: next })
      autosave.flash()
    } catch (e) {
      loginError = errorMessage(e)
    }
    busy = false
    await qc.invalidateQueries({ queryKey: ['google-client'] })
    await qc.invalidateQueries({ queryKey: ['auth-providers'] })
  }
</script>

{#snippet agenticRows(role: Role, client: ClientStatus)}
  <!-- What Google Cloud shows for this client's agentic box. Talaria cannot
       read it, so the operator records it; an env pin is shown read-only. -->
  <SettingsRow
    label="Marked for AI agents"
    hint="What Google Cloud shows for “Use this client for an AI-powered agent”."
    info={AGENTIC_INFO}
    saved={client.agentic.pinnedBy ? undefined : autosave.saved(`agentic-${role}`)}
  >
    {#if client.agentic.pinnedBy}
      <span class="font-sans text-[13px] leading-5 text-fg">
        {AGENTIC.find((o) => o.id === choiceOf(client.agentic.declared))?.label}, set by <code class="font-mono text-xs">{client.agentic.pinnedBy}</code>
      </span>
    {:else}
      <Segmented options={AGENTIC} value={choiceOf(client.agentic.declared)} onChange={(c) => void declare(role, client.clientId, c)} />
    {/if}
  </SettingsRow>
  {#if client.configured}
    <!-- Google's own answer, the only provider-side evidence of the mark:
         shown as Google sent it, never summarised. -->
    {@const canCheck = !!data?.personalClientId && data.personalClientId === client.clientId}
    <SettingsRow label="Check with Google" info={CHECK_INFO}>
      {#snippet hint()}
        {#if client.lastCheck}
          {#if client.lastCheck.ok}
            Google's Gmail MCP server accepted this client's call (HTTP {client.lastCheck.status}), which confirms the mark.
          {:else}
            Google answered HTTP {client.lastCheck.status}{client.lastCheck.text ? ':' : ', with no words.'}
            {#if client.lastCheck.text}<span class="break-words text-fg">«{client.lastCheck.text}»</span>{/if}
          {/if}
          Checked {relativeTime(client.lastCheck.at)}.
        {:else if canCheck}
          Not checked yet.
        {/if}
        {#if !canCheck}<span class="block">{checkUnavailable(role)}</span>{/if}
      {/snippet}
      {#snippet status()}
        {#if checking === role}
          <RowStatus status="idle">Checking</RowStatus>
        {:else if client.lastCheck}
          <RowStatus status={client.lastCheck.ok ? 'ok' : 'warn'}>{client.lastCheck.ok ? 'Confirmed' : 'Refused'}</RowStatus>
        {:else}
          <RowStatus status="idle">Not checked</RowStatus>
        {/if}
      {/snippet}
      {#if canCheck}
        <SettingsRowAction disabled={busy} onclick={() => void checkWithGoogle(role)}>Check with Google</SettingsRowAction>
      {/if}
    </SettingsRow>
  {/if}
{/snippet}

{#snippet uriList(uris: Payload['redirectUris'])}
  <ul class="divide-y divide-line-subtle rounded-md border border-line">
    {#each uris as r (r.uri)}
      <li class="flex items-center gap-3 px-3 py-2">
        <code class="min-w-0 flex-1 break-all font-mono text-xs text-fg">{r.uri}</code>
        <span class="shrink-0 font-sans text-xs leading-4 text-muted">{r.what}</span>
        <CopyButton value={r.uri} title="Copy address" class="shrink-0" />
      </li>
    {/each}
  </ul>
{/snippet}

<SettingsSection
  title="Google"
  description="A Google Cloud client runs every Google connection, and Google sign-in unless a sign-in client is set below."
>
  <!-- A failed read must not render as "not set up" — that verdict sends an
       admin to re-enter a client that is already stored. QueryState's error
       branch holds it. -->
  <QueryState query={query} errorTitle="Could not load the Google client" errorVariant="compact">
    {#snippet skeleton()}
      <SettingsGroup>
        <SettingsRow label="Connections client"><Skeleton class="h-5 w-28" /></SettingsRow>
        <SettingsRow label="Sign in with Google"><Skeleton class="h-5 w-9 rounded-full" /></SettingsRow>
      </SettingsGroup>
    {/snippet}
    {#snippet children(data)}
      <div class="space-y-3">
        <SettingsGroup>
          {#each data.warnings as w (w.code)}
            <NoticeRow status="warn">{w.message}</NoticeRow>
          {/each}
          <SettingsRow
            label="Connections client"
            info="Every Google connection runs on it: your account, the shared account and each agent's own. Sign in with Google runs on it too, unless a sign-in client is set. Create an OAuth client in Google Cloud Console, add the redirect addresses listed under Client setup, and paste its ID and secret there."
          >
            {#snippet hint()}
              {#if data.status.configured}
                <span class="break-all font-mono text-xs">{data.status.clientId}</span>
              {:else}
                Not set up yet. Open Client setup below.
              {/if}
            {/snippet}
            {#snippet status()}
              {#if data.status.configured}
                <RowStatus status="ok">
                  {data.status.source === 'admin' ? 'Set up here' : 'Set in the server environment'}
                </RowStatus>
              {:else}
                <RowStatus status="idle">Not set</RowStatus>
              {/if}
            {/snippet}
            <!-- Only a client stored here can be removed here. -->
            {#if data.status.source === 'admin'}
              <SettingsRowMenu
                label="Actions for the connections client"
                items={[{ label: 'Remove client', danger: true, disabled: busy, onSelect: () => void clear() }]}
              />
            {/if}
          </SettingsRow>
          {#if data.status.clientId}
            {@render agenticRows('connections', data.status)}
          {/if}
          <!-- Policy, not credential: offering "Continue with Google" on the
               sign-in screen is a deliberate admin act, so it is a toggle saved
               on flip, never a side effect of saving a client. -->
          <SettingsRow label="Sign in with Google" saved={autosave.saved()}>
            {#snippet hint()}
              {#if !data.loginRole && data.signin.configured}
                Off until the connections client, or AUTH_GOOGLE_HD, gives sign-in its domain.
              {:else if !data.loginRole}
                Set up a client first.
              {:else if data.loginEnabled && data.loginPinnedByEnv}
                Kept on by <code class="font-mono text-xs">AUTH_GOOGLE_ENABLED</code> in the server environment.
              {:else}
                Shows Continue with Google on the sign-in page.
              {/if}
              {#if data.loginRole === 'signin'}
                It runs on the sign-in client.
              {/if}
            {/snippet}
            <Toggle
              title="Sign in with Google"
              checked={data.loginEnabled}
              onChange={(next) => void toggleLogin(next)}
              disabled={busy || !data.loginRole}
            />
          </SettingsRow>
          <!-- Offered only once a client exists, because the connect flow
               cannot start without one. This is the PERSONAL connection; the
               shared account is the section below. -->
          {#if data.status.configured}
            <SettingsRow
              label="Your Google account"
              hint={data.personalConnected ? 'Manage it in Settings, under Connections.' : 'Lets your assistant use Drive, Calendar and Mail as you.'}
              info="This is your own connection, the same one as Settings, Connections. The shared account agents use is separate and is set up in the next section."
            >
              {#snippet status()}
                <RowStatus status={data.personalConnected ? 'ok' : 'idle'}>{data.personalConnected ? 'Connected' : 'Not connected'}</RowStatus>
              {/snippet}
              {#if !data.personalConnected}
                <SettingsRowMenu
                  label="Actions for your Google account"
                  items={[{ label: 'Connect Google', onSelect: () => window.location.assign('/api/integrations/google/connect') }]}
                />
              {/if}
            </SettingsRow>
          {/if}
          {#if loginError}
            <p transition:slide={{ duration: 150 }} class="px-5 py-3 font-sans text-[13px] leading-5 text-danger">{loginError}</p>
          {:else if error}
            <p transition:slide={{ duration: 150 }} class="px-5 py-3 font-sans text-[13px] leading-5 text-danger" role="alert">{error}</p>
          {:else if notice}
            <p transition:slide={{ duration: 150 }} class="px-5 py-3 font-sans text-[13px] leading-5 text-success">{notice}</p>
          {/if}
        </SettingsGroup>

        <!-- Kept mounted while closed, so a half-typed secret survives. -->
        <Disclosure title="Client setup" variant="card" keepMounted defaultOpen={!data.status.configured}>
          <div class="space-y-5">
            <SettingsField label="Client ID" for="google-client-id">
              <Input id="google-client-id" size="sm" class="font-mono" bind:value={clientId} placeholder="1234567890-abc.apps.googleusercontent.com" />
            </SettingsField>
            <SettingsField
              label="Client secret"
              for="google-client-secret"
              hint={secretHint(data.status.secretSet && data.status.source === 'admin', 'secret')}
            >
              <Input
                id="google-client-secret"
                size="sm"
                type="password"
                bind:value={secret}
                placeholder="GOCSPX-"
                autocomplete="off"
              />
            </SettingsField>
            <SettingsField
              label="Workspace domain"
              for="google-client-domain"
              hint="Optional, and limits sign-in and connections to Google accounts on this domain."
            >
              <Input id="google-client-domain" size="sm" class="font-mono" bind:value={hd} placeholder="yourcompany.com" />
            </SettingsField>
            <SettingsField
              label="Redirect addresses"
              hint="Add each one to the client's authorized redirect URIs in Google Cloud Console."
            >
              {@render uriList(data.redirectUris.filter((r) => r.role === 'connections'))}
            </SettingsField>
            <!-- Consent succeeds with these off — Google only checks at call
                 time — so they are easy to miss until a call is refused. -->
            <SettingsField
              label="APIs to turn on"
              hint="Turn each one on for the Google Cloud project that owns the client."
            >
              <ul class="flex flex-wrap gap-x-5 gap-y-1">
                {#each GOOGLE_APIS as a (a.service)}
                  <li>
                    <a href={a.consoleUrl} target="_blank" rel="noreferrer" class="font-sans text-[13px] leading-5 text-accent underline-offset-2 hover:underline">{a.name} ↗</a>
                  </li>
                {/each}
              </ul>
            </SettingsField>
          </div>
          {#snippet footer()}
            <SettingsSaveBar
              label="Save client"
              reason="The secret is stored encrypted and never shown again."
              {dirty}
              saving={busy}
              saved={clientSaved.saved}
              onSave={() => void save()}
              onDiscard={data.status.source === 'env' ? undefined : discard}
            />
          {/snippet}
        </Disclosure>
      </div>
    {/snippet}
  </QueryState>
</SettingsSection>

<SettingsSection
  title="Sign-in client"
  description="Optional. Keeps Sign in with Google off the client marked for AI agents."
  info="The client Google sign-in runs on, kept apart from the connections client so people never sign in through a client marked for AI agents. While it is empty, sign-in uses the connections client. It changes no stored connection."
>
  <QueryState query={query} errorTitle="Could not load the sign-in client" errorVariant="compact">
    {#snippet skeleton()}
      <SettingsGroup>
        <SettingsRow label="Sign-in client"><Skeleton class="h-5 w-28" /></SettingsRow>
      </SettingsGroup>
    {/snippet}
    {#snippet children(data)}
      <div class="space-y-3">
        <SettingsGroup>
          <SettingsRow label="Sign-in client">
            {#snippet hint()}
              {#if data.signin.configured}
                <span class="break-all font-mono text-xs">{data.signin.clientId}</span>
              {:else}
                Not set. Sign in with Google uses the connections client.
              {/if}
            {/snippet}
            {#snippet status()}
              {#if data.signin.configured}
                <RowStatus status="ok">{data.signin.source === 'admin' ? 'Set up here' : 'Set in the server environment'}</RowStatus>
              {:else}
                <RowStatus status="idle">Not set</RowStatus>
              {/if}
            {/snippet}
            {#if data.signin.source === 'admin' || (!data.signin.configured && data.signin.clientId)}
              <SettingsRowMenu
                label="Actions for the sign-in client"
                items={[{ label: 'Remove sign-in client', danger: true, disabled: busy, onSelect: () => void clearSignin() }]}
              />
            {/if}
          </SettingsRow>
          <SettingsRow
            label="Domain"
            info="Sign-in always enforces the connections client's domain, whichever client runs it. Change it in the connections client's setup. With no connections client and no AUTH_GOOGLE_HD, sign-in does not run at all rather than admit every Google account."
          >
            {#snippet hint()}
              {#if data.signin.hd}
                <span class="font-mono text-xs">{data.signin.hd}</span>
              {:else if data.status.configured}
                Any Google account.
              {:else}
                None yet: sign-in waits for the connections client.
              {/if}
            {/snippet}
          </SettingsRow>
          {#if data.signin.clientId}
            {@render agenticRows('signin', data.signin)}
          {/if}
          {#if signinError}
            <p transition:slide={{ duration: 150 }} class="px-5 py-3 font-sans text-[13px] leading-5 text-danger" role="alert">{signinError}</p>
          {:else if signinNotice}
            <p transition:slide={{ duration: 150 }} class="px-5 py-3 font-sans text-[13px] leading-5 text-success">{signinNotice}</p>
          {/if}
        </SettingsGroup>

        <Disclosure title="Sign-in client setup" variant="card" keepMounted>
          <div class="space-y-5">
            <SettingsField label="Client ID" for="google-signin-client-id">
              <Input
                id="google-signin-client-id"
                size="sm"
                class="font-mono"
                bind:value={signinClientId}
                placeholder="1234567890-def.apps.googleusercontent.com"
              />
            </SettingsField>
            <SettingsField
              label="Client secret"
              for="google-signin-client-secret"
              hint={secretHint(data.signin.secretSet && data.signin.source === 'admin', 'secret')}
            >
              <Input id="google-signin-client-secret" size="sm" type="password" bind:value={signinSecret} placeholder="GOCSPX-" autocomplete="off" />
            </SettingsField>
            <SettingsField
              label="Redirect address"
              hint="Add it to the sign-in client in Google Cloud Console. Once sign-in runs on this client, the connections client no longer needs it."
            >
              {@render uriList(data.redirectUris.filter((r) => r.role === 'signin'))}
            </SettingsField>
          </div>
          {#snippet footer()}
            <SettingsSaveBar
              label="Save sign-in client"
              reason="The secret is stored encrypted and never shown again."
              dirty={signinDirty}
              saving={busy}
              saved={signinSaved.saved}
              onSave={() => void saveSignin()}
              onDiscard={data.signin.source === 'env' ? undefined : discardSignin}
            />
          {/snippet}
        </Disclosure>
      </div>
    {/snippet}
  </QueryState>
</SettingsSection>

{#if data?.census.length}
  <!-- Which client each stored grant refreshes with. A grant keeps the
       client that minted it, wherever that client sits now, so this is
       what to read before and after moving a client between slots. -->
  <SettingsSection
    title="Connections by client"
    description="Which client each stored Google connection refreshes with."
    info="Every stored Google connection remembers the OAuth client that minted it and refreshes with that client, whichever slot it is in. A client set up nowhere leaves its connections needing a reconnect. Connections made before clients were recorded are recorded on their next refresh."
  >
    <SettingsGroup>
      {#each data.census as c (c.clientId ?? '')}
        <SettingsRow label={c.clientId ?? 'Not recorded yet'} hint={censusSlot(c)}>
          {#snippet status()}
            <RowStatus status={!c.clientId ? 'idle' : !c.role || c.flagged ? 'warn' : 'ok'}>
              {censusCounts(c)}{c.flagged ? ` · ${c.flagged} refused` : ''}
            </RowStatus>
          {/snippet}
        </SettingsRow>
      {/each}
    </SettingsGroup>
  </SettingsSection>
{/if}
