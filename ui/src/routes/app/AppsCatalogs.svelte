<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import SettingsRowMenu from '@/components/settings/SettingsRowMenu.svelte'
  import SettingsSection from '@/components/settings/SettingsSection.svelte'
  import Button from '@/components/ui/Button.svelte'
  import type { DotStatus } from '@/components/ui/chip'
  import Input from '@/components/ui/Input.svelte'
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
        : `Added ${r.catalog.name}. It lists ${r.apps ?? 0} app${r.apps === 1 ? '' : 's'}.`
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
        ? `Discover stops listing its apps, and it is the last source: Discover will be empty until you add one. Installed apps stay installed, and install from Git keeps working.`
        : `Discover stops listing its apps. Installed apps stay installed.`,
      confirmLabel: 'Remove source',
      danger: true,
    })
    if (ok) void run(() => removeCatalog(c.id))
  }

  /** What reading the catalog gave, in a few words. The reason a read failed
   *  is the row's second line. */
  const statusOf = (c: AppCatalogStatus): { dot: DotStatus; text: string } => {
    if (!c.enabled) return { dot: 'idle', text: 'Off' }
    if (c.error) return { dot: 'warn', text: 'Unreachable' }
    const n = c.apps ?? 0
    return { dot: 'ok', text: `${n} app${n === 1 ? '' : 's'}` }
  }
</script>

<SettingsSection
  title="Catalog sources"
  info="The lists Discover shows apps from. Each is an https address serving JSON: an apps list of slug, name, repo and description. When two sources list the same app, Discover shows it from the one higher in this list. Removing a source never uninstalls anything."
>
  <SettingsGroup data-app-catalogs>
    {#if catalogs.length === 0}
      <SettingsRow label="No catalog source yet" hint={isAdmin ? 'Discover lists nothing until you add one.' : 'Discover lists nothing until an admin adds one.'} />
    {/if}
    {#each catalogs as c, i (c.id)}
      {@const standing = statusOf(c)}
      <div data-catalog-row>
        {#if editing?.id === c.id}
          <div class="flex min-h-[60px] flex-wrap items-center gap-3 px-5 py-3">
            <Input size="sm" bind:value={editing.name} placeholder="Name" class="w-48" />
            <Input size="sm" bind:value={editing.url} placeholder="https://example.com/index.json" class="min-w-0 flex-1 basis-64 font-mono" />
            <Button size="md" variant="outline" disabled={busy || !editing.name.trim() || !editing.url.trim()} onclick={() => void save()}>Save</Button>
            <Button size="md" variant="ghost" disabled={busy} onclick={() => (editing = null)}>Cancel</Button>
          </div>
        {:else}
          <SettingsRow label={c.name}>
            {#snippet hint()}
              <div class="break-all font-mono text-xs leading-5">{c.url}</div>
              {#if c.enabled && c.error}<div class="wrap-anywhere text-warning">{c.error}</div>{/if}
              {#if c.shadowed.length > 0}
                <div>Also lists {c.shadowed.join(', ')}, shown from a source higher in the list.</div>
              {/if}
            {/snippet}
            {#snippet status()}<StatusDot status={standing.dot} />{standing.text}{/snippet}
            {#if isAdmin}
              <Toggle
                checked={c.enabled}
                disabled={busy}
                title={c.enabled ? `Stop reading ${c.name}` : `Read ${c.name}`}
                onChange={(on) => void run(() => editCatalog(c.id, { enabled: on }))}
              />
              <SettingsRowMenu
                label={`Actions for ${c.name}`}
                items={[
                  { label: 'Move up', disabled: busy || i === 0, onSelect: () => move(i, -1) },
                  { label: 'Move down', disabled: busy || i === catalogs.length - 1, onSelect: () => move(i, 1) },
                  { label: 'Edit', disabled: busy, onSelect: () => (editing = { id: c.id, name: c.name, url: c.url }) },
                  'sep',
                  { label: 'Remove source', danger: true, disabled: busy, onSelect: () => void remove(c) },
                ]}
              />
            {/if}
          </SettingsRow>
        {/if}
      </div>
    {/each}

    {#if isAdmin}
      <SettingsRow label="Add a source" hint="An address that serves a JSON list of apps." apart>
        <Input size="sm" bind:value={name} placeholder="Name" class="w-40" />
        <Input
          size="sm"
          bind:value={url}
          placeholder="https://example.com/index.json"
          onkeydown={(e) => e.key === 'Enter' && void add()}
          class="w-[280px] font-mono"
        />
        <Button size="md" variant="outline" disabled={busy || !name.trim() || !url.trim()} onclick={() => void add()}>Add source</Button>
      </SettingsRow>
    {/if}
  </SettingsGroup>
  {#if notice}<div class="mt-2 font-sans text-[13px] leading-5 text-fg">{notice}</div>{/if}
  {#if error}<div class="mt-2 font-sans text-[13px] leading-5 text-danger">{error}</div>{/if}
</SettingsSection>
