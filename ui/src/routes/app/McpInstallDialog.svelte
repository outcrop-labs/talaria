<script lang="ts">
  import Button from '@/components/ui/Button.svelte'
  import Combobox from '@/components/ui/Combobox.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Modal from '@/components/ui/Modal.svelte'
  import Select from '@/components/ui/Select.svelte'
  import McpServerMark from './McpServerMark.svelte'
  import type { LibraryServerRow } from './mcp'

  /** Credential capture for servers that declare required headers — schema-
   *  driven: secret fields mask, choices become selects, placeholders and
   *  descriptions come from the publisher. Per-user mode skips values here and
   *  lets each person connect their own account in Settings. */
  let {
    server: l,
    busy,
    onCancel,
    onInstall,
  }: {
    server: LibraryServerRow
    busy: boolean
    onCancel: () => void
    onInstall: (opts: { headers?: Record<string, string>; authMode?: 'org' | 'per-user' }) => void
  } = $props()

  let authMode = $state<'org' | 'per-user'>('org')
  let values = $state<Record<string, string>>(
    Object.fromEntries(l.requiredHeaders.filter((h) => h.default).map((h) => [h.name, h.default!])),
  )
  const missing = $derived(authMode === 'org' && l.requiredHeaders.some((h) => h.isRequired && !values[h.name]?.trim()))
  const submit = () => {
    if (busy || missing) return
    const headers = Object.fromEntries(Object.entries(values).filter(([, v]) => v.trim()))
    onInstall(authMode === 'org' ? { headers } : { authMode: 'per-user' })
  }
</script>

<Modal open onClose={onCancel} width="max-w-md">
  <div>
    <div class="flex items-center gap-3">
      <McpServerMark title={l.title} domain={l.domain} icon={l.icon} size={36} />
      <div class="min-w-0 flex-1">
        <div class="font-sans text-sm font-semibold text-fg">Connect {l.title}</div>
        {#if l.domain}<div class="font-mono text-[11px] text-muted">{l.domain}</div>{/if}
      </div>
    </div>
    <p class="mt-2 font-sans text-xs text-muted">This server declares credentials. Choose how your org authenticates:</p>
    <div class="mt-3">
      <Combobox
        options={[
          { value: 'org', label: 'Org account', sub: 'one shared credential, stored on the server row' },
          { value: 'per-user', label: 'Per-user accounts', sub: 'each person connects their own in Settings' },
        ]}
        selected={[authMode]}
        onChange={([v]) => v && (authMode = v as 'org' | 'per-user')}
        placeholder="Auth mode"
      />
    </div>
    {#if authMode === 'org'}
      <div class="mt-3 space-y-3">
        {#each l.requiredHeaders as h (h.name)}
          <div>
            <label class="mb-1 flex items-baseline gap-1.5 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">
              {h.name}
              {#if h.isRequired}<span class="text-accent">*</span>{/if}
            </label>
            {#if h.choices?.length}
              <Select size="sm" value={values[h.name] ?? ''} onchange={(e) => (values = { ...values, [h.name]: e.currentTarget.value })} class="w-full">
                <option value="">Pick one</option>
                {#each h.choices as c (c)}
                  <option value={c}>
                    {c}
                  </option>
                {/each}
              </Select>
            {:else}
              <Input
                type={h.isSecret ? 'password' : 'text'}
                value={values[h.name] ?? ''}
                oninput={(e) => (values = { ...values, [h.name]: e.currentTarget.value })}
                placeholder={h.placeholder ?? (h.isSecret ? '•••' : '')}
                autocomplete="off"
              />
            {/if}
            {#if h.description}<div class="mt-1 font-sans text-[11px] text-muted/90">{h.description}</div>{/if}
          </div>
        {/each}
        <div class="font-sans text-[11px] text-muted/80">Spoken only by Talaria's gateway; never rendered into an agent config.</div>
      </div>
    {/if}
    {#if authMode === 'per-user'}
      <p class="mt-3 font-sans text-xs text-muted">
        Nobody gets access until they connect their own account in Settings → Connections. Their assistant then acts as them on this server.
      </p>
    {/if}
    <div class="mt-4 flex justify-end gap-2 border-t border-line pt-3">
      <Button size="sm" variant="ghost" onclick={onCancel}>
        Cancel
      </Button>
      <Button size="sm" disabled={busy || missing} onclick={submit}>
        {busy ? 'Connecting' : 'Add to org'}
      </Button>
    </div>
  </div>
</Modal>
