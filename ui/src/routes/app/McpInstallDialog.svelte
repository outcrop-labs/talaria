<script lang="ts">
  import Button from '@/components/ui/Button.svelte'
  import Combobox from '@/components/ui/Combobox.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Modal from '@/components/ui/Modal.svelte'
  import Select from '@/components/ui/Select.svelte'
  import McpServerMark from './McpServerMark.svelte'
  import { composeHeaders, headerFields, literalHeaders, type HeaderField, type LibraryServerRow } from './mcp'

  /** Credential capture for servers that declare what they need. Two
   *  shapes: HOSTED servers (a registry `value` template becomes one field
   *  per variable composed back into the final header; a value-less
   *  declaration prompts for the whole header; a fixed value applies
   *  itself) and PACKAGE installs (npm/pypi/oci — the declared environment
   *  variables use the exact same Input schema, plus a fill per required
   *  run flag, plus an explicit acknowledgement that third-party code will
   *  run in a container with the credentials typed here). */
  let {
    server: l,
    busy,
    onCancel,
    onInstall,
  }: {
    server: LibraryServerRow
    busy: boolean
    onCancel: () => void
    onInstall: (opts: { headers?: Record<string, string>; env?: Record<string, string>; argValues?: Record<string, string>; authMode?: 'org' | 'per-user' }) => void
  } = $props()

  const isPkg = $derived(!!l.package)
  const pkg = $derived(l.package)

  let authMode = $state<'org' | 'per-user'>('org')
  const fields = $derived(isPkg ? headerFieldsOver(pkg!.declaredEnv) : l.requiredHeaders.flatMap((h) => headerFields(h)))
  /** Run flags that declare a placeholder and no default ask for a fill. */
  const argFields = $derived.by(() => {
    if (!pkg) return [] as Array<{ index: number; name: string; placeholder: string | null; description: string | null }>
    return pkg.runArgs
      .map((a, index) => ({ a, index }))
      .filter(({ a }) => (a.placeholder || a.isRequired) && !a.default)
      .map(({ a, index }) => ({ index, name: a.name ?? 'arg', placeholder: a.placeholder ?? null, description: a.description ?? null }))
  })
  const missingArgs = $derived(argFields.some((f) => !(argValues[f.index] ?? '').trim()))
  let acknowledged = $state(false)
  let values = $state<Record<string, string>>(
    Object.fromEntries(fields.filter((f) => f.default).map((f) => [f.key, f.default!])),
  )
  let argValues = $state<Record<string, string>>({})

  // headerFields over env declarations — the Input shape is identical, so
  // the helpers compose env values exactly as they do header values.
  function headerFieldsOver(decls: LibraryServerRow['requiredHeaders']): HeaderField[] {
    return decls.flatMap((h) => headerFields(h))
  }

  const missing = $derived(authMode === 'org' && fields.some((f) => f.isRequired && !values[f.key]?.trim()))
  const blocked = $derived(isPkg ? (!acknowledged || missingArgs) : (authMode === 'org' && missing))
  const submit = () => {
    if (busy || blocked) return
    if (isPkg) {
      // Env declarations share the Input schema with headers — templates
      // compose, fixed values apply themselves.
      const env = composeHeaders(pkg!.declaredEnv, values)
      onInstall({
        env,
        argValues: Object.fromEntries(Object.entries(argValues).filter(([, v]) => v.trim())),
      })
    } else if (authMode === 'org') {
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

    {#if isPkg}
      <!-- The package path: acknowledge the runtime, fill the declared env,
           fill the required run flags. One org-shared container — there is
           no per-user mode for packages. -->
      <div class="mt-3 rounded-lg border border-warning/40 bg-warning/5 p-3">
        <label class="flex items-start gap-2.5 font-sans text-xs leading-relaxed text-fg">
          <input type="checkbox" bind:checked={acknowledged} class="mt-0.5" />
          <span>
            {l.tier === 'community'
              ? 'This server is COMMUNITY-BUILT third-party code. Installing it runs its package in a Docker container on your deployment, with the credentials you enter below. Review who published it before continuing.'
              : 'Installing this server runs its package in a Docker container on your deployment, with the credentials you enter below.'}
            Talaria contains the container (no privileges, capped CPU/memory, no fleet network) — the code inside it is still the publisher's.
          </span>
        </label>
      </div>
      {#if fields.length === 0 && argFields.length === 0}
        <p class="mt-3 font-sans text-xs text-muted">This package declares no credentials — nothing to enter.</p>
      {/if}
      {#if fields.length > 0}
        <div class="mt-3 space-y-3">
          {#each fields as f (f.key)}
            <div>
              <label class="mb-1 flex items-baseline gap-1.5 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">
                {f.label}
                {#if f.isRequired}<span class="text-accent">*</span>{/if}
              </label>
              <Input
                type={f.isSecret ? 'password' : 'text'}
                value={values[f.key] ?? ''}
                oninput={(e) => (values = { ...values, [f.key]: e.currentTarget.value })}
                placeholder={f.placeholder ?? (f.isSecret ? '•••' : '')}
                autocomplete="off"
              />
              {#if f.description}<div class="mt-1 font-sans text-[11px] text-muted/90">{f.description}</div>{/if}
            </div>
          {/each}
        </div>
      {/if}
      {#if argFields.length > 0}
        <div class="mt-3 space-y-3">
          {#each argFields as f (f.index)}
            <div>
              <label class="mb-1 flex items-baseline gap-1.5 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">
                {f.name}
                <span class="text-accent">*</span>
              </label>
              <Input
                value={argValues[f.index] ?? ''}
                oninput={(e) => (argValues = { ...argValues, [f.index]: e.currentTarget.value })}
                placeholder={f.placeholder ?? ''}
                autocomplete="off"
              />
              {#if f.description}<div class="mt-1 font-sans text-[11px] text-muted/90">{f.description}</div>{/if}
            </div>
          {/each}
        </div>
      {/if}
      <div class="mt-3 font-sans text-[11px] text-muted/80">Credentials are sealed at rest and spoken only to the container; never rendered into an agent config.</div>
    {:else}
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
    {/if}

    <div class="mt-4 flex justify-end gap-2 border-t border-line pt-3">
      <Button size="sm" variant="ghost" onclick={onCancel}>
        Cancel
      </Button>
      <Button size="sm" disabled={busy || blocked} onclick={submit}>
        {busy ? 'Installing' : isPkg ? 'Install package' : 'Add to org'}
      </Button>
    </div>
  </div>
</Modal>
