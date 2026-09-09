<script lang="ts">
  import { isUnder, tabFromPath } from '@/lib/route-tabs'
  import {
    readArtifactsSelection,
    restorableArtifactsSelection,
    writeArtifactsSelection,
  } from '@/lib/artifacts-selection'
  import { searchParams } from 'sv-router'
  import { navigate, route } from '@/router'
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import { ChevronRight, FolderPlus, HardDrive, Plus, Search, Upload, X } from '@lucide/svelte'
  import Rail from '@/components/app/Rail.svelte'
  import RailRow from '@/components/app/RailRow.svelte'
  import RailSection from '@/components/app/RailSection.svelte'
  import RailSurface from '@/components/app/RailSurface.svelte'
  import Stage from '@/components/app/Stage.svelte'
  import StageHeader from '@/components/app/StageHeader.svelte'
  import Button from '@/components/ui/Button.svelte'
  import DropdownMenu from '@/components/ui/DropdownMenu.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Segmented from '@/components/ui/Segmented.svelte'
  import PermissionsModal from '@/components/kb/PermissionsModal.svelte'
  import { useContextMenu, type ContextMenuEntry } from '@/components/ui/context-menu.svelte'
import ContextMenu from '@/components/ui/ContextMenu.svelte'
  import { cn } from '@/lib/cn'
  import { errorMessage } from '@/lib/fetch-json'
  import { pushToast } from '@/lib/toast.svelte'
  import { useSession } from '@/lib/session'
  import { useUsers } from '@/lib/users'
  import {
    createArtifact, createFolder, duplicateArtifact, duplicateFolder,
    saveArtifact, updateFolder, uploadFile, useArtifacts, useFolders, type ArtifactKind,
  } from '@/lib/artifacts'
  import { clearClipboard, clipboard } from './files-clipboard.svelte'
  import {
    browseDrivePage, importDriveFile, useDriveRoster, type DriveBrowse, type DriveRosterEntry,
  } from '@/lib/google-drive'
  import { driveRow } from './artifacts'
  import type { PermKind } from '@/lib/kb'
  import ArtifactEditor from './ArtifactEditor.svelte'
  import ArtifactsBrowser from './ArtifactsBrowser.svelte'
  import SecretsVault from './SecretsVault.svelte'
  import ArtifactsProperties from './ArtifactsProperties.svelte'
  import {
    AGENTS_ROOT,
    ancestry,
    DRAG_MIME,
    folderRow,
    placeOf,
    NEW_KINDS,
    PLACES,
    sortRows,
    toRow,
    type Drag,
    type Place,
    type Row,
    type SortDir,
    type SortKey,
  } from './artifacts'

  // Files. A browser, not a tree: the rail lists PLACES and the stage is the
  // folder you're standing in, which is the shape every person arriving from
  // Drive or Dropbox already knows. (The code side stays "artifact" — see
  // artifacts.ts.)
  const qc = useQueryClient()
  const session = useSession()
  const me = $derived(session.data ?? null)
  const usersQuery = useUsers()
  const users = $derived(usersQuery.data ?? [])

  // Two reads build one browser, so both rejections have to survive to the
  // render: "Nothing here yet" is a claim about the owner's WORK, and a store
  // that is merely unreachable must never be reported as a store that is empty.
  const artifactsQuery = useArtifacts()
  const foldersQuery = useFolders()
  const artifacts = $derived(artifactsQuery.data ?? [])
  const folders = $derived(foldersQuery.data ?? [])
  const failure = $derived(
    artifactsQuery.isError && artifactsQuery.data === undefined
      ? { title: 'Could not load your files', error: artifactsQuery.error, retry: () => void artifactsQuery.refetch() }
      : foldersQuery.isError && foldersQuery.data === undefined
        ? { title: 'Could not load your folders', error: foldersQuery.error, retry: () => void foldersQuery.refetch() }
        : null,
  )

  // The URL IS the selection: /artifacts/<place>, then ?f= folder and ?a= the
  // open file. Place is a PLACE — the same kind of thing as a tab — so it is a
  // path segment; the folder and the open file are selection within it.
  const place = $derived(tabFromPath(route.pathname, '/artifacts', PLACES.map((p) => p.id), 'my'))
  const rawFolder = $derived(searchParams.get('f'))
  const folderId = $derived(rawFolder ? String(rawFolder) : null)
  const rawActive = $derived(searchParams.get('a'))
  const activeId = $derived(rawActive ? String(rawActive) : null)
  // The Drive place's own selection: which drive (roster key) and which of
  // ITS folders. Same axes, different alphabet — Drive ids are Google's, so
  // nothing validates them against the local folder roster.
  const driveKeyParam = $derived(place === 'drive' ? searchParams.get('d') : null)
  const driveKey = $derived(driveKeyParam ? String(driveKeyParam) : null)

  const setActiveId = (id: string | null) => {
    if (id) searchParams.set('a', id)
    else searchParams.delete('a')
  }
  const goPlace = (p: Place) => {
    // Leaving for another place abandons the folder AND the open file — the
    // stage should show the place you just asked for, not the last thing open.
    // Navigating with no `search` is what drops them.
    if (p === 'my') void navigate('/artifacts')
    else void navigate('/artifacts/:place', { params: { place: p } })
  }
  /** Switch Drive (roster key) — the folder resets with the drive. */
  const goDrive = (key: string) => {
    const search: Record<string, string> = { d: key }
    void navigate('/artifacts/:place', { params: { place: 'drive' }, search })
  }
  const goFolder = (id: string | null) => {
    searchParams.delete('a')
    if (id) searchParams.set('f', id)
    else searchParams.delete('f')
  }

  // Still here? This view outlives the click that leaves it, so both effects
  // below have to stop answering questions about a page the user has left —
  // see `isUnder`, and the nav-rail bug it is named for.
  const onArtifacts = $derived(isUnder(route.pathname, '/artifacts'))

  // WHERE YOU WERE, so leaving and coming back through the nav rail does not
  // land you at the root of "My files". All three parts travel together: the
  // right place with the wrong folder is its own kind of lost.
  $effect(() => {
    if (onArtifacts) writeArtifactsSelection({ place, folderId, activeId, driveId: place === 'drive' ? driveKey : null })
  })

  // RESTORED ONCE, ON ARRIVAL. Latched on mount rather than keyed off a bare
  // URL, and here that distinction is load-bearing rather than theoretical:
  // this view's own "My files" navigates to exactly `/artifacts` with nothing
  // selected, so a URL-shaped test would fire on it and drag the user back into
  // the folder they had just stepped out of. Arriving IS a mount.
  let restored = false
  $effect(() => {
    if (!onArtifacts || restored) return
    // An explicit place, folder or file outranks the memory and spends it.
    if (route.pathname !== '/artifacts' || rawFolder || rawActive) {
      restored = true
      return
    }
    // Each roster is passed only once loaded; validating against a list that
    // has not arrived would drop a good part of the memory for no reason.
    if (foldersQuery.isLoading || artifactsQuery.isLoading) return
    restored = true
    const saved = restorableArtifactsSelection(readArtifactsSelection(), {
      folderIds: foldersQuery.isSuccess ? folders.map((f) => f.id) : null,
      artifactIds: artifactsQuery.isSuccess ? artifacts.map((a) => a.id) : null,
    })
    if (!saved) return
    const search: Record<string, string> = {}
    if (saved.folderId) search.f = saved.folderId
    if (saved.activeId) search.a = saved.activeId
    if (saved.place === 'my') void navigate('/artifacts', { search, replace: true })
    else void navigate('/artifacts/:place', { params: { place: saved.place }, search, replace: true })
  })

  const crumbMenu = useContextMenu()

  /** The row whose Properties dialog is open (null = closed). */
  let propsRow = $state<Row | null>(null)
  /** The artifact whose sharing dialog is open. Sharing reuses the SAME
   *  PermissionsModal the editor and the knowledgebase use — one access model,
   *  one dialog, so a file's sharing can't mean two different things depending
   *  on where you opened it from. */
  let shareRow = $state<Row | null>(null)
  const shareFolder = $derived(shareRow?.type === 'folder' ? (folders.find((f) => f.id === shareRow!.id) ?? null) : null)
  const shareArtifact = $derived(shareRow?.type === 'artifact' ? shareRow.artifact : null)

  /** Mirrors the server's `canGovern`: the owner, or — for an OWNERLESS
   *  workspace item — an admin. The server is slightly more generous (it also
   *  admits anyone allowed to use the agent that created it), which needs a DB
   *  read this side can't do; erring narrow only ever greys the controls out,
   *  never lets through a change the server would refuse. */
  const governs = (rec: { ownerUserId: string | null; createdBy: string | null } | null) =>
    !!rec && !!me && (rec.ownerUserId ? rec.ownerUserId === me.id : rec.createdBy === (me.email ?? me.name) || me.role === 'admin')

  /** One dialog, two kinds. Folders and files share an access model, so they
   *  share the Share dialog too — only the REST path and the labels differ. */
  const shareTarget = $derived.by(() => {
    const rec = shareArtifact ?? shareFolder
    if (!rec || !shareRow) return null
    return {
      kind: (shareArtifact ? 'artifacts' : 'artifact-folders') as PermKind,
      id: rec.id,
      label: shareArtifact ? shareArtifact.title : (shareFolder?.name ?? ''),
      visibility: rec.visibility,
      editPolicy: rec.editPolicy,
      // Only artifacts get a public page of their own; a shared folder set to
      // public makes its CONTENTS reachable, not the folder itself.
      publicSlug: shareArtifact?.publicSlug ?? null,
      canManage: governs(rec),
    }
  })
  let q = $state('')
  let sortKey = $state<SortKey>('name')
  let sortDir = $state<SortDir>('asc')
  let view = $state<'list' | 'grid'>('list')


  const rosterQuery = useDriveRoster()
  const roster = $derived((rosterQuery.data?.drives ?? []) as DriveRosterEntry[])
  const rosterEntry = $derived(roster.find((r) => r.key === driveKey) ?? null)
  const notConnected = $derived(
    place === 'drive' && !rosterQuery.isLoading && rosterQuery.data !== undefined && roster.length === 0,
  )

  // THE DRIVE LISTING — first page through svelte-query (cache, retry), more
  // pages appended by Load-more. Drive rows are Google's facts in the
  // browser's Row shape; the server's order (folders first, then sort) is the
  // honest one with pagination, so the client never re-sorts them.
  const driveQuery = createQuery(() => ({
    queryKey: ['google-drive', 'browse', driveKey ?? '', folderId ?? '', q, sortKey, sortDir],
    enabled: place === 'drive' && !!driveKey,
    queryFn: () => browseDrivePage(driveKey!, folderId, q, `${sortKey}`),
  }))
  let driveMoreRows = $state<ReturnType<typeof driveRow>[]>([])
  let driveMoreToken = $state<string | null>(null)
  // New query = new standing: appended pages reset with the first page.
  $effect(() => {
    void driveQuery.data
    void driveKey
    void folderId
    driveMoreRows = []
    driveMoreToken = driveQuery.data?.nextPageToken ?? null
  })
  const loadMoreDrive = async () => {
    const token = driveMoreToken
    if (!token || !driveKey) return
    try {
      const page = await browseDrivePage(driveKey, folderId, q, `${sortKey}`, token)
      driveMoreRows = [...driveMoreRows, ...page.files.map((e) => driveRow(e, driveKey, rosterEntry?.writable ?? false, rosterEntry?.email ?? rosterEntry?.name ?? ''))]
      driveMoreToken = page.nextPageToken
    } catch (e) {
      pushToast({ title: 'Could not load more', body: errorMessage(e), tone: 'danger' })
    }
  }
  const driveRows = $derived.by(() => {
    const first = driveQuery.data?.files ?? []
    const owner = rosterEntry?.email ?? rosterEntry?.name ?? ''
    const writable = rosterEntry?.writable ?? false
    return [
      ...first.map((e) => driveRow(e, driveKey ?? '', writable, owner)),
      ...driveMoreRows,
    ]
  })
  const drivePath = $derived((driveQuery.data?.path ?? []) as DriveBrowse['path'])
  const driveMore = $derived(!!driveMoreToken)

  // HEALING: arriving at the Drive place with no drive picked lands on the
  // roster's first entry. replace — housekeeping never pollutes history.
  $effect(() => {
    if (place !== 'drive' || !onArtifacts) return
    if (driveKey || rosterQuery.isLoading || !roster.length) return
    const search: Record<string, string> = { d: roster[0]!.key }
    if (folderId) search.f = String(folderId)
    void navigate('/artifacts/:place', { params: { place: 'drive' }, search, replace: true })
  })

  // IMPORT: the selection bar's Import button — each selected Drive FILE
  // pulled into a Talaria folder named for the Drive folder it came from
  // (root imports land in "Google Drive"). Sequential (Google rate limits
  // parallel pulls), one summary toast, per-file failures counted not shown.
  let importing = $state(0)
  const importSelection = async (fileIds: string[]) => {
    if (!fileIds.length) return
    const folderName = drivePath.length ? drivePath[drivePath.length - 1]!.name : 'Google Drive'
    let created: ReturnType<typeof createFolder> | null = null
    let targetFolder: string | null = null
    try {
      const existing = folders.find((f) => f.name === folderName && !f.parentId)
      targetFolder = existing?.id ?? (await createFolder(folderName, null)).id
      void created
    } catch {
      targetFolder = null // a root import still works
    }
    let ok = 0
    let failed = 0
    for (const id of fileIds) {
      importing++
      try {
        await importDriveFile(id, targetFolder)
        ok++
      } catch {
        failed++
      }
      importing--
    }
    await refresh()
    if (failed) pushToast({ title: `Imported ${ok}, failed ${failed}`, body: 'Files over 25 MB cannot be imported.', tone: 'danger' })
    else pushToast({ title: `Imported ${ok} file${ok === 1 ? '' : 's'}`, body: `Into "${folderName}" in My Files.` })
  }
  // View mode is a preference, not a selection — it belongs to the person, not
  // to the link they might paste to someone else.
  $effect(() => {
    const saved = localStorage.getItem('files:view')
    if (saved === 'grid' || saved === 'list') view = saved
  })
  const setView = (v: 'list' | 'grid') => {
    view = v
    localStorage.setItem('files:view', v)
  }
  const onSort = (k: SortKey) => {
    if (sortKey === k) sortDir = sortDir === 'asc' ? 'desc' : 'asc'
    else {
      sortKey = k
      // Time sorts newest-first on first click; names sort A→Z. Anything else
      // makes the first click on "Modified" look broken.
      sortDir = k === 'modified' ? 'desc' : 'asc'
    }
  }

  const agentsRootId = $derived(folders.find((f) => !f.parentId && f.name === AGENTS_ROOT)?.id ?? null)
  const trail = $derived(place === 'drive' ? [] : ancestry(folderId, folders))
  // Drive crumbs: the roster entry's name is the root crumb, the walked path
  // the segments. Same breadcrumb UI, Google's ancestry.
  const driveCrumbs = $derived(
    place === 'drive'
      ? [{ id: '', name: rosterEntry?.name ?? 'Drive' }, ...drivePath.map((p) => ({ id: p.id, name: p.name }))]
      : [],
  )
  // Folders are locations; the flat places are views over everything.
  const canOrganize = $derived(place === 'my')

  const counts = $derived<Record<Place, number>>({
    my: artifacts.filter((a) => placeOf(a, me) === 'my').length,
    shared: artifacts.filter((a) => placeOf(a, me) === 'shared').length,
    workspace: artifacts.filter((a) => placeOf(a, me) === 'workspace').length,
    official: artifacts.filter((a) => a.official).length,
    recent: 0,
    drive: 0,
    // Counted by the vault itself — this file deliberately does not fetch
    // secrets, so that no artifact code path ever holds one.
    secrets: 0,
  })

  /** Newest artifact inside a folder or any of its descendants — what the
   *  folder's Modified column reports. */
  const folderModified = $derived.by(() => {
    const kids = new Map<string | null, string[]>()
    for (const f of folders) {
      const list = kids.get(f.parentId) ?? []
      list.push(f.id)
      kids.set(f.parentId, list)
    }
    const own = new Map<string, string>()
    for (const a of artifacts) {
      if (!a.folderId) continue
      const cur = own.get(a.folderId)
      if (!cur || a.updatedAt > cur) own.set(a.folderId, a.updatedAt)
    }
    const memo = new Map<string, string>()
    const walk = (id: string, depth: number): string => {
      const hit = memo.get(id)
      if (hit) return hit
      let best = own.get(id) ?? ''
      if (depth < 30) {
        for (const child of kids.get(id) ?? []) {
          const v = walk(child, depth + 1)
          if (v > best) best = v
        }
      }
      memo.set(id, best)
      return best
    }
    for (const f of folders) walk(f.id, 0)
    return memo
  })

  const rows = $derived.by(() => {
    if (place === 'drive') return driveRows
    let out: Row[] = []
    if (place === 'my') {
      // A location: the folders and files filed directly here.
      for (const f of folders) {
        if ((f.parentId ?? null) !== folderId) continue
        out.push(folderRow(f, users, me, folderModified.get(f.id) || f.createdAt))
      }
      for (const a of artifacts) {
        if ((a.folderId ?? null) !== folderId) continue
        // The ROOT of My Files is your cabinet — other people's and the
        // workspace's loose files have their own places. Inside a folder the
        // filter lifts: a folder shows everything filed in it, or walking into
        // Agents/ would show an empty room.
        if (!folderId && placeOf(a, me) !== 'my') continue
        out.push(toRow(a, users, me))
      }
    } else {
      // A view: flat, across everything, no folders.
      const pool =
        place === 'shared' ? artifacts.filter((a) => placeOf(a, me) === 'shared')
        : place === 'workspace' ? artifacts.filter((a) => placeOf(a, me) === 'workspace')
        : place === 'official' ? artifacts.filter((a) => a.official)
        : [...artifacts].sort((a, b) => b.updatedAt.localeCompare(a.updatedAt)).slice(0, 50)
      out = pool.map((a) => toRow(a, users, me))
    }
    const needle = q.trim().toLowerCase()
    if (needle) out = out.filter((r) => r.name.toLowerCase().includes(needle) || r.owner.toLowerCase().includes(needle))
    // Recent is already in the order its name promises; re-sorting it by name
    // would make the place a lie.
    if (place === 'recent' && sortKey === 'name' && !needle) return out
    return sortRows(out, sortKey, sortDir)
  })

  const currentPlace = $derived(PLACES.find((p) => p.id === place) ?? PLACES[0]!)
  const emptyTitle = $derived(q.trim() ? 'No matches.' : place === 'drive' ? 'Nothing in this Drive folder.' : trail.length ? 'This folder is empty.' : currentPlace.empty)
  const emptyHint = $derived(q.trim() ? 'Try a different search.' : canOrganize ? 'Drop files here to upload, or use New.' : currentPlace.hint)

  const refresh = () =>
    Promise.all([qc.invalidateQueries({ queryKey: ['artifacts'] }), qc.invalidateQueries({ queryKey: ['artifact-folders'] })])

  const create = async (kind: ArtifactKind) => {
    let artifact
    try {
      artifact = await createArtifact({ kind, title: 'Untitled' })
      if (folderId) await saveArtifact(artifact.id, { folderId })
    } catch (e) {
      pushToast({ title: 'Could not create', body: errorMessage(e), tone: 'danger' })
      return
    }
    await refresh()
    setActiveId(artifact.id)
  }
  const newFolder = async () => {
    try {
      await createFolder('New folder', folderId)
    } catch (e) {
      pushToast({ title: 'Could not create the folder', body: errorMessage(e), tone: 'danger' })
      return
    }
    await refresh()
  }

  let fileInput = $state<HTMLInputElement | null>(null)
  /** Upload → one file artifact each, filed where you're standing. The POST
   *  body can't carry a folder id, so the folder lands with the storage ref. */
  const upload = async (files: File[], intoFolderId?: string) => {
    const target = intoFolderId ?? folderId
    for (const file of files) {
      try {
        const artifact = await createArtifact({ kind: 'file', title: file.name })
        const up = await uploadFile(file)
        await saveArtifact(artifact.id, { storageRef: up.id, contentType: up.mime, folderId: target })
      } catch {
        // One bad file must not abandon the rest of the drop.
      }
    }
    await refresh()
  }

  const move = async (drag: NonNullable<Drag>, target: string | null) => {
    try {
      for (const id of drag.artifacts) await saveArtifact(id, { folderId: target })
      for (const id of drag.folders) await updateFolder(id, { parentId: target })
    } catch (e) {
      pushToast({ title: 'Move failed', body: errorMessage(e), tone: 'danger' })
    }
    await refresh()
  }

  /** Paste the clipboard into a folder (default: the one you're standing in).
   *  Cut moves; copy duplicates into the destination. Pasting a cut batch
   *  back where it came from is a silent no-op — nothing was asked for. */
  const paste = async (into?: string | null) => {
    const clip = clipboard()
    if (!clip) return
    const target = into === undefined ? folderId : into
    const keys = clip.keys.filter((k) => k.startsWith('artifact:')).map((k) => k.slice('artifact:'.length))
    const folderKeys = clip.keys.filter((k) => k.startsWith('folder:')).map((k) => k.slice('folder:'.length))
    if (clip.mode === 'cut') {
      if (target === clip.fromFolderId) {
        clearClipboard()
        return
      }
      await move({ artifacts: keys, folders: folderKeys }, target)
    } else {
      try {
        for (const id of keys) {
          const copy = await duplicateArtifact(id)
          if (copy.folderId !== target) await saveArtifact(copy.id, { folderId: target })
        }
        for (const id of folderKeys) {
          const copy = await duplicateFolder(id)
          if (copy.parentId !== target) await updateFolder(copy.id, { parentId: target })
        }
      } catch (e) {
        pushToast({ title: 'Paste failed', body: errorMessage(e), tone: 'danger' })
      }
      await refresh()
    }
    clearClipboard()
  }

  // Breadcrumb segments accept drops, which is the only way to move something
  // UP a level now that there is no tree to drag it onto. The payload rides on
  // the dataTransfer (DRAG_MIME) rather than shared state, so the browser and
  // the header don't have to know about each other.
  let crumbOver = $state<string | null>(null)
  const crumbOver_ = (e: DragEvent, id: string) => {
    if (!canOrganize || !e.dataTransfer?.types.includes(DRAG_MIME)) return
    e.preventDefault()
    crumbOver = id
  }
  const crumbDrop = async (e: DragEvent, target: string | null) => {
    crumbOver = null
    const raw = e.dataTransfer?.getData(DRAG_MIME)
    if (!raw) return
    e.preventDefault()
    try {
      const d = JSON.parse(raw) as NonNullable<Drag>
      // Dropping a folder onto itself, or onto the crumb it already sits in,
      // is a no-op rather than a cycle.
      if (target && d.folders.includes(target)) return
      await move(d, target)
    } catch {
      /* a drag from somewhere else in the app — not ours to handle */
    }
  }
  const newMenu = (): ContextMenuEntry[] => [
    ...NEW_KINDS.map((k): ContextMenuEntry => ({ label: k.label, icon: [k.icon, { size: 13 }], onSelect: () => void create(k.kind) })),
    'sep',
    { label: 'Upload files', icon: [Upload, { size: 13 }], onSelect: () => fileInput?.click() },
    { label: 'New folder', icon: [FolderPlus, { size: 13 }], disabled: !canOrganize, onSelect: () => void newFolder() },
  ]
