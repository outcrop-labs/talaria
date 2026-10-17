<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import { X } from '@lucide/svelte'
  import Button from '@/components/ui/Button.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Modal from '@/components/ui/Modal.svelte'
  import Select from '@/components/ui/Select.svelte'
  import { slide } from '@/lib/motion'
  import McpHeaderRows from './McpHeaderRows.svelte'
  import McpServerMark from './McpServerMark.svelte'
  import {
    composeHeaders,
    emptyHeaderRow,
    headerFields,
    headersOfRows,
    patchServer,
    type HeaderField,
    type HeaderRow,
    type McpServerRow,
  } from './mcp'

  /** The org credentials of one server, edited after the fact. Two shapes,
   *  same as the install dialog: a server that DECLARED its headers renders
   *  one field per declared variable and composes them back into the final
   *  header; one that did not gets free name/value rows. Stored keys show as
   *  •••: the api answers key NAMES only (values are sealed and never leave
   *  it), so this form writes a MERGE (set these keys, remove those, keep the
   *  rest) and never has to see one. Saving re-runs discovery, so the card's
   *  answer updates in the same breath. */
  let {
    server: s,
    onClose,
    onSaved,
  }: {
    server: McpServerRow
    onClose: () => void
    onSaved?: () => void
  } = $props()

  const qc = useQueryClient()
  // svelte-ignore state_referenced_locally -- reason: the form seeds once from the server row; the modal mounts per edit
  const declared = s.requiredHeaders ?? []
  const fields: HeaderField[] = declared.flatMap((h) => headerFields(h))
  const declaredNames = new Set(declared.map((h) => h.name))
  // svelte-ignore state_referenced_locally -- reason: see above
  const storedKeys = s.headerKeys ?? []
  // Stored keys the declared form does not already own — shown with their
  // own remove control, since no field will overwrite them.
  const extraStored = storedKeys.filter((k) => !declaredNames.has(k))

  let values = $state<Record<string, string>>({})
  let rows = $state<HeaderRow[]>(fields.length ? [] : [emptyHeaderRow()])
  let removed = $state<string[]>([])
  let busy = $state(false)
  let error = $state<string | null>(null)
  let discovery = $state<string | null>(null)

  const merge = $derived.by(() => {
    const out: Record<string, string | null> = {}
    if (fields.length) {
      for (const [k, v] of Object.entries(composeHeaders(declared, values))) out[k] = v
    }
    for (const [k, v] of Object.entries(headersOfRows(rows))) out[k] = v
    for (const k of removed) if (!(k in out)) out[k] = null
    return out
  })
  const dirty = $derived(Object.keys(merge).length > 0)

  const save = async () => {
    if (!dirty || busy) return
    busy = true
    error = null
    discovery = null
    const e = await patchServer(s.id, { mergeHeaders: merge })
    if (e) {
      busy = false
      error = e
      return
    }
    // The point of a credential is a catalog: prove it now, and say what the
    // server answered if it still refuses.
    const d = await patchServer(s.id, { refreshTools: true })
    busy = false
    await qc.invalidateQueries({ queryKey: ['mcp-servers'] })
    onSaved?.()
    if (d) {
      discovery = d
      return
    }
    onClose()
  }
</script>

