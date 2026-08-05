<script lang="ts">
  import Button from '@/components/ui/Button.svelte'
  import CopyButton from '@/components/ui/CopyButton.svelte'
  import InfoTip from '@/components/ui/InfoTip.svelte'
  import Input from '@/components/ui/Input.svelte'
  import { slide } from '@/lib/motion'
  import { OAUTH_APP_PORTALS, patchServer } from './mcp'

  /** Credentials form for providers without dynamic client registration: shows
   *  the exact callback URL to register, takes the app's client id/secret. */
  let {
    serverId,
    domain,
    docs,
    onSaved,
  }: { serverId: string; domain?: string | null; docs?: string | null; onSaved: () => void } = $props()

  let openForm = $state(false)
  let clientId = $state('')
  let clientSecret = $state('')
  let error = $state<string | null>(null)
  let busy = $state(false)
  const callback = `${window.location.origin}/api/mcp/oauth/callback`

  // The provider's own service_documentation (from its AS metadata)
  // beats our portal map — API data first.
  const link = $derived(docs ?? (domain ? OAUTH_APP_PORTALS[domain] : undefined))

  const save = async () => {
    busy = true
    error = null
    const e = await patchServer(serverId, { oauthClient: { clientId: clientId.trim(), clientSecret: clientSecret.trim() || null } })
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
        <span class="text-xs font-medium text-fg">This provider needs a pre-registered OAuth app</span>
        <InfoTip text="It doesn't support automatic client registration (GitHub, for example). Create an OAuth app in the provider's developer settings with the callback URL below, then paste the app's client id and secret here — stored encrypted, spoken only during the OAuth flow." />
      </div>
      {#if link}
        <a href={link} target="_blank" rel="noreferrer" class="mt-0.5 inline-block text-xs text-accent hover:underline">
          Create the app ↗
        </a>
      {/if}
    </div>
    {#if !openForm}
      <Button size="sm" class="shrink-0" onclick={() => (openForm = true)}>
        Set up
      </Button>
    {/if}
  </div>
  {#if openForm}
    <div transition:slide={{ duration: 150 }} class="mt-3 space-y-3">
      <div>
        <label class="mb-1 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Callback URL (register this with the provider)</label>
        <div class="flex items-center gap-2">
          <code class="min-w-0 flex-1 truncate rounded-md border border-line bg-surface px-2 py-1.5 font-mono text-xs text-fg">{callback}</code>
          <CopyButton value={callback} label="Copy" class="shrink-0 text-xs" />
        </div>
      </div>
      <div class="grid grid-cols-2 gap-3">
        <div>
          <label class="mb-1 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Client ID</label>
          <Input bind:value={clientId} autocomplete="off" />
        </div>
        <div>
          <label class="mb-1 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Client secret</label>
          <Input type="password" bind:value={clientSecret} autocomplete="off" />
        </div>
      </div>
      {#if error}<div transition:slide={{ duration: 150 }} class="text-xs text-danger">{error}</div>{/if}
      <div class="flex justify-end gap-2">
        <Button size="sm" variant="ghost" onclick={() => (openForm = false)}>
          Cancel
        </Button>
        <Button size="sm" disabled={busy || !clientId.trim()} onclick={() => void save()}>
          {busy ? 'Saving' : 'Save credentials'}
        </Button>
      </div>
    </div>
  {/if}
</div>