</script>

<RailSurface>
  <Rail>
    <RailSection label="Places">
      {#each PLACES as p (p.id)}
        {#if p.id !== 'drive'}
        <RailRow active={place === p.id && !activeId} onClick={() => goPlace(p.id)}>
          <span class="grid w-4 shrink-0 place-items-center text-[13px] leading-none">{p.glyph}</span>
          <span class="min-w-0 flex-1 truncate">{p.label}</span>
          {#if counts[p.id]}
            <span class="shrink-0 font-mono text-[10px] tracking-[0.05em] text-ink-dim">{counts[p.id]}</span>
          {/if}
        </RailRow>
        {/if}
      {/each}
    </RailSection>

    <RailSection label="Sources">
      <!-- Google Drive IS the browsable source now: the row opens the place,
           and while you're in it the roster expands underneath — one row per
           Drive, visually distinct by connection and kind, the same expansion
           grammar as the knowledgebase's spaces. -->
      <RailRow active={place === 'drive'} onClick={() => goPlace('drive')}>
        <span class="grid w-4 shrink-0 place-items-center"><HardDrive size={13} /></span>
        <span class="min-w-0 flex-1 truncate">Google Drive</span>
        {#if roster.length > 1}
          <span class="shrink-0 font-mono text-[10px] tracking-[0.05em] text-ink-dim">{roster.length}</span>
        {/if}
      </RailRow>
      {#if place === 'drive'}
        <div class="ml-4 mt-1 space-y-0.5 border-l border-line-subtle pl-2">
          {#each roster as d (d.key)}
            <RailRow active={driveKey === d.key} onClick={() => goDrive(d.key)}>
              <span class="grid w-4 shrink-0 place-items-center text-[12px] leading-none">
                {#if d.kind === 'my'}◆{:else}◫{/if}
              </span>
              <span class="min-w-0 flex-1 truncate" title={d.email ?? undefined}>{d.name}</span>
              {#if d.connection === 'org'}
                <span class="shrink-0 font-mono text-[9px] uppercase tracking-[0.08em] text-ink-dim">org</span>
              {/if}
            </RailRow>
          {/each}
        </div>
      {/if}
      <RailRow onClick={() => navigate('/settings')}>
        <span class="grid w-4 shrink-0 place-items-center"><Plus size={13} /></span>
        <span class="min-w-0 flex-1 truncate">Connect a source</span>
      </RailRow>
    </RailSection>
  </Rail>

  <Stage>
    {#snippet header()}
      <StageHeader>
        {#snippet title()}
          <!-- Breadcrumb: the place, then the folders you walked into. Each
               segment is a drop target for moving things back up. -->
          <nav class="flex min-w-0 items-center gap-0.5">
            <button
              type="button"
              onclick={() => goFolder(null)}
              ondragover={(e) => crumbOver_(e, 'root')}
              ondragleave={() => (crumbOver = crumbOver === 'root' ? null : crumbOver)}
              ondrop={(e) => void crumbDrop(e, null)}
              oncontextmenu={(e) => {
                e.preventDefault()
                crumbMenu.openMenu(e, [
                  { label: 'Open', onSelect: () => goFolder(null) },
                  { label: 'Paste into', disabled: !clipboard() || !canOrganize, onSelect: () => void paste(null) },
                ])
              }}
              class={cn('shrink-0 rounded px-1.5 py-0.5 font-sans text-sm font-semibold transition-colors', trail.length ? 'text-muted hover:text-fg' : 'text-fg', crumbOver === 'root' && 'bg-raised ring-1 ring-accent/60')}
            >
              {place === 'drive' ? (rosterEntry?.name ?? 'Google Drive') : currentPlace.label}
            </button>
            {#if place === 'drive'}
              {#each driveCrumbs as f, i (f.id || 'root')}
                {#if i > 0}
                  <ChevronRight size={13} class="shrink-0 text-ink-dim" />
                  <button
                    type="button"
                    onclick={() => goFolder(f.id || null)}
                    class={cn('min-w-0 truncate rounded px-1.5 py-0.5 font-sans text-sm font-semibold transition-colors', i === driveCrumbs.length - 1 ? 'text-fg' : 'text-muted hover:text-fg')}
                  >
                    {f.name}
                  </button>
                {/if}
              {/each}
            {:else}
            {#each trail as f, i (f.id)}
              <ChevronRight size={13} class="shrink-0 text-ink-dim" />
              <button
                type="button"
                onclick={() => goFolder(f.id)}
                ondragover={(e) => crumbOver_(e, f.id)}
                ondragleave={() => (crumbOver = crumbOver === f.id ? null : crumbOver)}
                ondrop={(e) => void crumbDrop(e, f.id)}
                oncontextmenu={(e) => {
                  e.preventDefault()
                  crumbMenu.openMenu(e, [
                    { label: 'Open', onSelect: () => goFolder(f.id) },
                    { label: 'Paste into', disabled: !clipboard() || !canOrganize, onSelect: () => void paste(f.id) },
                  ])
                }}
                class={cn('min-w-0 truncate rounded px-1.5 py-0.5 font-sans text-sm font-semibold transition-colors', i === trail.length - 1 ? 'text-fg' : 'text-muted hover:text-fg', crumbOver === f.id && 'bg-raised ring-1 ring-accent/60')}
              >
                {f.id === agentsRootId ? AGENTS_ROOT : f.name}
              </button>
            {/each}
            {/if}
          </nav>
        {/snippet}
        {#snippet actions()}
          <div class="flex items-center gap-2">
            <div class="relative">
              <Search size={13} class="pointer-events-none absolute left-2 top-1/2 -translate-y-1/2 text-muted" />
              <Input size="sm" bind:value={q} placeholder="Search files" class="h-7 w-44 pl-7 pr-6" />
              {#if q}
                <button type="button" onclick={() => (q = '')} class="absolute right-1.5 top-1/2 -translate-y-1/2 text-muted hover:text-fg" aria-label="Clear search">
                  <X size={12} />
                </button>
              {/if}
            </div>
            <Segmented
              size="xs"
              options={[
                { id: 'list', label: 'List', title: 'List view' },
                { id: 'grid', label: 'Grid', title: 'Grid view' },
              ] as const}
              value={view}
              onChange={setView}
            />
            <DropdownMenu items={newMenu}>
              {#snippet trigger(_open: boolean)}
                <!-- The `+` opens the menu of kinds, which is what says what is
                     being made; "New" beside it only repeats the glyph. -->
                <Button size="sm" title="New file" aria-label="New file">
                  <Plus size={13} />
                </Button>
              {/snippet}
            </DropdownMenu>
          </div>
        {/snippet}
      </StageHeader>
    {/snippet}

    {#if activeId}
      {#key activeId}
        <ArtifactEditor id={activeId} onDeleted={() => setActiveId(null)} />
      {/key}
    {:else if place === 'drive' && notConnected}
      <!-- Both connections absent — the Drive place's front door. The anchor
           passes through the router's api escape (router.ts) so the OAuth
           dance runs for real. -->
      <div class="grid h-full place-items-center">
        <div class="flex max-w-xs flex-col items-center gap-3 p-6 text-center">
          <HardDrive size={22} class="text-accent" />
          <div class="font-sans text-sm font-medium text-fg">Browse Google Drive in Talaria</div>
          <div class="font-sans text-xs leading-relaxed text-muted">Connect a Google account to browse its Drive — folders, shared drives, and one-click import into your files — right here.</div>
          <a href="/api/integrations/google/connect" class="mt-1 rounded-md border border-line bg-panel px-3 py-1.5 font-sans text-sm text-fg transition-colors hover:bg-raised">
            Connect Google
          </a>
        </div>
      </div>
    {:else if place === 'secrets'}
      <!-- Its own surface, not rows in the table. A secret has no body, no
           preview, no export and no public page — feeding it through the
           artifact row pipeline would mean teaching every one of those paths to
           refuse it, and the first one anybody forgets is the last one. -->
      <SecretsVault />
    {:else}
      <ArtifactsBrowser
        {rows}
        loading={place === 'drive' ? driveQuery.isLoading : artifactsQuery.isLoading || foldersQuery.isLoading}
        {failure}
        {view}
        {sortKey}
        {sortDir}
        {onSort}
        {activeId}
        {canOrganize}
        {emptyTitle}
        {emptyHint}
        {folderId}
        onPaste={(into) => paste(into)}
        onOpenFolder={goFolder}
        onOpenArtifact={setActiveId}
        onOpenDriveFile={(url) => url && window.open(url, '_blank', 'noopener')}
        onImport={(fileIds) => importSelection(fileIds)}
        more={place === 'drive' && driveMore}
        onLoadMore={() => loadMoreDrive()}
        importingCount={importing}
        onAscend={() => {
          // ← climbs one level: into the parent folder, or to the place root.
          // The trail is the walked ancestry — its last-but-one segment is the
          // parent; an empty trail means the browser is already at the root.
          if (trail.length >= 1) goFolder(trail[trail.length - 2]?.id ?? null)
        }}
        onMove={move}
        onUpload={upload}
        onRefresh={refresh}
        onDeleted={(id) => activeId === id && setActiveId(null)}
        onProperties={(row) => (propsRow = row)}
        onShare={(row) => (shareRow = row)}
      />
    {/if}
  </Stage>

  <input
    bind:this={fileInput}
    type="file"
    multiple
    class="hidden"
    onchange={(e) => {
      const files = Array.from(e.currentTarget.files ?? [])
      e.currentTarget.value = ''
      if (files.length) void upload(files)
    }}
  />

  {#if propsRow}
    <ArtifactsProperties
      row={propsRow}
      {folders}
      {artifacts}
      placeLabel={currentPlace.label}
      onClose={() => (propsRow = null)}
      onManageAccess={() => {
        // Hand off rather than stack: two modals over each other would leave
        // Esc ambiguous about which one it closes.
        shareRow = propsRow
        propsRow = null
      }}
    />
  {/if}

  {#if shareTarget}
    <PermissionsModal
      open
      onClose={() => (shareRow = null)}
      kind={shareTarget.kind}
      id={shareTarget.id}
      label={shareTarget.label}
      visibility={shareTarget.visibility}
      editPolicy={shareTarget.editPolicy}
      publicSlug={shareTarget.publicSlug}
      canManage={shareTarget.canManage}
      onSave={async (patch) => {
        try {
          if (shareTarget.kind === 'artifacts') await saveArtifact(shareTarget.id, patch)
          else await updateFolder(shareTarget.id, patch)
        } catch (e) {
          pushToast({ title: 'Could not save sharing', body: errorMessage(e), tone: 'danger' })
          return
        }
        await refresh()
      }}
    />
  {/if}

  <!-- The breadcrumb's own menu host (Open / Paste into — the crumb is a
       destination, the keyboard/touch twin of its drop target). -->
  <ContextMenu menu={crumbMenu} />
</RailSurface>
