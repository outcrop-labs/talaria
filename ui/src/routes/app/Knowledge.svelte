<script lang="ts">
  import { route } from '@/router'
  import { searchParams } from 'sv-router'
  import { useQueryClient } from '@tanstack/svelte-query'
  import { Plus } from '@lucide/svelte'
  import ContextMenu from '@/components/ui/ContextMenu.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import IconButton from '@/components/ui/IconButton.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Materialize from '@/components/ui/Materialize.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import { confirm } from '@/components/ui/confirm.svelte'
  import { copyAppLink, useContextMenu, type ContextMenuEntry } from '@/components/ui/context-menu.svelte'
  import { navigate } from '@/router'
  import { fade, slide } from '@/lib/motion'
  import { isUnder } from '@/lib/route-tabs'
  import {
    readKnowledgeSelection,
    restorableKnowledgeSelection,
    writeKnowledgeSelection,
  } from '@/lib/knowledge-selection'
  import {
    createDoc, createSpace, deleteDoc, deleteSpace, moveDoc, updateSpace, useDoc, useDocs, useSpaces,
    type KbDocMeta, type KbSpace,
  } from '@/lib/kb'
  import KbDocEditor from './KbDocEditor.svelte'
  import KbDocTree from './KbDocTree.svelte'
  import KbSearch from './KbSearch.svelte'
  import KbSpaceEditor from './KbSpaceEditor.svelte'
  import KbSpaceRow from './KbSpaceRow.svelte'

  // The knowledgebase — an Outline-style markdown drive. A searchable, nestable
  // tree of docs on the left; a WYSIWYG doc with breadcrumb, emoji, table of
  // contents, and backlinks in the middle. Official docs feed the org brain;
  // agent-kind docs start from an OKF scaffold.
  const qc = useQueryClient()
  // "No spaces yet." is a claim that the knowledgebase is EMPTY. A read that
  // failed says nothing about what is in there, so the rejection has to reach
  // the render instead of being swallowed by a `= []` default.
  const spacesQuery = useSpaces()
  const spaces = $derived(spacesQuery.data ?? [])
  // ?space=<id>&doc=<id> deep-link the tree selection — the URL IS the state.
  //
  // `d` is accepted as an alias for `doc` because SIX link builders spell it
  // that way — this file's own "Copy link" on a doc (twice), the cross-reference
  // picker, the comment notification, and the OKF resource line — and a search
  // key this function does not name is DROPPED. Every one of those links opened
  // the knowledgebase root instead of the document, silently: the same class of
  // bug as a swallowed error, one layer up. Normalizing here fixes all six and
  // any seventh, which chasing the call sites would not. `s` is taken for the
  // same reason before someone writes the matching short form for the space.
  // sv-router auto-parses query values (numbers, bare flags) — ids must stay
  // strings, so anything non-string is stringified and bare flags dropped.
  const idParam = (k: string) => {
    const v = searchParams.get(k)
    return v == null || v === true ? null : String(v)
  }
  // THE PATH IS THE SELECTION: /knowledge/<space>/<doc>. The query forms
  // (`?space=` / `?doc=`, and the short `?s=` / `?d=`) remain the BY-ID
  // PERMALINK, because a link can know a document without knowing which space
  // holds it — an attachment chip has only the doc id — and resolving that is
  // this view's job, not the linker's.
  const pathSpace = $derived(route.pathname.startsWith('/knowledge/') ? route.pathname.split('/')[2] ?? null : null)
  const pathDoc = $derived(route.pathname.startsWith('/knowledge/') ? route.pathname.split('/')[3] ?? null : null)
  const spaceId = $derived(pathSpace || idParam('space') || idParam('s') || null)
  const docId = $derived(pathDoc || idParam('doc') || idParam('d') || null)
  // One navigation per selection change — space + doc move together.
  // `replace` for housekeeping — restoring where you were is not a place you
  // navigated to, and pushing it would make Back walk your own bookkeeping.
  const setLoc = (space: string | null, doc: string | null, opts: { replace?: boolean } = {}) => {
    const { replace } = opts
    if (space && doc) void navigate('/knowledge/:space/:doc', { params: { space, doc }, replace })
    else if (space) void navigate('/knowledge/:space', { params: { space }, replace })
    else void navigate('/knowledge', { replace })
  }
  const setSpaceId = (id: string | null) => setLoc(id, null)
  const setDocId = (id: string | null) => setLoc(spaceId, id)
  let creatingSpace = $state(false)
  const activeSpace = $derived(spaces.find((s) => s.id === spaceId) ?? spaces[0])
  const docsQuery = useDocs(() => activeSpace?.id ?? null)
  const docs = $derived(docsQuery.data ?? [])

  // Landing without a space picks the first one. It must NOT drop the doc: a
  // `?doc=…` link that arrives without a space was rewritten to `?space=…`
  // alone, so the document the link pointed at was thrown away between the
  // click and the first paint, and the reader got the folder overview with no
  // sign anything had been asked for.
  // A by-id permalink resolves to the canonical path, and a doc resolves to ITS
  // OWN space rather than to the first one in the list. That guess was a real
  // bug: `?doc=<id>` for a document in the second space rendered the first
  // space's overview and quietly dropped the request.
  const permalinkDoc = useDoc(() => (!pathSpace && docId ? docId : null))
  // Still on Knowledge? This view outlives the click that leaves it, and the
  // effect below navigates — without this it reads the NEXT view's pathname as
  // "Knowledge with no space", canonicalises to the first space, and drags you
  // back. Same bug the nav rail showed on Comms; see `isUnder`.
  const onKnowledge = $derived(isUnder(route.pathname, '/knowledge'))

  // WHERE YOU WERE, so leaving and coming back through the nav rail does not
  // land you on the first space. Only while genuinely here — a leaving view
  // runs its effects once against the next view's URL, and writing there would
  // record an empty selection over a good memory.
  $effect(() => {
    if (onKnowledge && spaceId) writeKnowledgeSelection({ spaceId, docId })
  })

  // RESTORED ONCE, ON ARRIVAL — latched rather than keyed off a bare URL,
  // because bare is reachable from inside the view too (deleting the space you
  // were in navigates to `/knowledge`), and restoring there would undo the
  // thing the user just did. Arriving IS a mount, so the latch is the honest
  // test. An explicit link or permalink outranks the memory and just spends it.
  let restored = false
  $effect(() => {
    if (!onKnowledge || restored) return
    if (pathSpace || docId) {
      restored = true
      return
    }
    // Validating against a list that has not arrived would discard a good
    // memory and fall straight through to the first space.
    if (spacesQuery.isLoading) return
    restored = true
    const saved = restorableKnowledgeSelection(readKnowledgeSelection(), { spaceIds: spaces.map((s) => s.id) })
    if (saved) setLoc(saved.spaceId, saved.docId, { replace: true })
  })

  $effect(() => {
    if (!onKnowledge) return
    if (pathSpace) return // already canonical
    // Let the restore above answer first; otherwise the first-space default
    // wins the race and there is nothing left for the memory to restore.
    if (!restored) return
    const home = permalinkDoc.data?.spaceId ?? (docId ? null : spaces[0]?.id)
    if (!home) return
    if (docId) void navigate('/knowledge/:space/:doc', { params: { space: home, doc: docId }, replace: true })
    else void navigate('/knowledge/:space', { params: { space: home }, replace: true })
  })

  const newSpace = async (name: string) => {
    const { space } = await createSpace(name)
    await qc.invalidateQueries({ queryKey: ['kb-spaces'] })
    if (space) setSpaceId(space.id)
  }
  const newDoc = async (kind: 'human' | 'agent', parentId: string | null = null) => {
    if (!activeSpace) return
    const { doc } = await createDoc(activeSpace.id, { kind, title: 'Untitled', parentId })
    await qc.invalidateQueries({ queryKey: ['kb-docs', activeSpace.id] })
    if (doc) setDocId(doc.id)
  }
  const move = async (id: string, parentId: string | null, sort: number) => {
    await moveDoc(id, parentId, sort)
    await qc.invalidateQueries({ queryKey: ['kb-docs', activeSpace?.id] })
  }
  const renameSpace = async (id: string, name: string) => {
    await updateSpace(id, { name })
    await qc.invalidateQueries({ queryKey: ['kb-spaces'] })
  }
  const removeSpace = async (id: string) => {
    await deleteSpace(id)
    await qc.invalidateQueries({ queryKey: ['kb-spaces'] })
    if (spaceId === id) setLoc(null, null)
  }

  // Jump to a doc from search — switch to its space if needed.
  // Jump to a doc from search — one navigation carries space + doc together.
  const openDoc = (hit: { id: string; spaceId: string; kind?: 'doc' | 'space' }) => {
    setLoc(hit.spaceId, hit.kind === 'space' ? null : hit.id)
  }

  // Right-click menus — shortcuts to actions the sidebar/editors already offer.
  const menu = useContextMenu()
  // Same createDoc flow as newDoc, but scoped to any space (not just the active one).
  const newDocIn = async (sid: string) => {
    const { doc } = await createDoc(sid, { kind: 'human', title: 'Untitled' })
    await qc.invalidateQueries({ queryKey: ['kb-docs', sid] })
    setLoc(sid, doc ? doc.id : null)
  }
  const spaceMenu = (s: KbSpace): ContextMenuEntry[] => [
    { label: 'Open', onSelect: () => setLoc(s.id, null) },
    { label: 'Copy link', onSelect: () => copyAppLink(`/knowledge?space=${s.id}`) },
    { label: 'New doc', onSelect: () => void newDocIn(s.id) },
    'sep',
    {
      label: 'Delete space',
      danger: true,
      onSelect: async () => {
        // Mirrors the space editor's delete confirm.
        if (await confirm({ title: 'Delete folder', message: `Delete "${s.name}" and all its docs?`, confirmLabel: 'Delete', danger: true })) await removeSpace(s.id)
      },
    },
  ]
  const docMenu = (d: KbDocMeta): ContextMenuEntry[] => [
    { label: 'Open', onSelect: () => setLoc(d.spaceId, d.id) },
    { label: 'Copy link', onSelect: () => copyAppLink(`/knowledge?space=${d.spaceId}&doc=${d.id}`) },
    'sep',
    {
      label: 'Delete document',
      danger: true,
      onSelect: async () => {
        // Same confirm + deleteDoc flow as the doc editor's kebab menu.
        if (!(await confirm({ title: 'Delete document', message: `Delete "${d.title}"?`, confirmLabel: 'Delete', danger: true }))) return
        await deleteDoc(d.id)
        await qc.invalidateQueries({ queryKey: ['kb-docs', d.spaceId] })
        if (docId === d.id) setDocId(null)
      },
    },
  ]
