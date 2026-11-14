<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import SettingsRowFix from '@/components/settings/SettingsRowFix.svelte'
  import SettingsRowMenu from '@/components/settings/SettingsRowMenu.svelte'
  import Button from '@/components/ui/Button.svelte'
  import Input from '@/components/ui/Input.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import QueryState from '@/components/ui/QueryState.svelte'
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import { getList, putJson } from '@/lib/fetch-json'
  import { slide } from '@/lib/motion'
  import { toastError } from '@/lib/toast.svelte'
  import { composeHeader, headerFields, type HeaderDecl, type HeaderField } from '@/routes/app/mcp'

  /** Per-user MCP connected accounts: servers the org registered in per-user
   *  auth mode. Connect yours (a token header, sealed at rest) and your
   *  assistant starts carrying the server — acting as YOU there.
   *
   *  Rows only: `IntegrationsSection` owns the card and its title, and these
   *  follow Google Workspace in it as accounts of the same kind. */
  interface McpConnection {
    id: string
    name: string
    label: string
    description: string | null
    requiredHeaders: HeaderDecl[]
    authKind: 'oauth' | 'headers'
    connected: boolean
    /** Connected once, and the provider has since refused to refresh it
     *  (found by the hourly MCP health check): sign in again. */
    needsReconnect?: boolean
  }

  /** A server registered before it carried declarations: one auth header is
   *  what such servers have always wanted in practice. */
  const GUESSED: HeaderDecl[] = [
    { name: 'Authorization', description: null, isRequired: true, isSecret: true, placeholder: 'Bearer your-token', default: null, choices: null, value: null, variables: null },
  ]

  const fieldsOf = (s: McpConnection): HeaderField[] =>
    (s.requiredHeaders.length ? s.requiredHeaders : GUESSED).flatMap((h) => headerFields(h))

  const qc = useQueryClient()
  // An org that runs no per-user servers adds no rows. That is a statement
  // about the org, so only a REAL empty list may make it — a failed read
  // would otherwise quietly remove a connected account from view.
  const query = createQuery(() => ({
    queryKey: ['me-mcp'],
    queryFn: (): Promise<McpConnection[]> => getList<McpConnection>('/api/me/mcp', 'servers'),
  }))
  let connecting = $state<string | null>(null)
  let values = $state<Record<string, string>>({})

  // Opening a form starts clean — a stale value must never leak across servers.
  const toggle = (s: McpConnection) => {
    values = {}
    connecting = connecting === s.id ? null : s.id
  }

  // The OAuth popup announces completion — flip to "connected" live.
  $effect(() => {
    const onMsg = (e: MessageEvent) => {
      if (e.origin === window.location.origin && (e.data as { type?: string })?.type === 'talaria:mcp-oauth-done') {
        void qc.invalidateQueries({ queryKey: ['me-mcp'] })
      }
    }
    window.addEventListener('message', onMsg)
    return () => window.removeEventListener('message', onMsg)
  })

  const put = async (serverId: string, headers: Record<string, string> | null) => {
    try {
      await putJson<{ ok: true }>('/api/me/mcp', { serverId, headers })
    } catch (e) {
      // Leave the form open with what was typed — a failed connect is retryable.
      toastError('Could not save that connection', e)
      return
    }
    connecting = null
    await qc.invalidateQueries({ queryKey: ['me-mcp'] })
  }

  // The user's own credentials only: fixed publisher headers ride the org
  // row, so a declared header contributes its typed variables and nothing else.
  const submit = (s: McpConnection) => {
    const headers: Record<string, string> = {}
    for (const h of s.requiredHeaders.length ? s.requiredHeaders : GUESSED) {
      const fs = headerFields(h)
      if (!fs.length || !fs.some((f) => (values[f.key] ?? '').trim())) continue
      headers[h.name] = composeHeader(h, values)
    }
    void put(s.id, headers)
  }

  const signIn = (s: McpConnection) => void window.open(`/api/mcp/oauth/start?server=${s.id}&scope=me`, 'talaria-mcp-oauth', 'width=620,height=780')
