<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import Button from '@/components/ui/Button.svelte'
  import CopyButton from '@/components/ui/CopyButton.svelte'
  import Input from '@/components/ui/Input.svelte'
  import InfoTip from '@/components/ui/InfoTip.svelte'
  import Panel from '@/components/ui/Panel.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import SectionHeader from '@/components/ui/SectionHeader.svelte'
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import Toggle from '@/components/ui/Toggle.svelte'
  import { confirm } from '@/components/ui/confirm.svelte'
  import { delJson, errorMessage, getJson, putJson } from '@/lib/fetch-json'
  import { slide } from '@/lib/motion'
  import { GOOGLE_API_LIBRARY as GOOGLE_APIS } from '@/lib/google-apis'

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
    redirectUris: Array<{ uri: string; what: string }>
  }

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
  let error = $state<string | null>(null)
  let notice = $state<string | null>(null)
  let busy = $state(false)

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

{#if query.isPending}
  <Panel class="mt-4">
    <Skeleton class="mb-3 h-4 w-32 rounded-full" />
    <SkeletonRows rows={2} />
  </Panel>
{:else if !data}
  <!-- A failed read must not render as "not configured" — that verdict sends
       an admin to re-enter a client that is already stored. -->
  <Panel class="mt-4">
    <QueryError
      variant="compact"
      error={query.error}
      title="Could not load the Google client"
      onRetry={() => void query.refetch()}
    />
  </Panel>
{:else}
  <Panel class="mt-4">
    <SectionHeader
      title="Google Workspace · OAuth client"
      info="The one credential every Google surface runs on: workspace connect (Drive, Docs, Calendar, Gmail) and, when enabled, Google login. Register it here instead of editing ui/.env; the secret is stored encrypted and never shown again."
    />
    <div class="space-y-4">
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
          disabled={busy || !data.status.configured}
          label="Google login"
        />
        <span class="font-sans text-[10px] text-muted">
          {#if !data.status.configured}
            configure a client first
          {:else if data.loginEnabled && data.loginPinnedByEnv}
            on — pinned by <code>AUTH_GOOGLE_ENABLED</code> in ui/.env
          {:else if data.loginEnabled}
            on — the sign-in button is live on /login
          {:else}
            off — the login screen stays password-only
          {/if}
        </span>
      </div>

      <!-- The redirect URIs — the one thing every Google setup asks for first. -->
      <div>
        <div class="mb-1.5 flex items-center gap-2 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">
          <span>Redirect URIs</span>
          <InfoTip text="Google Cloud Console → APIs &amp; Services → Credentials → your OAuth client → Authorized redirect URIs. Add each line exactly as shown." />
        </div>
        <div class="space-y-1">
          {#each data.redirectUris as r (r.uri)}
            <div class="flex items-center gap-2">
              <span class="min-w-0 flex-1 truncate font-mono text-xs text-fg">{r.uri}</span>
              <span class="shrink-0 font-sans text-[10px] text-muted">{r.what}</span>
              <CopyButton value={r.uri} class="shrink-0" />
            </div>
          {/each}
        </div>
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
          <label class="w-24 shrink-0 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Client ID</label>
          <Input size="sm" bind:value={clientId} placeholder="1234567890-abc.apps.googleusercontent.com" class="w-full" />
        </div>
        <div class="flex items-center gap-3">
          <label class="w-24 shrink-0 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Client secret</label>
          <Input
            size="sm"
            type="password"
            bind:value={secret}
            placeholder={data.status.secretSet && data.status.source === 'admin' ? 'stored (paste to rotate)' : 'GOCSPX-…'}
            autocomplete="off"
            class="w-full"
          />
        </div>
        <div class="flex items-center gap-3">
          <label class="w-24 shrink-0 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Domain</label>
          <Input size="sm" bind:value={hd} placeholder="yourcompany.com (optional)" class="w-full" />
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
  </Panel>
{/if}
