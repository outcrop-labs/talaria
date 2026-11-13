<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import Button from '@/components/ui/Button.svelte'
  import CopyButton from '@/components/ui/CopyButton.svelte'
  import DangerLink from '@/components/ui/DangerLink.svelte'
  import Input from '@/components/ui/Input.svelte'
  import InfoTip from '@/components/ui/InfoTip.svelte'
  import Panel from '@/components/ui/Panel.svelte'
  import QueryState from '@/components/ui/QueryState.svelte'
  import SectionHeader from '@/components/ui/SectionHeader.svelte'
  import Segmented from '@/components/ui/Segmented.svelte'
  import type { SegmentedOption } from '@/components/ui/segmented'
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import Toggle from '@/components/ui/Toggle.svelte'
  import { confirm } from '@/components/ui/confirm.svelte'
  import { delJson, errorMessage, getJson, putJson } from '@/lib/fetch-json'
  import { slide } from '@/lib/motion'
  import { GOOGLE_API_LIBRARY as GOOGLE_APIS } from '@/lib/google-apis'

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
    /** Each role's list is what that client must have registered. */
    redirectUris: Array<{ uri: string; what: string; role: Role }>
  }

  type AgenticChoice = 'ticked' | 'unticked' | 'unsure'
  const AGENTIC: ReadonlyArray<SegmentedOption<AgenticChoice>> = [
    { id: 'ticked', label: 'Ticked', title: 'Marked for AI agents in Google Cloud' },
    { id: 'unticked', label: 'Unticked', title: 'Not marked for AI agents in Google Cloud' },
    { id: 'unsure', label: 'Not said', title: 'Nobody has checked yet' },
  ]
  const choiceOf = (declared: boolean | null): AgenticChoice =>
    declared === true ? 'ticked' : declared === false ? 'unticked' : 'unsure'
  const AGENTIC_INFO =
    "Google Auth Platform → Clients → this client → “Use this client for an AI-powered agent”. Only clients with it ticked may call Google's Workspace MCP servers, and Google says to keep sign-in and other non-agent features on a client without it. Google offers no way to read the box, so record what it shows. Recording it changes no flow."

  const qc = useQueryClient()
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
  let notice = $state<string | null>(null)
  let busy = $state(false)
  let signinError = $state<string | null>(null)
  let signinNotice = $state<string | null>(null)

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
      notice = 'Client saved. Connect the org account below.'
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
    // Login takes its domain from this client even when a sign-in client runs
    // it, so removing this one stops login too (the server refuses to run the
    // sign-in client with no domain to take) — say so before, not after.
    const loginToo = data?.signin.configured
      ? ' Google login stops too: the sign-in client takes its domain from this client, so login stays off until a connections client is configured again, unless AUTH_GOOGLE_HD in ui/.env names a domain.'
      : ''
    if (!(await confirm({ title: 'Remove the Google client', message: `Remove the Google client credentials stored in the Admin UI? Connected accounts stop working until a client is configured again.${loginToo}`, confirmLabel: 'Remove', danger: true }))) return
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
      signinNotice = 'Sign-in client saved. Register the redirect URI above on it.'
      signinSecret = ''
    } catch (e) {
      signinError = errorMessage(e)
    }
    busy = false
    await qc.invalidateQueries({ queryKey: ['google-client'] })
    await qc.invalidateQueries({ queryKey: ['auth-providers'] })
  }

  const clearSignin = async () => {
    if (!(await confirm({ title: 'Remove the sign-in client', message: 'Remove the sign-in client stored in the Admin UI? Google login goes back to the connections client, unless AUTH_GOOGLE_SIGNIN_CLIENT_ID is set in ui/.env.', confirmLabel: 'Remove', danger: true }))) return
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
  // the client and changes no flow.
  const declare = async (role: Role, id: string, choice: AgenticChoice) => {
    if (busy) return
    // The refusal lands on the card that was clicked.
    const fail = (m: string | null) => (role === 'signin' ? (signinError = m) : (error = m))
    busy = true
    fail(null)
    try {
      await putJson<{ ok: true }>('/api/admin/google-client/agentic', {
        clientId: id,
        agentic: choice === 'ticked' ? true : choice === 'unticked' ? false : null,
      })
    } catch (e) {
      fail(errorMessage(e))
    }
    busy = false
    await qc.invalidateQueries({ queryKey: ['google-client'] })
  }

  // The login switch saves itself — it is one deliberate flip, not a form
  // field, and the login screen follows it immediately.
  const toggleLogin = async (next: boolean) => {
    busy = true
    error = null
    notice = null
    try {
      await putJson<{ ok: true }>('/api/admin/google-client/login', { enabled: next })
      notice = next ? 'Google login enabled — the sign-in button is live on /login.' : 'Google login disabled.'
    } catch (e) {
      error = errorMessage(e)
    }
    busy = false
    await qc.invalidateQueries({ queryKey: ['google-client'] })
    await qc.invalidateQueries({ queryKey: ['auth-providers'] })
  }
</script>

{#snippet agenticRow(role: Role, client: ClientStatus)}
  <!-- What Google Cloud shows for this client's agentic box. Talaria cannot
       read it, so the operator records it; an env pin is shown read-only. -->
  <div class="flex flex-wrap items-center gap-2 border-t border-line pt-3">
    <span class="text-xs text-fg">“Use this client for an AI-powered agent”</span>
    <InfoTip text={AGENTIC_INFO} />
    <span class="ml-auto"></span>
    {#if client.agentic.pinnedBy}
      <span class="font-mono text-[10px] uppercase tracking-[0.05em] text-fg">{AGENTIC.find((o) => o.id === choiceOf(client.agentic.declared))?.label}</span>
      <span class="font-sans text-[10px] text-muted">pinned by <code>{client.agentic.pinnedBy}</code></span>
    {:else}
      <Segmented size="xs" options={AGENTIC} value={choiceOf(client.agentic.declared)} onChange={(c) => void declare(role, client.clientId, c)} />
    {/if}
  </div>
{/snippet}

{#snippet uriList(uris: Payload['redirectUris'])}
  <div class="space-y-1">
    {#each uris as r (r.uri)}
      <div class="flex items-center gap-2">
        <span class="min-w-0 flex-1 truncate font-mono text-xs text-fg">{r.uri}</span>
        <span class="shrink-0 font-sans text-[10px] text-muted">{r.what}</span>
        <CopyButton value={r.uri} class="shrink-0" />
      </div>
    {/each}
  </div>
{/snippet}

<Panel class="mt-4">
  <QueryState query={query} errorTitle="Could not load the Google client" errorVariant="compact">
    {#snippet skeleton()}
      <Skeleton class="mb-3 h-4 w-32 rounded-full" />
      <SkeletonRows rows={2} />
    {/snippet}
    {#snippet children(data)}
    <!-- A failed read must not render as "not configured" — that verdict sends
         an admin to re-enter a client that is already stored. QueryState's
         error branch holds it. -->
    <SectionHeader
      title="Google Workspace · OAuth client"
      info="The connections client: every Google connection runs on it (your account, the org account, per-agent identities), and Google login too unless a sign-in client is set below. Register it here instead of editing ui/.env; the secret is stored encrypted and never shown again."
    />
    <div class="space-y-4">
      {#each data.warnings as w (w.code)}
        <div class="flex items-start gap-2 text-xs text-warning">
          <StatusDot status="warn" class="mt-1" />
          <span>{w.message}</span>
        </div>
      {/each}

      <!-- Status -->
      {#if data.status.configured}
        <div class="flex items-center gap-2 text-sm">
          <StatusDot status="ok" />
          <span class="text-fg">Client configured</span>
          <span class="font-mono text-[10px] uppercase tracking-[0.05em] text-muted">
            via {data.status.source === 'admin' ? 'Admin UI' : 'ui/.env'}
          </span>
          <span class="ml-auto font-mono text-[11px] text-muted">{data.status.clientId}</span>
        </div>
      {:else}
        <div class="text-xs text-muted">
          No Google client configured. Create one in Google Cloud Console, add the redirect URIs below, and paste the credentials here.
        </div>
      {/if}

      <!-- The login switch — policy, not credential: offering "Continue with
           Google" on the login screen is a deliberate admin act, so it is a
           toggle here (saved on flip), never a side effect of saving a client. -->
      <div class="flex items-center gap-2 border-t border-line pt-3">
        <Toggle
          checked={data.loginEnabled}
          onChange={(next) => void toggleLogin(next)}
          disabled={busy || !data.loginRole}
          label="Google login"
        />
        <span class="font-sans text-[10px] text-muted">
          {#if !data.loginRole && data.signin.configured}
            off until the connections client (or AUTH_GOOGLE_HD) gives login its domain
          {:else if !data.loginRole}
            configure a client first
          {:else if data.loginEnabled && data.loginPinnedByEnv}
            on, pinned by <code>AUTH_GOOGLE_ENABLED</code> in ui/.env
          {:else if data.loginEnabled}
            on: the sign-in button is live on /login
          {:else}
            off: the login screen stays password-only
          {/if}
          {#if data.loginRole === 'signin'}
            · runs on the sign-in client below
          {:else if data.loginRole === 'connections'}
            · runs on this client
          {/if}
        </span>
      </div>

      <!-- Your own account — offered only once a client exists, because the
           connect flow cannot even start without one. This is the PERSONAL
           connection (Drive/Calendar/Mail as you, lands on Settings →
           Connections); the ORG connection is the panel below and is what
           gates Google login domains. -->
      {#if data.status.configured}
        <div class="flex items-center gap-2 border-t border-line pt-3 text-sm">
          {#if data.personalConnected}
            <StatusDot status="ok" />
            <span class="text-fg">Your Google account is connected</span>
            <span class="font-sans text-[10px] text-muted">manage it in Settings → Connections</span>
          {:else}
            <span class="text-fg">Your Google account</span>
            <a
              href="/api/integrations/google/connect"
              class="text-xs text-accent underline-offset-2 hover:underline"
            >Connect your account</a>
            <InfoTip text="Runs the same personal connect as Settings → Connections and lands there when it finishes (it needs the redirect URI listed below). The org connection — the one Google login and the workspace surfaces run on — is separate: Admin → Org → Google Workspace." />
          {/if}
        </div>
      {/if}

      {#if data.status.clientId}
        {@render agenticRow('connections', data.status)}
      {/if}

      <!-- The redirect URIs — the one thing every Google setup asks for first. -->
      <div>
        <div class="mb-1.5 flex items-center gap-2 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">
          <span>Redirect URIs</span>
          <InfoTip text="Google Cloud Console → APIs &amp; Services → Credentials → your OAuth client → Authorized redirect URIs. Add each line exactly as shown." />
        </div>
        {@render uriList(data.redirectUris.filter((r) => r.role === 'connections'))}
      </div>

      <!-- The library switches. Consent succeeds with these off — Google only
           checks at call time — so they're easy to miss until a surface 403s. -->
      <div>
        <div class="mb-1.5 flex items-center gap-2 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">
          <span>Enabled APIs</span>
          <InfoTip text="Google Cloud Console → APIs &amp; Services → Library → enable each of these on the project that owns the OAuth client. Connecting works without them; using Drive, Calendar, or Mail does not." />
        </div>
        <div class="space-y-1">
          {#each GOOGLE_APIS as a (a.service)}
            <div class="flex items-center gap-2">
              <a
                href={a.consoleUrl}
                target="_blank"
                rel="noreferrer"
                class="min-w-0 flex-1 truncate text-xs text-accent hover:underline"
              >{a.name}</a>
              <span class="shrink-0 font-sans text-[10px] text-muted">enable ↗</span>
            </div>
          {/each}
        </div>
      </div>

      <!-- Credentials -->
      <div class="space-y-2">
        <div class="flex items-center gap-3">
          <label for="google-client-id" class="w-24 shrink-0 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Client ID</label>
          <Input id="google-client-id" size="sm" bind:value={clientId} placeholder="1234567890-abc.apps.googleusercontent.com" class="w-full" />
        </div>
        <div class="flex items-center gap-3">
          <label for="google-client-secret" class="w-24 shrink-0 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Client secret</label>
          <Input
            id="google-client-secret"
            size="sm"
            type="password"
            bind:value={secret}
            placeholder={data.status.secretSet && data.status.source === 'admin' ? 'stored (paste to rotate)' : 'GOCSPX-…'}
            autocomplete="off"
            class="w-full"
          />
        </div>
        <div class="flex items-center gap-3">
          <label for="google-client-domain" class="w-24 shrink-0 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Domain</label>
          <Input id="google-client-domain" size="sm" bind:value={hd} placeholder="yourcompany.com (optional)" class="w-full" />
          <InfoTip text="Restricts connect + login to Google accounts on this Workspace domain. Leave blank to allow any Google account." />
        </div>
      </div>

      {#if error}<div transition:slide={{ duration: 150 }} class="text-xs text-danger">{error}</div>{/if}
      {#if notice}<div transition:slide={{ duration: 150 }} class="text-xs text-success">{notice}</div>{/if}

      <div class="flex items-center gap-2">
        <Button size="sm" onclick={() => void save()} disabled={busy || !clientId.trim() || (!secret.trim() && !data.status.secretSet)}>
          {data.status.configured ? 'Save changes' : 'Save client'}
        </Button>
        {#if data.status.source === 'admin'}
          <Button variant="ghost" size="sm" class="hover:text-danger" onclick={() => void clear()} disabled={busy}>
            Remove
          </Button>
        {/if}
      </div>
    </div>
    {/snippet}
  </QueryState>
</Panel>

<Panel class="mt-4">
  <QueryState query={query} errorTitle="Could not load the sign-in client" errorVariant="compact">
    {#snippet skeleton()}
      <Skeleton class="mb-3 h-4 w-32 rounded-full" />
      <SkeletonRows rows={2} />
    {/snippet}
    {#snippet children(data)}
    <SectionHeader
      title="Google Workspace · Sign-in client (optional)"
      info="The client Google login runs on, kept apart from the connections client so people never sign in through a client marked for AI agents. Empty, login uses the connections client above. It changes no stored connection."
    />
    <div class="space-y-4">
      {#if data.signin.configured}
        <div class="flex items-center gap-2 text-sm">
          <StatusDot status="ok" />
          <span class="text-fg">Sign-in client configured</span>
          <span class="font-mono text-[10px] uppercase tracking-[0.05em] text-muted">
            via {data.signin.source === 'admin' ? 'Admin UI' : 'ui/.env'}
          </span>
          <span class="ml-auto font-mono text-[11px] text-muted">{data.signin.clientId}</span>
        </div>
      {:else}
        <div class="text-xs text-muted">
          Not set. Google login uses the connections client.
        </div>
      {/if}

      <div class="flex items-center gap-2 text-xs">
        <span class="text-fg">Domain</span>
        <span class="font-mono text-[11px] text-muted">{data.signin.hd ?? (data.status.configured ? 'any Google account' : 'none yet: login waits for the connections client')}</span>
        <InfoTip text="Login always enforces the connections client's domain, whichever client runs it. Change it on the connections client above. With no connections client and no AUTH_GOOGLE_HD, login does not run at all rather than admit every Google account." />
      </div>

      {#if data.signin.clientId}
        {@render agenticRow('signin', data.signin)}
      {/if}

      <div>
        <div class="mb-1.5 flex items-center gap-2 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">
          <span>Redirect URI</span>
          <InfoTip text="Add it to the sign-in client in Google Cloud Console. Once login runs on the sign-in client, the connections client no longer needs it." />
        </div>
        {@render uriList(data.redirectUris.filter((r) => r.role === 'signin'))}
      </div>

      <div class="space-y-2">
        <div class="flex items-center gap-3">
          <label for="google-signin-client-id" class="w-24 shrink-0 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Client ID</label>
          <Input id="google-signin-client-id" size="sm" bind:value={signinClientId} placeholder="1234567890-def.apps.googleusercontent.com" class="w-full" />
        </div>
        <div class="flex items-center gap-3">
          <label for="google-signin-client-secret" class="w-24 shrink-0 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Client secret</label>
          <Input
            id="google-signin-client-secret"
            size="sm"
            type="password"
            bind:value={signinSecret}
            placeholder={data.signin.secretSet && data.signin.source === 'admin' ? 'stored (paste to rotate)' : 'GOCSPX-'}
            autocomplete="off"
            class="w-full"
          />
        </div>
      </div>

      {#if signinError}<div transition:slide={{ duration: 150 }} class="text-xs text-danger">{signinError}</div>{/if}
      {#if signinNotice}<div transition:slide={{ duration: 150 }} class="text-xs text-success">{signinNotice}</div>{/if}

      <div class="flex items-center gap-2">
        <Button size="sm" onclick={() => void saveSignin()} disabled={busy || !signinClientId.trim() || (!signinSecret.trim() && !data.signin.secretSet)}>
          {data.signin.configured ? 'Save changes' : 'Save sign-in client'}
        </Button>
        {#if data.signin.source === 'admin' || (!data.signin.configured && data.signin.clientId)}
          <DangerLink class="ml-auto" onClick={() => void clearSignin()} disabled={busy}>Remove</DangerLink>
        {/if}
      </div>
    </div>
    {/snippet}
  </QueryState>
</Panel>