<Modal open {onClose} width="max-w-md">
  <div>
    <div class="flex items-center gap-3">
      <McpServerMark title={s.label} domain={(() => { try { const u = new URL(s.url); return u.protocol.startsWith('http') ? u.hostname.replace(/^mcp\./, '') : null } catch { return null } })()} size={36} />
      <div class="min-w-0 flex-1">
        <div class="font-sans text-sm font-semibold text-fg">Credentials for {s.label}</div>
        <div class="font-sans text-xs text-muted">
          {s.authMode === 'per-user'
            ? 'Fixed publisher headers only — people connect their own account under Settings → Connections.'
            : 'The org account every agent speaks with. Sealed at rest; spoken only by the gateway.'}
        </div>
      </div>
    </div>

    {#if s.probe?.state === 'auth-required'}
      <div class="mt-3 rounded-md border border-warning/40 bg-warning/5 px-3 py-2 font-sans text-xs text-fg">{s.probe.detail}</div>
    {/if}

    {#if fields.length}
      <div class="mt-3 space-y-3">
        {#each fields as f (f.key)}
          {@const stored = storedKeys.includes(f.header)}
          <div>
            <label for={`cred-${f.key}`} class="mb-1 flex items-baseline gap-1.5 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">
              {f.label}
              {#if f.isRequired && !stored}<span class="text-accent">*</span>{/if}
              {#if f.header !== f.label}<span class="normal-case text-ink-dim/70">→ {f.header}</span>{/if}
              {#if stored}<span class="normal-case text-success/80">set</span>{/if}
            </label>
            {#if f.choices?.length}
              <Select id={`cred-${f.key}`} size="sm" value={values[f.key] ?? ''} onchange={(e) => (values = { ...values, [f.key]: e.currentTarget.value })} class="w-full">
                <option value="">{stored ? 'Keep current' : 'Pick one'}</option>
                {#each f.choices as c (c)}<option value={c}>{c}</option>{/each}
              </Select>
            {:else}
              <Input
                id={`cred-${f.key}`}
                type={f.isSecret ? 'password' : 'text'}
                value={values[f.key] ?? ''}
                oninput={(e) => (values = { ...values, [f.key]: e.currentTarget.value })}
                placeholder={stored ? '••• (leave blank to keep)' : (f.placeholder ?? (f.isSecret ? '•••' : ''))}
                autocomplete="off"
              />
            {/if}
            {#if f.description}<div class="mt-1 font-sans text-[11px] text-muted/90">{f.description}</div>{/if}
          </div>
        {/each}
      </div>
    {/if}

    {#if extraStored.length || !fields.length}
      <div class="mt-3">
        <div class="mb-1 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">
          {fields.length ? 'Other headers' : 'Auth headers'}
        </div>
        {#if extraStored.length}
          <div class="mb-2 flex flex-wrap gap-1.5">
            {#each extraStored as k (k)}
              <span class="inline-flex items-center gap-1 rounded border border-line px-1.5 py-0.5 font-mono text-[11px] text-muted" class:line-through={removed.includes(k)}>
                {k}: •••
                {#if !removed.includes(k)}
                  <button type="button" title="Remove this header" class="text-muted hover:text-danger" onclick={() => (removed = [...removed, k])}><X size={11} /></button>
                {:else}
                  <button type="button" class="text-[10px] text-accent" onclick={() => (removed = removed.filter((x) => x !== k))}>keep</button>
                {/if}
              </span>
            {/each}
          </div>
        {/if}
        <McpHeaderRows bind:rows />
        {#if !fields.length}
          <div class="mt-1.5 font-sans text-[11px] text-muted/80">
            Most servers want <code class="font-mono">Authorization: Bearer &lt;key&gt;</code>; some name their own header (<code class="font-mono">X-API-Key</code>). The server's docs say which.
          </div>
        {/if}
      </div>
    {/if}

    {#if error}<div transition:slide={{ duration: 150 }} class="mt-3 text-xs text-danger">{error}</div>{/if}
    {#if discovery}
      <div transition:slide={{ duration: 150 }} class="mt-3 rounded-md border border-danger/40 bg-danger/5 px-3 py-2 font-sans text-xs text-fg">
        Saved, but the server still refused: {discovery}
      </div>
    {/if}

    <div class="mt-4 flex justify-end gap-2 border-t border-line pt-3">
      <Button size="sm" variant="ghost" onclick={onClose}>{discovery ? 'Close' : 'Cancel'}</Button>
      <Button size="sm" disabled={busy || !dirty} onclick={() => void save()}>{busy ? 'Saving' : 'Save and test'}</Button>
    </div>
  </div>
</Modal>
