<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import Button from '@/components/ui/Button.svelte'
  import Combobox from '@/components/ui/Combobox.svelte'
  import InfoTip from '@/components/ui/InfoTip.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Modal from '@/components/ui/Modal.svelte'
  import { slide } from '@/lib/motion'
  import { patchServer, slugify } from './mcp'

  let { onClose }: { onClose: () => void } = $props()

  const qc = useQueryClient()
  let label = $state('')
  let slugEdited = $state(false)
  let name = $state('')
  let url = $state('')
  let description = $state('')
  let headerKey = $state('')
  let headerVal = $state('')
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
    const r = await fetch('/api/mcp/servers', {
      method: 'POST',
      credentials: 'same-origin',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({
        name: name.trim(),
        label: label.trim() || undefined,
        url: url.trim(),
        description: description.trim() || null,
        headers: headerKey.trim() && headerVal.trim() ? { [headerKey.trim()]: headerVal } : undefined,
        timeoutSecs: timeoutSecs.trim() ? Number(timeoutSecs) : undefined,
        authMode,
      }),
    })
    busy = false
    if (!r.ok) {
      error = ((await r.json().catch(() => ({}))) as { error?: string }).error ?? 'failed'
      return
    }
    const { server } = (await r.json()) as { server: { id: string } }
    await qc.invalidateQueries({ queryKey: ['mcp-servers'] })
    // Kick discovery right away — a server with no tool catalog is half-registered.
    void patchServer(server.id, { refreshTools: true }).then(() => qc.invalidateQueries({ queryKey: ['mcp-servers'] }))
    onClose()
  }
  const onEnter = (e: KeyboardEvent) => {
    if (e.key === 'Enter') {
      e.preventDefault()
      void submit()
    }
  }
</script>

<Modal open {onClose} title="Custom MCP server">
  <div class="space-y-4">
    <div>
      <label class="mb-1 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Name</label>
      <Input autofocus value={label} oninput={(e) => setLabelAndSlug(e.currentTarget.value)} onkeydown={onEnter} placeholder="GitHub" />
      {#if name}
        <div class="mt-1 flex items-center gap-1.5 font-mono text-[11px] text-muted">
          agents will know it as
          <input
            value={name}
            oninput={(e) => {
              slugEdited = true
              name = slugify(e.currentTarget.value)
            }}
            class="rounded bg-raised px-1.5 py-0.5 font-mono font-medium text-fg outline-none"
            size={Math.max(name.length, 4)}
          />
        </div>
      {/if}
    </div>
    <div>
      <label class="mb-1 flex items-center gap-1.5 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">
        Server URL
        <InfoTip text="The MCP endpoint (streamable HTTP). Agents never see this address — they go through Talaria's gateway, which enforces the access rules you set here." />
      </label>
      <Input bind:value={url} onkeydown={onEnter} placeholder="https://mcp.example.com/mcp" />
    </div>
    <div>
      <label class="mb-1 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Description</label>
      <Input bind:value={description} onkeydown={onEnter} placeholder="What agents get from it (shown in pickers)" />
    </div>
    <div>
      <label class="mb-1 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Authentication</label>
      <Combobox
        options={[
          { value: 'org', label: 'Org account', sub: 'one shared credential for every agent' },
          { value: 'per-user', label: 'Per-user accounts', sub: 'each person connects their own (Settings → Connections)' },
        ]}
        selected={[authMode]}
        onChange={([v]) => v && (authMode = v as 'org' | 'per-user')}
        placeholder="Auth mode"
      />
    </div>
    {#if authMode === 'org'}
      <div class="grid grid-cols-[10rem_1fr] gap-2">
        <div>
          <label class="mb-1 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Auth header</label>
          <Input bind:value={headerKey} onkeydown={onEnter} placeholder="Authorization" />
        </div>
        <div>
          <label class="mb-1 flex items-center gap-1.5 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">
            Value
            <InfoTip text="Stored on the server row and spoken only by the gateway — never rendered into an agent config, never echoed back to this UI." />
          </label>
          <Input type="password" bind:value={headerVal} onkeydown={onEnter} placeholder="Bearer …" autocomplete="off" />
        </div>
      </div>
    {/if}
    <details>
      <summary class="cursor-pointer font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim transition-colors hover:text-muted">Advanced</summary>
      <div class="mt-2">
        <label class="mb-1 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Timeout (seconds)</label>
        <Input value={timeoutSecs} oninput={(e) => (timeoutSecs = e.currentTarget.value.replace(/[^0-9]/g, ''))} placeholder="120" class="w-32" />
      </div>
    </details>
    {#if error}<div transition:slide={{ duration: 150 }} class="text-sm text-danger">{error}</div>{/if}
    <div class="flex justify-end gap-2 border-t border-line pt-3">
      <Button variant="ghost" size="sm" onclick={onClose}>
        Cancel
      </Button>
      <Button size="sm" onclick={() => void submit()} disabled={busy || !valid}>
        {busy ? 'Registering' : 'Register server'}
      </Button>
    </div>
  </div>
</Modal>
