<script lang="ts">
  import PageSurface from '@/components/app/PageSurface.svelte'
  import { tabFromPath } from '@/lib/route-tabs'
  import { createRawSnippet, type Snippet } from 'svelte'
  import { searchParams } from 'sv-router'
  import { useQueryClient } from '@tanstack/svelte-query'
  import { navigate, route } from '@/router'
  import Tabs from '@/components/ui/Tabs.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import LibraryPane from '@/components/ui/LibraryPane.svelte'
  import { listQuery } from '@/components/ui/query-state'
  import { confirmDelete } from '@/components/ui/confirm.svelte'
  import ContextMenu from '@/components/ui/ContextMenu.svelte'
  import { useContextMenu, copyAppLink } from '@/components/ui/context-menu.svelte'
  import { fly, staggerIn } from '@/lib/motion'
  import { useSession } from '@/lib/session'
  import { claimViewTitle } from '@/lib/view-title.svelte'
  import { createTemplate, deleteTemplate, useTemplates, type Template, type TemplateKind } from '@/lib/templates'
  import TemplateDetail from './templates/TemplateDetail.svelte'

  // The view's title and its InfoTip live in the top strip (lib/view-title) —
  // the body opens straight onto its tabs.
  claimViewTitle('Templates', {
    info: 'The markdown skeletons work starts from. Resolution order everywhere: explicit pick → agent binding → board default → freeform.',
  })

  // Templates — the skeletons work starts from, managed in one place. One tab
  // per surface that consumes them (tickets, plans); the body edits in the
  // full workspace editor (rich + Muse prompt-editing + version history).
  const TEMPLATE_TABS: { id: TemplateKind; label: string; blurb: string }[] = [
    { id: 'ticket', label: 'Tickets', blurb: 'Seeds every ticket description: board defaults, agent bindings, and the QA judge scores against its sections.' },
    { id: 'plan', label: 'Plans', blurb: 'Seeds a new plan’s living document and shapes how the agent rewrites it as the conversation grows.' },
  ]

  const sessionQuery = useSession()
  const session = $derived(sessionQuery.data)
  const qc = useQueryClient()
  const templatesQuery = useTemplates()
  // One read backs BOTH the list and the per-tab counters, and a bare `?? []`
  // would turn its failure into a confident "Tickets 0 / Plans 0" — a count is
  // an assertion, so it must not be printed from a read that failed. `failed`
  // is what keeps the counters honest; LibraryPane renders `notice` itself.
  const read = listQuery(templatesQuery, { title: 'Could not load templates', variant: 'compact' })
  const templates = $derived(read.rows)
  const isLoading = $derived(read.pending)
  const failed = $derived(read.failed)
  // /templates/plan&t=<id> deep-links a tab + template.
  // THE URL IS THE TAB — /templates and /templates/plan. The SELECTED template
  // stays `?t=`: it is a different axis (which item is open, inside whichever
  // kind you are looking at), and collapsing both into the path would make
  // "the plan tab with nothing selected" unexpressible.
  const tab = $derived(tabFromPath(route.pathname, '/templates', TEMPLATE_TABS.map((v) => v.id), 'ticket'))
  const selectedId = $derived.by((): string | null => {
    const t = searchParams.get('t')
    return t == null || t === '' ? null : String(t)
  })
  const setTab = (t: TemplateKind) => {
    // Changing kind clears the selection, as it did before: a ticket template
    // id means nothing on the plan tab.
    if (t === 'ticket') void navigate('/templates')
    else void navigate('/templates/:tab', { params: { tab: t } })
  }
  const select = (id: string | null) => {
    const search: Record<string, string> = id ? { t: id } : {}
    if (tab === 'ticket') void navigate('/templates', { search })
    else void navigate('/templates/:tab', { params: { tab }, search })
  }
  const menu = useContextMenu()

  const list = $derived(templates.filter((t) => t.kind === tab))
  const selected = $derived(templates.find((t) => t.id === selectedId && t.kind === tab) ?? null)
  const meta = $derived(TEMPLATE_TABS.find((t) => t.id === tab)!)

  const refresh = () => qc.invalidateQueries({ queryKey: ['templates'] })
  // The pane hands over a trimmed, non-empty name; making the record out of it
  // is this view's business, and landing on it afterwards is the point.
  const create = async (name: string) => {
    const { template } = await createTemplate({ name, kind: tab })
    await refresh()
    if (template) select(template.id)
  }
  const remove = async (t: Template) => {
    if (
      !(await confirmDelete({
        what: 'template',
        name: t.name,
        detail: `Deleting “${t.name}” (${t.kind}) cannot be undone. Boards and agents bound to it fall back down the template chain.`,
      }))
    )
      return
    await deleteTemplate(t.id)
    if (selectedId === t.id) select(null)
    await refresh()
  }

  // Tab labels carry the per-kind count — bridged into the zero-arg snippet
  // Tabs.svelte expects (the React version passed a <span> inline).
  // An em dash where a number would be: unknown, not zero. A
  // count is an assertion — never print one from a read that
  // hasn't landed or has failed.
  const tabLabel = (label: string, count: string): Snippet =>
    createRawSnippet(() => ({
      render: () => `<span>${label}<span class="ml-1.5 text-ink-dim">${count}</span></span>`,
    }))
  const tabItems = $derived(
    TEMPLATE_TABS.map((t) => ({
      id: t.id,
      label: tabLabel(t.label, isLoading || failed ? '…' : String(templates.filter((x) => x.kind === t.id).length)),
    })),
  )
