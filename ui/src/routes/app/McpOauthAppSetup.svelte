<script lang="ts">
  import SettingsField from '@/components/settings/SettingsField.svelte'
  import Button from '@/components/ui/Button.svelte'
  import CopyButton from '@/components/ui/CopyButton.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Modal from '@/components/ui/Modal.svelte'
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
    serverLabel,
    meta,
    onSaved,
    onClose,
  }: {
    serverId: string
    serverLabel: string
    meta: OauthMeta
    onSaved: () => void
    onClose: () => void
  } = $props()

  // A client is already set: this is a change, not a first setup.
  // svelte-ignore state_referenced_locally -- reason: read once when the dialog mounts; saving closes it
  const editing = meta.clientSet
  let clientId = $state('')
  let clientSecret = $state('')
  // svelte-ignore state_referenced_locally -- reason: seeded once from the stored override (or the advertised set) when the dialog mounts
  let scopes = $state((meta.scopesOverride ?? meta.scopes).join(' '))
  let error = $state<string | null>(null)
  let busy = $state(false)

  const guide = $derived(meta.guide)
  const why = $derived(
    editing
      ? 'Change the OAuth app this server signs in through.'
      : meta.dcrRejected
        ? 'The provider only registers apps for sign-in addresses it has approved, and this one is not on its list. Create an OAuth app with the provider, then paste its details here.'
        : 'The provider does not register apps automatically. Create an OAuth app with the provider, then paste its details here.',
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
    onSaved()
  }
</script>

<Modal open {onClose} title={editing ? `OAuth app for ${serverLabel}` : `Set up sign-in for ${serverLabel}`} width="max-w-lg">
  <div class="space-y-5">
    <div class="space-y-2">
      <p class="font-sans text-[13px] leading-5 text-muted">{why}</p>
      {#if guide || meta.documentation}
        <div class="flex flex-wrap gap-x-4 gap-y-1 font-sans text-[13px] leading-5">
          {#if guide}
            <a href={guide.portal} target="_blank" rel="noreferrer" class="text-accent hover:underline">Create the {guide.appName} ↗</a>
          {/if}
          {#if meta.documentation}
            <a href={meta.documentation} target="_blank" rel="noreferrer" class="text-accent hover:underline">Provider docs ↗</a>
          {/if}
        </div>
      {/if}
      {#if guide?.notes.length}
        <ol class="list-decimal space-y-1 pl-5 font-sans text-[13px] leading-5 text-muted">
          {#each guide.notes as n, i (i)}<li>{n}</li>{/each}
        </ol>
      {/if}
    </div>

    <SettingsField label="Callback URL" hint="Register this with the provider, exactly as written.">
      <div class="flex items-center gap-2">
        <code class="min-w-0 flex-1 truncate rounded-md border border-line bg-surface px-2.5 py-1.5 font-mono text-xs leading-5 text-fg">{meta.callbackUrl}</code>
        <CopyButton value={meta.callbackUrl} title="Copy the callback URL" class="shrink-0" />
      </div>
      {#if !/^https:/.test(meta.callbackUrl)}
        <p class="font-sans text-[13px] leading-5 text-warning">
          This callback is plain http, and many providers accept only https. Set a verified hosting domain under Organization, General to get an https one.
        </p>
      {/if}
    </SettingsField>
    <div class="grid gap-4 sm:grid-cols-2">
      <SettingsField label="Client ID" for={`oauth-client-id-${serverId}`}>
        <Input id={`oauth-client-id-${serverId}`} size="sm" bind:value={clientId} autocomplete="off" />
      </SettingsField>
      <SettingsField label="Client secret" for={`oauth-client-secret-${serverId}`}>
        <Input
          id={`oauth-client-secret-${serverId}`}
          size="sm"
          type="password"
          bind:value={clientSecret}
          autocomplete="off"
          placeholder={editing ? 'Leave blank to keep it' : ''}
        />
      </SettingsField>
    </div>
    <SettingsField
      label="Scopes"
      for={`oauth-scopes-${serverId}`}
      hint="Leave blank to ask for what the provider advertises."
      info="What the sign-in asks for, separated by spaces or commas. Filled in with what the server advertised. Some providers need the scopes spelled out here."
    >
      <Input
        id={`oauth-scopes-${serverId}`}
        size="sm"
        bind:value={scopes}
        autocomplete="off"
        placeholder={meta.scopes.join(' ') || 'provider default'}
        class="font-mono"
      />
    </SettingsField>
    {#if error}<div class="font-sans text-[13px] leading-5 text-danger" role="alert">{error}</div>{/if}
  </div>
  {#snippet footer()}
    <div class="flex justify-end gap-2">
      <Button size="sm" variant="ghost" onclick={onClose}>Cancel</Button>
      <Button size="sm" disabled={busy || !clientId.trim()} onclick={() => void save()}>{busy ? 'Saving' : 'Save'}</Button>
    </div>
  {/snippet}
</Modal>
