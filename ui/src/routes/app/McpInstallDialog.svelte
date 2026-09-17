<script lang="ts">
  import Button from '@/components/ui/Button.svelte'
  import Combobox from '@/components/ui/Combobox.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Modal from '@/components/ui/Modal.svelte'
  import Select from '@/components/ui/Select.svelte'
  import McpServerMark from './McpServerMark.svelte'
  import { composeHeaders, headerFields, literalHeaders, type LibraryServerRow } from './mcp'

  /** Credential capture for servers that declare required headers — schema-
   *  driven: a registry `value` template ("Bearer {api_key}") becomes one
   *  field per variable composed back into the final header; a value-less
   *  declaration prompts for the whole header; a fixed value applies itself.
   *  Secret fields mask, choices become selects, placeholders and
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
  const fields = $derived(l.requiredHeaders.flatMap((h) => headerFields(h)))
  let values = $state<Record<string, string>>(
    Object.fromEntries(fields.filter((f) => f.default).map((f) => [f.key, f.default!])),
  )
  const missing = $derived(authMode === 'org' && fields.some((f) => f.isRequired && !values[f.key]?.trim()))
  const submit = () => {
    if (busy || missing) return
    if (authMode === 'org') {
      const headers = composeHeaders(l.requiredHeaders, values)
      onInstall(Object.keys(headers).length ? { headers } : {})
    } else {
      // Fixed publisher-set headers ride on the org row in both modes; the
      // per-person secret is entered later in Settings → Connections.
      const headers = literalHeaders(l.requiredHeaders)
      onInstall(Object.keys(headers).length ? { headers, authMode: 'per-user' } : { authMode: 'per-user' })
    }
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
    {#if authMode === 'org' && fields.length === 0}
      <p class="mt-3 font-sans text-xs text-muted">
        The publisher sets every credential this server needs — there's nothing to enter.
      </p>
    {:else if authMode === 'org'}
      <div class="mt-3 space-y-3">
        {#each fields as f (f.key)}
          <div>
            <label class="mb-1 flex items-baseline gap-1.5 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">
              {f.label}
              {#if f.isRequired}<span class="text-accent">*</span>{/if}
              {#if f.header !== f.label}<span class="normal-case text-ink-dim/70">→ {f.header}</span>{/if}
            </label>
            {#if f.choices?.length}
              <Select size="sm" value={values[f.key] ?? ''} onchange={(e) => (values = { ...values, [f.key]: e.currentTarget.value })} class="w-full">
                <option value="">Pick one</option>
                {#each f.choices as c (c)}
                  <option value={c}>
                    {c}
                  </option>
                {/each}
              </Select>
            {:else}
              <Input
                type={f.isSecret ? 'password' : 'text'}
                value={values[f.key] ?? ''}
                oninput={(e) => (values = { ...values, [f.key]: e.currentTarget.value })}
                placeholder={f.placeholder ?? (f.isSecret ? '•••' : '')}
                autocomplete="off"
              />
            {/if}
            {#if f.description}<div class="mt-1 font-sans text-[11px] text-muted/90">{f.description}</div>{/if}
          </div>
        {/each}
        <div class="font-sans text-[11px] text-muted/80">Spoken only by Talaria's gateway; never rendered into an agent config.</div>
      </div>
    {:else if authMode === 'per-user'}
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