</script>

{#if session && session.role !== 'admin'}
  <EmptyState icon="▣" title="Admins only" />
{:else}
  <PageSurface>
    <!-- Page content entrance: tab strip → list+detail pane rise in sequence
         (ANIMATIONS.md). The keyed pane keeps its own fly on kind switch and
         stays unstaggered inside — one level only. -->
    <div use:staggerIn class="space-y-6">
      <Tabs items={tabItems} value={tab} onChange={setTab} />

      <!-- Tab-pane grammar: the whole library pane rises in on a kind switch
           (no exit). Safe to key: the new-template input's state lives above;
           the detail is already keyed by selection. -->
      {#key tab}
      <div in:fly={{ y: 6, duration: 200 }} class="h-[calc(100vh-19rem)] min-h-[26rem]">
        <LibraryPane
          groups={[{ items: list }]}
          idOf={(t: Template) => t.id}
          labelOf={(t: Template) => t.name}
          selectedId={selected?.id ?? null}
          onSelect={(t: Template) => select(t.id)}
          pending={isLoading}
          notice={read.notice}
          onRowMenu={(e: MouseEvent, t: Template) =>
            menu.openMenu(e, [
              { label: 'Open', onSelect: () => select(t.id) },
              { label: 'Copy link', onSelect: () => copyAppLink(`/templates/${t.kind}?t=${t.id}`) },
              'sep',
              { label: 'Delete', danger: true, onSelect: () => void remove(t) },
            ])}
          class="h-full"
          onCreate={create}
          createLabel={`New ${tab} template`}
        >
          {#snippet empty()}
            <!-- "Create the first one" is the same zero-claim in prose. -->
            <EmptyState
              icon="▣"
              title={`No ${tab} template selected`}
              hint={failed
                ? 'The template list could not be loaded. Retry on the left.'
                : list.length
                  ? 'Pick one on the left, or create a new one.'
                  : 'Create the first one on the left.'}
            />
          {/snippet}

          {#snippet detail()}
            {#if selected}
              {#key selected.id}
                <!-- No padding here: the record surface owns its own inset
                     (inside its scroll) so its pinned menu runs flush to the
                     pane's edges. -->
                <div class="min-h-0 flex-1 overflow-hidden">
                  <TemplateDetail template={selected} blurb={meta.blurb} onChanged={refresh} onDelete={() => selected && void remove(selected)} />
                </div>
              {/key}
            {/if}
          {/snippet}
        </LibraryPane>
      </div>
      {/key}
    </div>
    <ContextMenu {menu} />
  </PageSurface>
{/if}
