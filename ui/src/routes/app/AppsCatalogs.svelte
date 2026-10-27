<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import { ArrowDown, ArrowUp, Pencil, Trash2 } from '@lucide/svelte'
  import Button from '@/components/ui/Button.svelte'
  import IconButton from '@/components/ui/IconButton.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Panel from '@/components/ui/Panel.svelte'
  import SectionHeader from '@/components/ui/SectionHeader.svelte'
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import Toggle from '@/components/ui/Toggle.svelte'
  import { confirm } from '@/components/ui/confirm.svelte'
  import { errorMessage } from '@/lib/fetch-json'
  import { addCatalog, editCatalog, removeCatalog, reorderCatalogs, type AppCatalogStatus } from './apps'

  // THE CATALOGS DISCOVER READS — a list the operator fills. Each is one URL
  // serving a JSON index; Discover shows every enabled one's apps, and an app
  // two catalogs list is shown from the one higher in this list. Nothing puts
  // a catalog back once it is removed: an empty list is a choice, and
  // install-from-git works without one. Admins edit; everyone else with the
  // Apps view reads the list.
  let { catalogs, isAdmin }: { catalogs: AppCatalogStatus[]; isAdmin: boolean } = $props()

  const qc = useQueryClient()
  let name = $state('')
  let url = $state('')
  let busy = $state(false)
  let error = $state<string | null>(null)
  let notice = $state<string | null>(null)
  /** The row being edited, and its draft. */
  let editing = $state<{ id: string; name: string; url: string } | null>(null)

  const refresh = () => qc.invalidateQueries({ queryKey: ['admin-apps-catalog'] })

  const run = async (write: () => Promise<unknown>) => {
    busy = true
    error = null
    notice = null
    try {
      await write()
      await refresh()
      return true
    } catch (e) {
      error = errorMessage(e)
      return false
    } finally {
      busy = false
    }
  }

  const add = async () => {
    const n = name.trim()
    const u = url.trim()
    if (!n || !u) return
    let said = ''
    const ok = await run(async () => {
      const r = await addCatalog(n, u)
      said = r.error
        ? `Added ${r.catalog.name}, but it could not be read: ${r.error}.`
        : `Added ${r.catalog.name}: it lists ${r.apps ?? 0} app${r.apps === 1 ? '' : 's'}.`
    })
    if (ok) {
      name = ''
      url = ''
      notice = said
    }
  }

  const save = async () => {
    const e = editing
    if (!e) return
    const ok = await run(() => editCatalog(e.id, { name: e.name.trim(), url: e.url.trim() }))
    if (ok) editing = null
  }

  const move = (index: number, by: -1 | 1) => {
    const order = catalogs.map((c) => c.id)
    const [id] = order.splice(index, 1)
    if (!id) return
    order.splice(index + by, 0, id)
    void run(() => reorderCatalogs(order))
  }

  const remove = async (c: AppCatalogStatus) => {
    const last = catalogs.length === 1
    const ok = await confirm({
      title: `Remove ${c.name}?`,
      message: last
        ? `Discover stops listing its apps, and it is the last catalog: Discover will be empty until you add one. Installed apps stay installed, and install from Git keeps working.`
        : `Discover stops listing its apps. Installed apps stay installed.`,
      confirmLabel: 'Remove catalog',
      danger: true,
    })
    if (ok) void run(() => removeCatalog(c.id))
  }

  /** What reading the catalog gave, in a few words. */
  const statusOf = (c: AppCatalogStatus): { dot: 'ok' | 'warn' | 'idle'; text: string } => {
    if (!c.enabled) return { dot: 'idle', text: 'off' }
    if (c.error) return { dot: 'warn', text: c.error }
    const n = c.apps ?? 0
    return { dot: 'ok', text: `${n} app${n === 1 ? '' : 's'}` }
  }
</script>

