<script lang="ts">
  import Button from '@/components/ui/Button.svelte'
  import Combobox from '@/components/ui/Combobox.svelte'
  import Disclosure from '@/components/ui/Disclosure.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Modal from '@/components/ui/Modal.svelte'
  import Select from '@/components/ui/Select.svelte'
  import { useUiVoice } from '@/lib/ui-voice'
  import McpField from './McpField.svelte'
  import McpHeaderRows from './McpHeaderRows.svelte'
  import McpServerMark from './McpServerMark.svelte'
  import {
    composeHeaders,
    emptyHeaderRow,
    headerFields,
    headersOfRows,
    literalHeaders,
    type HeaderField,
    type HeaderRow,
    type InstallOpts,
    type LibraryServerRow,
  } from './mcp'

  /** Credential capture for servers that declare what they need. Two
   *  shapes: HOSTED servers (a registry `value` template becomes one field
   *  per variable composed back into the final header; a value-less
   *  declaration prompts for the whole header; a fixed value applies
   *  itself — plus a folded "other headers" door, because the registry's
   *  declarations are often incomplete) and PACKAGE installs (npm/pypi/oci —
   *  the declared environment variables use the exact same Input schema,
   *  plus a fill per required run flag, plus an explicit acknowledgement
   *  that third-party code will run in a container with the credentials
   *  typed here). */
  let {
    server: l,
    busy,
    onCancel,
    onInstall,
  }: {
    server: LibraryServerRow
    busy: boolean
    onCancel: () => void
    onInstall: (opts: InstallOpts) => void
  } = $props()

  // The marketplace opens this from an agent's dialog in comms too, where the
  // voice is mono: the second template below is that dialog as it was.
  const sans = useUiVoice() === 'sans'
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
  // svelte-ignore state_referenced_locally -- reason: defaults seeded once from the selected server's declared fields; the dialog mounts per install
  let values = $state<Record<string, string>>(
    Object.fromEntries(fields.filter((f) => f.default).map((f) => [f.key, f.default!])),
  )
  let argValues = $state<Record<string, string>>({})
  /** Headers the registry did not declare but the server may still want. */
  let extraRows = $state<HeaderRow[]>([emptyHeaderRow()])

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
      const headers = { ...composeHeaders(l.requiredHeaders, values), ...headersOfRows(extraRows) }
      onInstall(Object.keys(headers).length ? { headers } : {})
    } else {
      // Fixed publisher-set headers ride on the org row in both modes; the
      // per-person secret is entered later in Settings → Connections.
      const headers = literalHeaders(l.requiredHeaders)
      onInstall(Object.keys(headers).length ? { headers, authMode: 'per-user' } : { authMode: 'per-user' })
    }
  }
</script>

