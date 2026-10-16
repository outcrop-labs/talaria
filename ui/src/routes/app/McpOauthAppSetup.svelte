<script lang="ts">
  import Button from '@/components/ui/Button.svelte'
  import CopyButton from '@/components/ui/CopyButton.svelte'
  import InfoTip from '@/components/ui/InfoTip.svelte'
  import Input from '@/components/ui/Input.svelte'
  import { slide } from '@/lib/motion'
  import { parseScopes, patchServer, type OauthMeta } from './mcp'

  /** Credentials form for providers a dashboard OAuth app is the way into:
   *  either no dynamic client registration (GitHub), or a registration that
   *  refused this deployment's callback URL (Vercel allowlists localhost and
   *  known clients only). Shows the provider's own portal and the steps that
   *  matter there, the exact callback URL to register (the api's canonical
   *  one — the same the authorize request will name), and takes the app's
   *  client id/secret plus an optional explicit scope list. */
  let {
    serverId,
    meta,
    rejected = false,
    editing = false,
    onSaved,
    onCancel,
  }: {
    serverId: string
    meta: OauthMeta
    rejected?: boolean
    /** A client is already set; this is a change, not a first setup. */
    editing?: boolean
    onSaved: () => void
    onCancel?: () => void
  } = $props()

  // svelte-ignore state_referenced_locally -- reason: the form opens straight into editing when it is a change; a first setup starts folded
  let openForm = $state(editing)
  let clientId = $state('')
  let clientSecret = $state('')
  // svelte-ignore state_referenced_locally -- reason: seeded once from the stored override (or the advertised set) when the panel mounts
  let scopes = $state((meta.scopesOverride ?? meta.scopes).join(' '))
  let error = $state<string | null>(null)
  let busy = $state(false)

  const guide = $derived(meta.guide)
  const headline = $derived(
    editing
      ? 'OAuth app credentials'
      : rejected
        ? 'This provider refused automatic registration for this callback URL'
        : 'This provider needs a pre-registered OAuth app',
  )
  const tip = $derived(
    rejected
      ? 'It supports automatic client registration, but only for redirect URLs it has approved (localhost apps and a few known clients) — a hosted Talaria callback is not on that list. Create an OAuth app in the provider\'s developer settings with the callback URL below, then paste the app\'s client id and secret here. They\'re stored encrypted and spoken only during the OAuth flow.'
      : 'It doesn\'t support automatic client registration. Create an OAuth app in the provider\'s developer settings with the callback URL below, then paste the app\'s client id and secret here. They\'re stored encrypted and spoken only during the OAuth flow.',
  )

  const save = async () => {
    busy = true
    error = null
    const e = await patchServer(serverId, {
      oauthClient: { clientId: clientId.trim(), clientSecret: clientSecret.trim() || null, scopes: parseScopes(scopes) },
    })
    busy = false
    if (e) {
      error = e
      return
    }
    openForm = false
    onSaved()
  }
</script>

<div class="mt-3 rounded-lg border border-accent/40 bg-accent/5 p-3">
  <div class="flex items-center gap-3">
    <div class="min-w-0 flex-1">
      <div class="flex items-center gap-2">
        <span class="text-xs font-medium text-fg">{headline}</span>
        <InfoTip text={tip} />
      </div>
      <div class="mt-0.5 flex flex-wrap items-center gap-x-3 gap-y-0.5 text-xs">
        {#if guide}
          <a href={guide.portal} target="_blank" rel="noreferrer" class="text-accent hover:underline">
            Create the {guide.appName} ↗
          </a>
        {/if}
        {#if meta.documentation}
          <a href={meta.documentation} target="_blank" rel="noreferrer" class="text-accent hover:underline">
            Provider docs ↗
          </a>
        {/if}
      </div>
    </div>
    {#if !openForm}
      <Button size="sm" class="shrink-0" onclick={() => (openForm = true)}>
        {editing ? 'Change' : 'Set up'}
      </Button>
    {/if}
  </div>
  {#if openForm}
    <div transition:slide={{ duration: 150 }} class="mt-3 space-y-3">
      {#if guide?.notes.length}
        <ol class="list-decimal space-y-0.5 pl-4 font-sans text-[11px] leading-relaxed text-muted">
          {#each guide.notes as n, i (i)}<li>{n}</li>{/each}
        </ol>
      {/if}
      <div>
        <span class="mb-1 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Callback URL (register this with the provider, exactly)</span>
        <div class="flex items-center gap-2">
          <code class="min-w-0 flex-1 truncate rounded-md border border-line bg-surface px-2 py-1.5 font-mono text-xs text-fg">{meta.callbackUrl}</code>
          <CopyButton value={meta.callbackUrl} label="Copy" class="shrink-0 text-xs" />
        </div>
        {#if !/^https:/.test(meta.callbackUrl)}
          <div class="mt-1 font-sans text-[11px] text-warning">
            This is a plain-http callback. Many providers accept only https — set a verified hosting domain under Admin → Org and the callback becomes stable and https.
          </div>
        {/if}
      </div>
      <div class="grid grid-cols-2 gap-3">
        <div>
          <label for={`oauth-client-id-${serverId}`} class="mb-1 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Client ID</label>
          <Input id={`oauth-client-id-${serverId}`} bind:value={clientId} autocomplete="off" />
        </div>
        <div>
          <label for={`oauth-client-secret-${serverId}`} class="mb-1 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Client secret</label>
          <Input id={`oauth-client-secret-${serverId}`} type="password" bind:value={clientSecret} autocomplete="off" placeholder={editing ? '(unchanged if blank)' : ''} />
        </div>
      </div>
      <div>
        <label for={`oauth-scopes-${serverId}`} class="mb-1 flex items-center gap-1.5 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">
          Scopes
          <InfoTip text="What the authorize request asks for. Pre-filled with what the server advertised; some providers' apps need the scopes spelled out here (space- or comma-separated). Blank = whatever the provider advertises." />
        </label>
        <Input id={`oauth-scopes-${serverId}`} bind:value={scopes} autocomplete="off" placeholder={meta.scopes.join(' ') || 'provider default'} class="font-[var(--font-mono)]" />
      </div>
      {#if error}<div transition:slide={{ duration: 150 }} class="text-xs text-danger">{error}</div>{/if}
      <div class="flex justify-end gap-2">
        <Button size="sm" variant="ghost" onclick={() => { openForm = false; onCancel?.() }}>
          Cancel
        </Button>
        <Button size="sm" disabled={busy || !clientId.trim()} onclick={() => void save()}>
          {busy ? 'Saving' : 'Save credentials'}
        </Button>
      </div>
    </div>
  {/if}
</div>