<Panel data-app-catalogs>
  <SectionHeader
    title="Catalogs"
    info="The indexes Discover lists apps from. Each is an https URL serving JSON: an apps list of slug, name, repo and description. When two catalogs list the same app, Discover shows it from the one higher in this list. Removing a catalog never uninstalls anything."
    action={`${catalogs.length} listed`}
  />
  <div class="space-y-2">
    {#if catalogs.length === 0}
      <div class="rounded-md border border-dashed border-line px-3 py-2.5 font-sans text-xs text-muted">
        No catalog is configured, so Discover lists nothing.{isAdmin ? ' Add one below, or install any app from its git URL.' : ''}
      </div>
    {/if}
    {#each catalogs as c, i (c.id)}
      {@const status = statusOf(c)}
      <div class="space-y-1.5 rounded-md border border-line p-2.5" data-catalog-row>
        {#if editing?.id === c.id}
          <div class="flex flex-wrap gap-2">
            <Input size="sm" bind:value={editing.name} placeholder="Name" class="min-w-0 flex-1 basis-40" />
            <Input size="sm" bind:value={editing.url} placeholder="https://…/index.json" class="min-w-0 flex-[2] basis-64 font-[var(--font-mono)]" />
          </div>
          <div class="flex gap-2">
            <Button size="sm" disabled={busy || !editing.name.trim() || !editing.url.trim()} onclick={() => void save()}>Save</Button>
            <Button size="sm" variant="ghost" disabled={busy} onclick={() => (editing = null)}>Cancel</Button>
          </div>
        {:else}
          <div class="flex flex-wrap items-center gap-x-2 gap-y-1">
            <span class="min-w-0 font-sans text-sm text-fg">{c.name}</span>
            <span class="flex min-w-0 items-center gap-1.5 font-mono text-[10px] uppercase tracking-[0.05em] text-muted" title={status.text}>
              <StatusDot status={status.dot} />
              <span class={status.dot === 'warn' ? 'normal-case tracking-normal text-warning' : ''}>{status.text}</span>
            </span>
            {#if isAdmin}
              <span class="ml-auto flex shrink-0 items-center gap-1">
                <Toggle checked={c.enabled} disabled={busy} title={c.enabled ? 'Turn off: Discover stops reading it' : 'Turn on'} onChange={(on) => void run(() => editCatalog(c.id, { enabled: on }))} />
                <IconButton size="sm" title="Move up" disabled={busy || i === 0} onclick={() => move(i, -1)}><ArrowUp size={13} /></IconButton>
                <IconButton size="sm" title="Move down" disabled={busy || i === catalogs.length - 1} onclick={() => move(i, 1)}><ArrowDown size={13} /></IconButton>
                <IconButton size="sm" title="Edit" disabled={busy} onclick={() => (editing = { id: c.id, name: c.name, url: c.url })}><Pencil size={13} /></IconButton>
                <IconButton size="sm" title="Remove" danger disabled={busy} onclick={() => void remove(c)}><Trash2 size={13} /></IconButton>
              </span>
            {/if}
          </div>
          <div class="break-all font-mono text-[11px] text-muted">{c.url}</div>
          {#if c.shadowed.length > 0}
            <div class="font-sans text-[11px] text-muted">
              Also lists {c.shadowed.join(', ')}, shown from a catalog higher in the list.
            </div>
          {/if}
        {/if}
      </div>
    {/each}

    {#if isAdmin}
      <div class="flex flex-wrap gap-2 pt-1">
        <Input size="sm" bind:value={name} placeholder="Name" class="min-w-0 flex-1 basis-40" />
        <Input
          size="sm"
          bind:value={url}
          placeholder="https://…/index.json"
          onkeydown={(e) => e.key === 'Enter' && void add()}
          class="min-w-0 flex-[2] basis-64 font-[var(--font-mono)]"
        />
        <Button size="sm" disabled={busy || !name.trim() || !url.trim()} onclick={() => void add()}>Add catalog</Button>
      </div>
    {/if}
    {#if notice}<div class="font-sans text-xs text-fg">{notice}</div>{/if}
    {#if error}<div class="font-sans text-xs text-danger">{error}</div>{/if}
  </div>
</Panel>
