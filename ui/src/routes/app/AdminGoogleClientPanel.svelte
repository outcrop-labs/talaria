<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import SaveAction from '@/components/admin/SaveAction.svelte'
  import { secretHint } from '@/components/admin/secret-hint'
  import { useAutosave } from '@/components/settings/autosave.svelte'
  import SettingsField from '@/components/settings/SettingsField.svelte'
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import SettingsSection from '@/components/settings/SettingsSection.svelte'
  import { buttonClasses } from '@/components/ui/button'
  import CopyButton from '@/components/ui/CopyButton.svelte'
  import DangerLink from '@/components/ui/DangerLink.svelte'
  import Disclosure from '@/components/ui/Disclosure.svelte'
  import Input from '@/components/ui/Input.svelte'
  import QueryState from '@/components/ui/QueryState.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import Toggle from '@/components/ui/Toggle.svelte'
  import { confirm } from '@/components/ui/confirm.svelte'
  import { delJson, errorMessage, getJson, putJson } from '@/lib/fetch-json'
  import { slide } from '@/lib/motion'
  import { GOOGLE_API_LIBRARY as GOOGLE_APIS } from '@/lib/google-apis'
  import RowStatus from '@/components/admin/RowStatus.svelte'

  // The Google OAuth CLIENT — the credential every Google flow (workspace
  // connect, calendar/mail/drive, login when enabled) runs on. Registered
  // here instead of ui/.env; the secret is sealed and never shown again.
  interface ClientStatus {
    configured: boolean
    source: 'admin' | 'env' | null
    clientId: string
    secretSet: boolean
    hd: string | null
  }
  interface Payload {
    status: ClientStatus
    loginEnabled: boolean
    /** Env pinned login on — the toggle can't turn it back off. */
    loginPinnedByEnv: boolean
    /** The signed-in admin's OWN Google connection (Settings → Connections). */
    personalConnected: boolean
    redirectUris: Array<{ uri: string; what: string }>
  }

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
  let error = $state<string | null>(null)
  // The sign-in switch sits outside the setup form, so its failure shows beside it.
  let loginError = $state<string | null>(null)
  let notice = $state<string | null>(null)
  let busy = $state(false)
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
      notice = 'Client saved. Connect the shared account below.'
      secret = ''
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
    if (!(await confirm({ title: 'Remove the Google client', message: 'Remove the Google client credentials stored in the Admin UI? Connected accounts stop working until a client is configured again.', confirmLabel: 'Remove', danger: true }))) return
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

<SettingsSection
  title="Google"
  description="One Google Cloud client runs Google sign-in and every Google connection."
  info="Create an OAuth client in Google Cloud Console, add the redirect addresses listed under Client setup, and paste its ID and secret there. The secret is stored encrypted and never shown again."
>
  <!-- A failed read must not render as "not set up" — that verdict sends an
       admin to re-enter a client that is already stored. QueryState's error
       branch holds it. -->
  <QueryState query={query} errorTitle="Could not load the Google client" errorVariant="compact">
    {#snippet skeleton()}
      <SettingsGroup>
        <SettingsRow label="Google Cloud client"><Skeleton class="h-5 w-28" /></SettingsRow>
        <SettingsRow label="Sign in with Google"><Skeleton class="h-5 w-9 rounded-full" /></SettingsRow>
      </SettingsGroup>
    {/snippet}
    {#snippet children(data)}
      <div class="space-y-3">
        <SettingsGroup>
          <SettingsRow label="Google Cloud client">
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
          </SettingsRow>
          <!-- Policy, not credential: offering "Continue with Google" on the
               sign-in screen is a deliberate admin act, so it is a toggle saved
               on flip, never a side effect of saving a client. -->
          <SettingsRow label="Sign in with Google">
            {#snippet hint()}
              {#if !data.status.configured}
                Set up a client first.
              {:else if data.loginEnabled && data.loginPinnedByEnv}
                Kept on by <code class="font-mono text-xs">AUTH_GOOGLE_ENABLED</code> in the server environment.
              {:else}
                Shows Continue with Google on the sign-in page.
              {/if}
            {/snippet}
            <Toggle
              title="Sign in with Google"
              checked={data.loginEnabled}
              onChange={(next) => void toggleLogin(next)}
              disabled={busy || !data.status.configured}
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
                <a href="/api/integrations/google/connect" class={buttonClasses({ size: 'sm', variant: 'outline', voice: 'sans' })}>Connect</a>
              {/if}
            </SettingsRow>
          {/if}
          {#if loginError}
            <p transition:slide={{ duration: 150 }} class="px-5 py-3 font-sans text-[13px] leading-5 text-danger">{loginError}</p>
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
              <ul class="divide-y divide-line-subtle rounded-md border border-line">
                {#each data.redirectUris as r (r.uri)}
                  <li class="flex items-center gap-3 px-3 py-2">
                    <code class="min-w-0 flex-1 break-all font-mono text-xs text-fg">{r.uri}</code>
                    <span class="shrink-0 font-sans text-xs leading-4 text-muted">{r.what}</span>
                    <CopyButton value={r.uri} title="Copy address" class="shrink-0" />
                  </li>
                {/each}
              </ul>
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
            {#if error}<div transition:slide={{ duration: 150 }} class="font-sans text-[13px] leading-5 text-danger" role="alert">{error}</div>{/if}
            <div class="flex items-center gap-4">
              {#if data.status.source === 'admin'}
                <DangerLink onClick={() => void clear()} disabled={busy}>Remove client</DangerLink>
              {/if}
              <span class="flex-1"></span>
              <SaveAction {dirty} {busy} onclick={() => void save()}>
                {data.status.configured ? 'Save changes' : 'Save client'}
              </SaveAction>
            </div>
          </div>
        </Disclosure>
      </div>
    {/snippet}
  </QueryState>
</SettingsSection>