</script>

<div class="flex h-full min-h-0">
  <aside class="flex h-full w-72 shrink-0 flex-col border-r border-line-subtle bg-sidebar font-sans">
    <!-- No "Knowledge" label: naming the view is the top strip's job now. The
         bar stays to carry the one action it always had. -->
    <div class="flex h-12 shrink-0 items-center gap-1.5 border-b border-line-subtle px-4">
      <IconButton class="ml-auto" size="sm" title="New space" onclick={() => (creatingSpace = !creatingSpace)}>
        <Plus size={15} />
      </IconButton>
    </div>
    <div class="border-b border-line-subtle p-3">
      <KbSearch onOpen={openDoc} />
    </div>
    {#if creatingSpace}
      <div transition:slide={{ duration: 150 }} class="border-b border-line-subtle px-3 py-2">
        <Input
          autofocus
          size="sm"
          placeholder="space name"
          onkeydown={(e) => {
            if (e.key === 'Enter') {
              const v = (e.target as HTMLInputElement).value.trim()
              if (v) void newSpace(v)
              creatingSpace = false
            } else if (e.key === 'Escape') creatingSpace = false
          }}
          onblur={() => (creatingSpace = false)}
        />
      </div>
    {/if}
    <div class="min-h-0 flex-1 overflow-y-auto p-2">
      <!-- Skeleton → content as one motion: space-row-shaped skeletons (icon
           lane + name, KbSpaceRow's own frame) materialize into the rows.
           Materialize direct — error and resolved-empty stay in the content
           branch, keyed off the query exactly as before. -->
      <Materialize loading={spacesQuery.isLoading} count={6}>
        {#snippet skeleton(i)}
          <div aria-hidden="true" class="mb-2 rounded-md px-2 py-1.5">
            <div class="flex h-5 items-center gap-2">
              <Skeleton class="h-4 w-4 shrink-0 rounded" />
              <Skeleton class={`h-3 rounded-full ${['w-28', 'w-20', 'w-36', 'w-24'][i % 4]}`} />
            </div>
          </div>
        {/snippet}
        {#if spacesQuery.isError && spacesQuery.data === undefined}
        <!-- Unreachable is not deleted. Without this branch an outage reads
             as somebody having wiped the whole knowledgebase. -->
        <div in:fade={{ duration: 150 }}>
          <QueryError
            variant="compact"
            error={spacesQuery.error}
            title="Could not load your spaces"
            onRetry={() => void spacesQuery.refetch()}
          />
        </div>
      {:else if spaces.length === 0}
        <!-- A 200 with no spaces is still a real answer — keep saying so. -->
        <div in:fade={{ duration: 150 }}>
          <EmptyState variant="inline" title="No spaces yet." class="px-2 py-6 text-center" />
        </div>
      {:else}
        {#each spaces as s (s.id)}
          <div class="mb-2">
            <KbSpaceRow
              space={s}
              active={activeSpace?.id === s.id}
              onSelect={() => {
                setLoc(s.id, null) // open the space's own overview
              }}
              onRename={(name) => void renameSpace(s.id, name)}
              onContextMenu={(e) => menu.openMenu(e, spaceMenu(s))}
            />
            {#if activeSpace?.id === s.id}
              {#if docsQuery.isLoading}
                <!-- Switching spaces refetches the doc tree — keep its shape. -->
                <SkeletonRows rows={6} class="ml-4 mt-1 border-l border-line-subtle py-1 pl-4" />
              {:else if docsQuery.isError && docsQuery.data === undefined}
                <!-- Same lie one level down: a space whose doc list 500s
                     would otherwise render as a space with nothing in it. -->
                <QueryError
                  variant="inline"
                  class="ml-4 mt-1 border-l border-line-subtle py-2 pl-4"
                  error={docsQuery.error}
                  title="Could not load this space"
                  onRetry={() => void docsQuery.refetch()}
                />
              {:else}
                <KbDocTree {docs} activeId={docId} onSelect={setDocId} onNew={newDoc} onMove={move} onDocMenu={(e, d) => menu.openMenu(e, docMenu(d))} />
              {/if}
            {/if}
          </div>
        {/each}
      {/if}
      </Materialize>
    </div>
  </aside>

  <main class="min-h-0 min-w-0 flex-1">
    {#if docId}
      {#key docId}
        <KbDocEditor {docId} {docs} onDeleted={() => setDocId(null)} onSelect={setDocId} folderName={activeSpace?.name} />
      {/key}
    {:else if activeSpace}
      <!-- A top-level folder is itself a document: its editable overview. -->
      {#key activeSpace.id}
        <KbSpaceEditor spaceId={activeSpace.id} onNewDoc={() => void newDoc('human')} onDeleted={() => void removeSpace(activeSpace.id)} />
      {/key}
    {:else}
      <EmptyState icon="❖" title="Knowledge" hint="Create a space to start writing." />
    {/if}
  </main>
  <ContextMenu {menu} />
</div>