</script>

<QueryState query={query} errorTitle="Could not load your tool accounts" errorVariant="inline">
  {#snippet skeleton()}<div class="px-5 py-4"><SkeletonRows rows={2} /></div>{/snippet}
  {#snippet children(data)}
    {#each data ?? [] as s (s.id)}
      <div>
        <SettingsRow label={s.label} hint={s.description ?? undefined}>
          {#snippet status()}
            {#if s.connected}
              <StatusDot status="ok" />Connected
            {:else if s.authKind === 'oauth' && s.needsReconnect}
              <span
                class="flex items-center gap-2"
                title="The provider refused to refresh your sign-in, so your assistant can no longer use this server as you."
              >
                <StatusDot status="warn" />Needs sign-in
              </span>
            {:else}
              <StatusDot status="idle" />Not connected
            {/if}
          {/snippet}
          {#if s.connected}
            <SettingsRowMenu
              label={`Actions for ${s.label}`}
              items={[{ label: `Disconnect ${s.label}`, danger: true, onSelect: () => void put(s.id, null) }]}
            />
          {:else if s.authKind === 'oauth' && s.needsReconnect}
            <SettingsRowFix
              label={`Fix ${s.label}: sign in again`}
              title="Your sign-in expired at the provider. Sign in again so your assistant can act as you here."
              onclick={() => signIn(s)}
            />
            <SettingsRowMenu label={`Actions for ${s.label}`} items={[{ label: 'Sign in again', onSelect: () => signIn(s) }]} />
          {:else}
            <!-- Not connected is a choice, not a fault: the same quiet action a fix has, under its own word. -->
            {#if s.authKind === 'oauth'}
              <SettingsRowFix
                text="Connect"
                label={`Connect ${s.label}`}
                title="Sign in with the provider. Your assistant then acts as you on this server."
                onclick={() => signIn(s)}
              />
            {:else}
              <SettingsRowFix
                text={connecting === s.id ? 'Cancel' : 'Connect'}
                label={connecting === s.id ? `Cancel connecting ${s.label}` : `Connect ${s.label}`}
                onclick={() => toggle(s)}
              />
            {/if}
            <SettingsRowMenu label={`Actions for ${s.label}`} items={[]} />
          {/if}
        </SettingsRow>
        {#if connecting === s.id}
          <div transition:slide={{ duration: 150 }}>
            <!-- Publisher-declared credentials drive the form: one field
                 per template variable, composed back into its header. A
                 server without declarations falls back to one auth header. -->
            <div class="space-y-3 border-t border-line-subtle px-5 py-4">
              {#each fieldsOf(s) as f (f.key)}
                <label class="flex items-center gap-6">
                  <span class="min-w-0 flex-1">
                    <span class="block truncate font-mono text-xs text-muted" title={f.header === f.label ? f.label : `${f.label} → ${f.header}`}>
                      {f.label}{#if f.header !== f.label} → {f.header}{/if}
                    </span>
                    {#if f.description}<span class="block font-sans text-[13px] leading-5 text-muted">{f.description}</span>{/if}
                  </span>
                  <Input
                    size="sm"
                    class="w-[280px] shrink-0"
                    type={f.isSecret ? 'password' : 'text'}
                    value={values[f.key] ?? ''}
                    oninput={(e) => (values = { ...values, [f.key]: e.currentTarget.value })}
                    placeholder={f.placeholder ?? ''}
                    autocomplete="off"
                  />
                </label>
              {/each}
              <div class="flex justify-end">
                <Button size="sm" variant="outline" disabled={!fieldsOf(s).some((f) => (values[f.key] ?? '').trim())} onclick={() => submit(s)}>
                  Save and connect
                </Button>
              </div>
            </div>
          </div>
        {/if}
      </div>
    {/each}
  {/snippet}
</QueryState>