<!-- The declared credentials. A package's environment variables share the
     schema but never offered a choice list or a header name. -->
{#snippet declared(hosted: boolean)}
  {#each fields as f (f.key)}
    <McpField
      label={f.label}
      for={`install-${f.key}`}
      required={f.isRequired}
      header={hosted && f.header !== f.label ? f.header : null}
      hint={f.description}
    >
      {#if hosted && f.choices?.length}
        <Select id={`install-${f.key}`} size="sm" value={values[f.key] ?? ''} onchange={(e) => (values = { ...values, [f.key]: e.currentTarget.value })} class="w-full">
          <option value="">Pick one</option>
          {#each f.choices as c (c)}
            <option value={c}>
              {c}
            </option>
          {/each}
        </Select>
      {:else}
        <Input
          id={`install-${f.key}`}
          size={sans ? 'sm' : 'md'}
          type={f.isSecret ? 'password' : 'text'}
          value={values[f.key] ?? ''}
          oninput={(e) => (values = { ...values, [f.key]: e.currentTarget.value })}
          placeholder={f.placeholder ?? (f.isSecret ? '•••' : '')}
          autocomplete="off"
        />
      {/if}
    </McpField>
  {/each}
{/snippet}

{#snippet runFlags()}
  {#each argFields as f (f.index)}
    <McpField label={f.name} for={`install-arg-${f.index}`} required hint={f.description}>
      <Input
        id={`install-arg-${f.index}`}
        size={sans ? 'sm' : 'md'}
        value={argValues[f.index] ?? ''}
        oninput={(e) => (argValues = { ...argValues, [f.index]: e.currentTarget.value })}
        placeholder={f.placeholder ?? ''}
        autocomplete="off"
      />
    </McpField>
  {/each}
{/snippet}

{#snippet signIn()}
  <Combobox
    size={sans ? 'sm' : 'md'}
    options={sans
      ? [
          { value: 'org', label: 'Org account', sub: 'One shared credential for every agent' },
          { value: 'per-user', label: 'Per-user accounts', sub: 'Each person connects their own in Settings' },
        ]
      : [
          { value: 'org', label: 'Org account', sub: 'one shared credential, stored on the server row' },
          { value: 'per-user', label: 'Per-user accounts', sub: 'each person connects their own in Settings' },
        ]}
    selected={[authMode]}
    onChange={([v]) => v && (authMode = v as 'org' | 'per-user')}
    placeholder="Auth mode"
  />
{/snippet}

{#snippet actions()}
  <Button size="sm" variant="ghost" onclick={onCancel}>Cancel</Button>
  <Button size="sm" disabled={busy || blocked} onclick={submit}>
    {busy ? 'Installing' : isPkg ? 'Install package' : 'Add to org'}
  </Button>
{/snippet}

{#snippet footer()}
  <div class="flex justify-end gap-2">{@render actions()}</div>
{/snippet}

{#if sans}
  <Modal open onClose={onCancel} title={`Add ${l.title}`} {footer} width="max-w-md">
    <div class="space-y-5">
      {#if isPkg}
        <!-- The package path: acknowledge the runtime, fill the declared env,
             fill the required run flags. One org-shared container — there is
             no per-user mode for packages. -->
        <label class="flex items-start gap-3 rounded-md border border-warning/40 bg-warning/5 p-3 font-sans text-[13px] leading-5 text-fg">
          <input type="checkbox" bind:checked={acknowledged} class="mt-1 accent-accent" />
          <span>
            {#if l.tier === 'community'}This is community-built code. Check who published it first.{/if}
            Installing runs the publisher's code in a locked-down container on your deployment, with the credentials you enter here. Only an admin can install one.
          </span>
        </label>
        {#if fields.length === 0 && argFields.length === 0}
          <p class="font-sans text-[13px] leading-5 text-muted">This package asks for no credentials.</p>
        {/if}
        {@render declared(false)}
        {@render runFlags()}
        <p class="font-sans text-[13px] leading-5 text-muted">Credentials are stored encrypted and given only to the container.</p>
      {:else}
        <McpField label="Sign-in" hint="This server asks for credentials. Choose how your org provides them.">
          {@render signIn()}
        </McpField>
        {#if authMode === 'org' && fields.length === 0}
          <p class="font-sans text-[13px] leading-5 text-muted">The publisher sets every credential this server needs. There is nothing to enter.</p>
        {:else if authMode === 'org'}
          {@render declared(true)}
        {:else}
          <p class="font-sans text-[13px] leading-5 text-muted">
            Nobody gets access until they connect their own account under Settings, Connections. Their assistant then acts as them.
          </p>
        {/if}
        {#if authMode === 'org'}
          <!-- Registry declarations are what the publisher wrote down, which
               is not always what the server checks. The door is folded so the
               common case stays one field; after install, a 401 from discovery
               asks for the rest on the row. -->
          <Disclosure title="Other headers, if the server wants more">
            <div class="py-1">
              <McpHeaderRows bind:rows={extraRows} />
            </div>
          </Disclosure>
          <p class="font-sans text-[13px] leading-5 text-muted">Credentials are stored encrypted and sent only by Talaria.</p>
        {/if}
      {/if}
    </div>
  </Modal>
{:else}
  <Modal open onClose={onCancel} width="max-w-md">
    <div>
      <div class="flex items-center gap-3">
        <McpServerMark title={l.title} domain={l.domain} icon={l.icon} size={36} />
        <div class="min-w-0 flex-1">
          <div class="font-sans text-sm font-semibold text-fg">Connect {l.title}</div>
          {#if l.domain}<div class="font-mono text-[11px] text-muted">{l.domain}</div>{/if}<!-- voice-ok: mono branch -->
        </div>
      </div>

      {#if isPkg}
        <div class="mt-3 rounded-lg border border-warning/40 bg-warning/5 p-3">
          <label class="flex items-start gap-2.5 font-sans text-xs leading-relaxed text-fg">
            <input type="checkbox" bind:checked={acknowledged} class="mt-0.5" />
            <span>
              {l.tier === 'community'
                ? 'This server is COMMUNITY-BUILT third-party code. Installing it runs its package in a Docker container on your deployment, with the credentials you enter below. Review who published it before continuing.'
                : 'Installing this server runs its package in a Docker container on your deployment, with the credentials you enter below.'}
              Talaria contains the container (unprivileged user, read-only filesystem, no host mounts, no privileges, capped CPU/memory, no fleet network) — the code inside it is still the publisher's. Only an admin can install one.
            </span>
          </label>
        </div>
        {#if fields.length === 0 && argFields.length === 0}
          <p class="mt-3 font-sans text-xs text-muted">This package declares no credentials — nothing to enter.</p>
        {/if}
        {#if fields.length > 0}
          <div class="mt-3 space-y-3">{@render declared(false)}</div>
        {/if}
        {#if argFields.length > 0}
          <div class="mt-3 space-y-3">{@render runFlags()}</div>
        {/if}
        <div class="mt-3 font-sans text-[11px] text-muted/80">Credentials are sealed at rest and spoken only to the container; never rendered into an agent config.</div><!-- voice-ok: mono branch -->
      {:else}
        <p class="mt-2 font-sans text-xs text-muted">This server declares credentials. Choose how your org authenticates:</p>
        <div class="mt-3">{@render signIn()}</div>
        {#if authMode === 'org' && fields.length === 0}
          <p class="mt-3 font-sans text-xs text-muted">
            The publisher sets every credential this server needs — there's nothing to enter.
          </p>
        {:else if authMode === 'org'}
          <div class="mt-3 space-y-3">
            {@render declared(true)}
            <div class="font-sans text-[11px] text-muted/80">Spoken only by Talaria's gateway; never rendered into an agent config.</div><!-- voice-ok: mono branch -->
          </div>
        {:else if authMode === 'per-user'}
          <p class="mt-3 font-sans text-xs text-muted">
            Nobody gets access until they connect their own account in Settings → Connections. Their assistant then acts as them on this server.
          </p>
        {/if}
        {#if authMode === 'org'}
          <Disclosure title="Other headers (if the server wants more)" class="mt-3">
            <div class="p-2">
              <McpHeaderRows bind:rows={extraRows} />
            </div>
          </Disclosure>
        {/if}
      {/if}

      <div class="mt-4 flex justify-end gap-2 border-t border-line pt-3">{@render actions()}</div>
    </div>
  </Modal>
{/if}
