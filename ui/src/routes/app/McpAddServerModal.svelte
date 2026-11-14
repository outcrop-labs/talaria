<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import Button from '@/components/ui/Button.svelte'
  import Combobox from '@/components/ui/Combobox.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Modal from '@/components/ui/Modal.svelte'
  import { slide } from '@/lib/motion'
  import { cn } from '@/lib/cn'
  import { sansFill, useUiVoice } from '@/lib/ui-voice'
  import { errorMessage, postJson } from '@/lib/fetch-json'
  import McpField from './McpField.svelte'
  import McpHeaderRows from './McpHeaderRows.svelte'
  import { emptyHeaderRow, headersOfRows, patchServer, slugify, type HeaderRow } from './mcp'

  /** A custom / self-hosted endpoint: name, URL, how it authenticates — as
   *  many auth headers as it wants — and discovery runs on save, so the
   *  server card says "ready", "needs credentials" or "unreachable" the
   *  moment the dialog closes. `onCreated` lets the agent tab attach the new
   *  server to its agent in the same breath. */
  let { onClose, onCreated }: { onClose: () => void; onCreated?: (serverId: string) => void } = $props()

  // An agent's dialog opens this from comms too, where the voice is mono:
  // every `sans` fork below keeps that look as it was.
  const sans = useUiVoice() === 'sans'
  const qc = useQueryClient()
  let label = $state('')
  let slugEdited = $state(false)
  let name = $state('')
  let url = $state('')
  let description = $state('')
  let rows = $state<HeaderRow[]>([emptyHeaderRow()])
  let authMode = $state<'org' | 'per-user'>('org')
  let timeoutSecs = $state('')
  let error = $state<string | null>(null)
  let busy = $state(false)
  const setLabelAndSlug = (v: string) => {
    label = v
    if (!slugEdited) name = slugify(v)
  }
  const valid = $derived(!!name.trim() && /^https?:\/\//.test(url.trim()))

  const submit = async () => {
    if (!valid || busy) return
    busy = true
    error = null
    try {
      const headers = headersOfRows(rows)
      const { server } = await postJson<{ server: { id: string } }>('/api/mcp/servers', {
        name: name.trim(),
        label: label.trim() || undefined,
        url: url.trim(),
        description: description.trim() || null,
        headers: authMode === 'org' && Object.keys(headers).length ? headers : undefined,
        timeoutSecs: timeoutSecs.trim() ? Number(timeoutSecs) : undefined,
        authMode,
      })
      busy = false
      await qc.invalidateQueries({ queryKey: ['mcp-servers'] })
      onCreated?.(server.id)
      onClose()
      // Discovery right away — a server with no tool catalog is half-registered.
      // Its verdict lands on the card (ready / needs credentials / unreachable).
      void patchServer(server.id, { refreshTools: true }).then(() => qc.invalidateQueries({ queryKey: ['mcp-servers'] }))
    } catch (e) {
      busy = false
      error = errorMessage(e)
    }
  }
  const onEnter = (e: KeyboardEvent) => {
    if (e.key === 'Enter') {
      e.preventDefault()
      void submit()
    }
  }
</script>

{#snippet actions()}
  <Button variant="ghost" size="sm" onclick={onClose}>Cancel</Button>
  <Button size="sm" onclick={() => void submit()} disabled={busy || !valid}>
    {#if sans}{busy ? 'Adding' : 'Add server'}{:else}{busy ? 'Registering' : 'Register server'}{/if}
  </Button>
{/snippet}

{#snippet footer()}
  <div class="flex justify-end gap-2">{@render actions()}</div>
{/snippet}

<Modal open {onClose} title={sans ? 'Add custom server' : 'Custom MCP server'} footer={sans ? footer : undefined}>
  <div class={sans ? 'space-y-5' : 'space-y-4'}>
    <McpField label="Name" for="mcp-server-name">
      <Input
        id="mcp-server-name"
        size={sans ? 'sm' : 'md'}
        autofocus
        value={label}
        oninput={(e) => setLabelAndSlug(e.currentTarget.value)}
        onkeydown={onEnter}
        placeholder="GitHub"
      />
      {#if name}
        <div
          class={sans
            ? 'flex items-center gap-2 font-sans text-[13px] leading-5 text-muted'
            : 'mt-1 flex items-center gap-1.5 font-mono text-[11px] text-muted' /* voice-ok: mono branch */}
        >
          {sans ? 'Agents will know it as' : 'agents will know it as'}
          <input
            value={name}
            aria-label="Name agents use"
            oninput={(e) => {
              slugEdited = true
              name = slugify(e.currentTarget.value)
            }}
            class={sans
              ? cn(sansFill, 'rounded-md px-1.5 py-0.5 font-mono text-xs text-fg outline-none focus-visible:outline-2 focus-visible:outline-accent')
              : 'rounded bg-raised px-1.5 py-0.5 font-mono font-medium text-fg outline-none'}
            size={Math.max(name.length, 4)}
          />
        </div>
      {/if}
    </McpField>
    <McpField
      label="Server URL"
      for="mcp-server-url"
      info={sans
        ? 'The server\'s MCP address. Agents never see it. They go through Talaria, which applies the access rules you set.'
        : "The MCP endpoint (streamable HTTP). Agents never see this address; they go through Talaria's gateway, which enforces the access rules you set here."}
    >
      <Input id="mcp-server-url" size={sans ? 'sm' : 'md'} bind:value={url} onkeydown={onEnter} placeholder="https://mcp.example.com/mcp" class={sans ? 'font-mono' : undefined} />
    </McpField>
    <McpField label="Description" for="mcp-server-desc">
      <Input
        id="mcp-server-desc"
        size={sans ? 'sm' : 'md'}
        bind:value={description}
        onkeydown={onEnter}
        placeholder={sans ? 'What agents get from it' : 'What agents get from it (shown in pickers)'}
      />
    </McpField>
    <McpField label={sans ? 'Sign-in' : 'Authentication'}>
      <Combobox
        size={sans ? 'sm' : 'md'}
        options={sans
          ? [
              { value: 'org', label: 'Org account', sub: 'One shared credential for every agent' },
              { value: 'per-user', label: 'Per-user accounts', sub: 'Each person connects their own in Settings' },
            ]
          : [
              { value: 'org', label: 'Org account', sub: 'one shared credential for every agent' },
              { value: 'per-user', label: 'Per-user accounts', sub: 'each person connects their own (Settings → Connections)' },
            ]}
        selected={[authMode]}
        onChange={([v]) => v && (authMode = v as 'org' | 'per-user')}
        placeholder="Auth mode"
      />
    </McpField>
    {#if authMode === 'org'}
      <McpField
        label="Auth headers"
        hint={sans ? 'Leave empty if the server needs none, or signs in with OAuth.' : undefined}
        info={sans
          ? 'Stored encrypted and sent only by Talaria. Never shown again, and never written into an agent config.'
          : 'Stored on the server row and spoken only by the gateway; never rendered into an agent config, never echoed back to this UI. Leave empty for a server that needs none — or one that speaks OAuth, which is detected on save.'}
      >
        <McpHeaderRows bind:rows />
      </McpField>
    {/if}
    {#if sans}
      <details class="group">
        <summary class="cursor-pointer list-none font-sans text-[13px] font-medium leading-5 text-muted transition-colors hover:text-fg">
          <span class="mr-1 inline-block transition-transform group-open:rotate-90">›</span>Advanced
        </summary>
        <div class="mt-3">
          <McpField label="Timeout in seconds" for="mcp-timeout">
            <Input id="mcp-timeout" size="sm" value={timeoutSecs} oninput={(e) => (timeoutSecs = e.currentTarget.value.replace(/[^0-9]/g, ''))} placeholder="120" class="w-32" />
          </McpField>
        </div>
      </details>
      {#if error}<div class="font-sans text-[13px] leading-5 text-danger" role="alert">{error}</div>{/if}
    {:else}
      <details>
        <summary class="cursor-pointer font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim transition-colors hover:text-muted">Advanced</summary><!-- voice-ok: mono branch -->
        <div class="mt-2">
          <McpField label="Timeout (seconds)" for="mcp-timeout">
            <Input id="mcp-timeout" value={timeoutSecs} oninput={(e) => (timeoutSecs = e.currentTarget.value.replace(/[^0-9]/g, ''))} placeholder="120" class="w-32" />
          </McpField>
        </div>
      </details>
      {#if error}<div transition:slide={{ duration: 150 }} class="text-sm text-danger">{error}</div>{/if}
      <div class="flex justify-end gap-2 border-t border-line pt-3">{@render actions()}</div>
    {/if}
  </div>
</Modal>
