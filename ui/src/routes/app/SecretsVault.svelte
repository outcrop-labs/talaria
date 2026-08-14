<script lang="ts">
  // FILES → SECRETS. The credentials a person saves while building something,
  // shares with teammates, and reads back.
  //
  // WHY IT IS A PLACE AND NOT A ROW TYPE. Everything else in this browser is an
  // artifact, and an artifact's body is indexed for retrieval, exported to
  // Google, downloadable, and served unauthenticated at /api/artifacts/public/
  // $slug. A credential that entered that pipeline would be one visibility click
  // from the open internet. So a secret is never an artifact row — it lives in
  // its own place, on its own store, and shares only the CABINET with them.
  //
  // THE TWO KINDS OF SHARING ARE DRAWN DIFFERENTLY ON PURPOSE. A person you
  // share with can reveal it. An agent you grant it to gets a handle it can
  // spend and can never read. Those are different powers, and a UI that showed
  // them as one list of names would be lying about what it just did.
  import { onDestroy } from 'svelte'
  import Button from '@/components/ui/Button.svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import { confirm } from '@/components/ui/confirm.svelte'
  import { listStagger, slide } from '@/lib/motion'
  import { matchesSecret, REVEAL_ERROR, useSecretFolders, useSecretsVault, useWorkingSecrets, type SecretFolder, type WorkingSecret } from '@/lib/secrets-vault'
  import { useSession } from '@/lib/session'
  import { useContextMenu, type ContextMenuEntry } from '@/components/ui/context-menu.svelte'
  import ContextMenu from '@/components/ui/ContextMenu.svelte'
  import { Check, ChevronRight, Copy, Eye, EyeOff, Folder, FolderPlus, KeyRound, MoreHorizontal, Plus, Search, Share2, X } from '@lucide/svelte'
  import SecretShareModal from './SecretShareModal.svelte'

  const query = useWorkingSecrets()
  const vault = useSecretsVault()
  const session = useSession()
  const meId = $derived(session.data?.id ?? null)
  const all = $derived(query.data?.secrets ?? [])

  // ── Filing, searching, and telling mine from yours ─────────────────────────
  //
  // The first cut of this place was a flat list, and it lost every affordance
  // the rest of the browser has: no folders, no search, and no way to see at a
  // glance which of these were yours. That is a fine shape for three secrets and
  // useless at thirty — and thirty is the number a team of four reaches in a
  // quarter. The safety argument for keeping secrets off the artifact ROW
  // pipeline never required keeping them off its IDEAS.
  //
  // FOLDERS ARE THE ARTIFACT FOLDERS, deliberately. Somebody organising their
  // work wants "Checkout rewrite" to hold the spec, the notes and the staging
  // key, not two filing systems whose names drift apart in a week. Only the
  // shelf is shared; a secret still has no body to index or export.
  const foldersQuery = useSecretFolders()
  const folders = $derived(foldersQuery.data?.folders ?? [])
  const folderName = (id: string | null) => (id ? (folders.find((f) => f.id === id)?.name ?? 'Unknown folder') : null)
  const menu = useContextMenu()

  let folderId = $state<string | null>(null)
  let needle = $state('')

  // SEARCH IGNORES THE FOLDER. Looking for something by name means you do not
  // know where it is — a search that only looked in the folder you happened to
  // be standing in would answer "not found" about a secret three feet away.
  const searching = $derived(needle.trim().length > 0)
  const visible = $derived(all.filter((s) => matchesSecret(s, needle) && (searching || s.folderId === folderId)))

  const mineList = $derived(visible.filter((s) => s.ownerUserId === meId))
  const sharedList = $derived(visible.filter((s) => s.ownerUserId !== meId))

  // FLAT, on purpose. Somebody with thirty credentials wants six labelled
  // piles, not a tree — and a tree is where "which folder was that in" starts
  // costing more than the tidying saved. Empty folders still show, because a
  // folder you just made and have not filled yet is not a bug.
  const childFolders = $derived(searching || folderId ? [] : folders)
  const trail = $derived.by(() => {
    const here = folders.find((f) => f.id === folderId)
    return here ? [{ id: null, name: 'Secrets' }, { id: here.id, name: here.name }] : [{ id: null, name: 'Secrets' }]
  })
  const openFolder = $derived(folders.find((f) => f.id === folderId) ?? null)

  let newFolderName = $state('')
  const addFolder = async () => {
    const name = newFolderName.trim()
    if (!name) return
    newFolderName = ''
    await run(() => vault.newFolder(name))
  }

  /** The row's ⋯ menu. A dropdown form control inline in a dense row was the
   *  first attempt and it was illegible — a full-width input squeezed between
   *  chips. Row actions belong in the menu idiom the rest of the app uses. */
  const rowMenu = (e: MouseEvent, s: WorkingSecret) => {
    const items: ContextMenuEntry[] = [
      { label: 'Share…', onSelect: () => (sharingName = s.name) },
      ...(mine(s)
        ? ([
            {
              label: 'Move to',
              children: [
                { label: 'No folder', checked: s.folderId === null, onSelect: () => void run(() => vault.move(s.name, null)) },
                ...folders
                  .filter((f) => f.ownerUserId === meId)
                  .map((f) => ({ label: f.name, checked: s.folderId === f.id, onSelect: () => void run(() => vault.move(s.name, f.id)) })),
              ],
            },
            'sep',
            { label: 'Delete', danger: true, onSelect: () => void remove(s) },
          ] as ContextMenuEntry[])
        : []),
    ]
    menu.openMenu(e, items)
  }

  const folderMenu = (e: MouseEvent, f: SecretFolder) =>
    menu.openMenu(e, [
      { label: 'Share folder…', onSelect: () => (sharingFolderId = f.id) },
      'sep',
      {
        label: 'Delete folder',
        danger: true,
        onSelect: () => void deleteFolder(f),
      },
    ])

  const deleteFolder = async (f: SecretFolder) => {
    const ok = await confirm({
      title: `Delete “${f.name}”?`,
      // THE REASSURANCE IS THE POINT. Losing four working keys because somebody
      // tidied a label would be an unforgivable way to lose them.
      message:
        f.count > 0
          ? `The ${f.count} secret${f.count === 1 ? '' : 's'} in it are NOT deleted — they move back to the top level. Anyone this folder was shared with loses access to them.`
          : 'This folder is empty.',
      confirmLabel: 'Delete folder',
      danger: true,
    })
    if (!ok) return
    await run(() => vault.deleteFolder(f.id))
    if (folderId === f.id) folderId = null
  }

  let busy = $state(false)
  let msg = $state<string | null>(null)

  // ── Revealed values ────────────────────────────────────────────────────────
  //
  // Keyed `name/key`, held here and NOWHERE else — not in the query cache, not
  // on the secret object. Auto-cleared on a timer, because the realistic threat
  // is not an attacker, it is a shared screen and a person who walked away.
  const HIDE_AFTER_MS = 30_000
  let shown = $state<Record<string, string>>({})
  let copied = $state<string | null>(null)
  const timers = new Map<string, ReturnType<typeof setTimeout>>()

  const hide = (id: string) => {
    const { [id]: _drop, ...rest } = shown
    shown = rest
    clearTimeout(timers.get(id))
    timers.delete(id)
  }
  // Every pending timer dies with the component; a reveal must not outlive the
  // page that asked for it.
  onDestroy(() => {
    for (const t of timers.values()) clearTimeout(t)
    shown = {}
  })

  const reveal = async (s: WorkingSecret, key: string) => {
    const id = `${s.name}/${key}`
    if (shown[id]) {
      hide(id)
      return
    }
    msg = null
    const r = await vault.reveal(s.name, key)
    if (r.error || r.value === undefined) {
      msg = REVEAL_ERROR[r.error ?? ''] ?? r.error ?? 'could not reveal that'
      return
    }
    shown = { ...shown, [id]: r.value }
    timers.set(
      id,
      setTimeout(() => hide(id), HIDE_AFTER_MS),
    )
  }

  // COPY WITHOUT LOOKING is the common case — somebody pasting into a .env does
  // not need it on screen at all, and not showing it is strictly safer.
  const copy = async (s: WorkingSecret, key: string) => {
    const r = await vault.reveal(s.name, key)
    if (r.error || r.value === undefined) {
      msg = REVEAL_ERROR[r.error ?? ''] ?? 'could not copy that'
      return
    }
    await navigator.clipboard.writeText(r.value).catch(() => {
      msg = 'clipboard refused — reveal it and copy by hand'
    })
    copied = `${s.name}/${key}`
    setTimeout(() => (copied = null), 1500)
  }

  // ── The draft ──────────────────────────────────────────────────────────────
  let open = $state(false)
  let title = $state('')
  let note = $state('')
  let hosts = $state('')
  let entries = $state<Array<{ key: string; label: string; value: string }>>([{ key: '', label: '', value: '' }])

  const reset = () => {
    title = ''
    note = ''
    hosts = ''
    entries = [{ key: '', label: '', value: '' }]
  }

  const save = async () => {
    busy = true
    msg = null
    const clean = entries.filter((e) => e.key.trim() && e.value)
    const r = await vault.save({
      title: title.trim(),
      note: note.trim() || null,
      entries: clean.map((e) => ({ key: e.key.trim(), label: e.label.trim() || 'Credential', value: e.value })),
      allowedHosts: hosts
        .split(/[\s,]+/)
        .map((h) => h.trim().toLowerCase())
        .filter(Boolean),
    })
    busy = false
    // Cleared either way: on success it is stored, and on failure it is a live
    // credential sitting in a browser tab.
    reset()
    if (r.error) msg = r.error
    else open = false
  }

  const remove = async (s: WorkingSecret) => {
    const ok = await confirm({
      title: `Delete ${s.title}?`,
      message:
        `This deletes the value and every share on it. ${s.readers.length > 0 ? `${s.readers.length} ${s.readers.length === 1 ? 'person' : 'people'} will lose access. ` : ''}` +
        'There is no undo and no copy — if you need it again, save it again.',
      confirmLabel: 'Delete',
      danger: true,
    })
    if (!ok) return
    busy = true
    const r = await vault.remove(s.name)
    busy = false
    msg = r.error ?? null
  }

  const mine = (s: WorkingSecret) => s.ownerUserId === meId

  const run = async (fn: () => Promise<{ error?: string }>) => {
    busy = true
    msg = null
    const r = await fn()
    busy = false
    if (r.error) msg = r.error
  }

  // The share dialog's target. Re-derived from the live list rather than held as
  // a snapshot, so the modal re-renders as grants land instead of showing the
  // list as it was when it opened.
  let sharingName = $state<string | null>(null)
  const sharing = $derived(all.find((s) => s.name === sharingName) ?? null)
  let sharingFolderId = $state<string | null>(null)
  const sharingFolder = $derived(folders.find((f) => f.id === sharingFolderId) ?? null)
</script>

<div class="flex min-h-0 flex-1 flex-col gap-3 overflow-y-auto p-4">
  <div>
    <!-- The same breadcrumb idiom the file browser uses, because this IS the
         file browser as far as anybody standing here is concerned. -->
    <div class="flex flex-wrap items-center gap-1">
      {#each trail as crumb, i (crumb.id ?? 'root')}
        {#if i > 0}<ChevronRight size={12} class="text-ink-dim" aria-hidden="true" />{/if}
        <button
          type="button"
          onclick={() => (folderId = crumb.id)}
          class="font-sans text-sm text-fg transition-colors hover:text-accent disabled:text-fg"
          disabled={i === trail.length - 1}
        >
          {crumb.name}
        </button>
      {/each}
    </div>
    <p class="mt-1 max-w-2xl font-sans text-xs text-muted">
      Credentials you are working with — a staging key, a test token. Sealed here, shared deliberately, and every reveal is recorded. Share with a
      teammate and they can read it; grant it to an agent and it can <em>use</em> it without ever seeing the value.
    </p>
  </div>

  <label class="relative block">
    <Search size={13} class="pointer-events-none absolute left-2 top-1/2 -translate-y-1/2 text-ink-dim" aria-hidden="true" />
    <input
      bind:value={needle}
      placeholder="Search secrets by name, note or entry — never by value"
      class="w-full rounded-md border border-line bg-panel py-1.5 pl-7 pr-2 font-sans text-xs text-fg"
    />
  </label>

  {#if query.isPending}
    <Skeleton class="h-24" />
  {:else if query.isError}
    <QueryError error={query.error} />
  {:else}
    {#if !searching && !folderId}
      <ul class="space-y-1" use:listStagger>
        {#each childFolders as f (f.id)}
          <li
            class="flex items-center gap-2 rounded-md border border-line px-3 py-2 transition-colors hover:bg-hover"
            oncontextmenu={(e) => folderMenu(e, f)}
          >
            <button type="button" onclick={() => (folderId = f.id)} class="flex min-w-0 flex-1 items-center gap-2 text-left">
              <Folder size={14} class="text-muted" aria-hidden="true" />
              <span class="truncate font-sans text-sm text-fg">{f.name}</span>
              <span class="font-mono text-[10px] text-ink-dim">{f.count}</span>
              <!-- WHO ELSE HAS THE WHOLE PILE. The reason folder sharing exists
                   is that it covers what lands in the folder LATER, so saying
                   how far it reaches belongs on the folder itself. -->
              {#if f.readers.length > 0 || f.grants.length > 0}
                <span class="font-sans text-xs text-muted">
                  shared · {f.readers.length} {f.readers.length === 1 ? 'person' : 'people'}{#if f.grants.length > 0}, {f.grants.length} agent{f.grants.length === 1 ? '' : 's'}{/if}
                </span>
              {/if}
              {#if f.ownerUserId !== meId}<span class="font-mono text-[10px] text-ink-dim">shared with you</span>{/if}
            </button>
            {#if f.ownerUserId === meId}
              <button
                type="button"
                title="More"
                onclick={(e) => folderMenu(e, f)}
                class="grid h-7 w-7 shrink-0 place-items-center rounded-md text-muted transition-colors hover:bg-hover hover:text-fg"
              >
                <MoreHorizontal size={14} aria-hidden="true" />
              </button>
            {/if}
          </li>
        {/each}
        <li class="flex items-center gap-2">
          <FolderPlus size={14} class="text-muted" aria-hidden="true" />
          <input
            bind:value={newFolderName}
            placeholder="New folder"
            onkeydown={(e) => {
              if (e.key === 'Enter') {
                e.preventDefault()
                void addFolder()
              }
            }}
            class="w-48 rounded border border-line bg-panel px-2 py-1 font-sans text-xs text-fg"
          />
          {#if newFolderName.trim()}
            <Button size="sm" variant="ghost" onclick={addFolder} disabled={busy}>Create</Button>
          {/if}
        </li>
      </ul>
    {/if}

    {#if openFolder}
      <!-- Standing INSIDE a folder: its sharing is the thing to act on here. -->
      <div class="flex flex-wrap items-center gap-2 rounded-md border border-line px-3 py-2">
        <Folder size={14} class="text-muted" aria-hidden="true" />
        <span class="font-sans text-xs text-muted">
          {#if openFolder.readers.length === 0 && openFolder.grants.length === 0}
            This folder is not shared.
          {:else}
            Shared with {openFolder.readers.length} {openFolder.readers.length === 1 ? 'person' : 'people'}{#if openFolder.grants.length > 0}, and {openFolder.grants.length} agent{openFolder.grants.length === 1 ? '' : 's'} may spend everything in it{/if}.
          {/if}
        </span>
        {#if openFolder.ownerUserId === meId}
          <Button size="sm" variant="ghost" onclick={() => (sharingFolderId = openFolder.id)}>
            <Share2 size={13} aria-hidden="true" />
            Share folder
          </Button>
        {/if}
      </div>
    {/if}

    {#if visible.length === 0}
      <EmptyState
        variant="compact"
        title={searching ? 'Nothing matches' : folderId ? 'Nothing filed here' : 'No secrets yet'}
        hint={searching
          ? 'Search looks at names, notes and entry labels — never at values.'
          : 'Save one instead of pasting it into chat — a key in a message is in the transcript, the database, and every later prompt.'}
      />
    {:else}
      {#if searching}
        <p class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">{visible.length} matching, across every folder</p>
      {/if}
      <!-- MINE AND SHARED ARE DIFFERENT THINGS, and the difference is not
           cosmetic: you can delete, move and re-share one of these lists and
           not the other. -->
      {#each [{ label: 'Mine', rows: mineList }, { label: 'Shared with me', rows: sharedList }] as section (section.label)}
        {#if section.rows.length > 0}
          <p class="mt-1 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">{section.label}</p>
          <ul class="space-y-2" use:listStagger>
            {#each section.rows as s (s.id)}
          <li class="rounded-md border border-line p-3" transition:slide oncontextmenu={(e) => rowMenu(e, s)}>
            <div class="flex flex-wrap items-baseline gap-2">
              <KeyRound size={14} class="text-muted" aria-hidden="true" />
              <span class="font-sans text-sm text-fg">{s.title}</span>
              {#if !mine(s)}<Chip tone="neutral">shared with you</Chip>{/if}
              {#if s.allowedHosts.length > 0}<Chip tone="neutral">{s.allowedHosts.join(', ')}</Chip>{/if}
              <span class="ml-auto font-mono text-[10px] text-ink-dim">{new Date(s.createdAt).toLocaleDateString()}</span>
            </div>
            {#if s.note}<p class="mt-1 font-sans text-xs text-muted">{s.note}</p>{/if}

            <div class="mt-2 space-y-1">
              {#each s.entries as e (e.key)}
                {@const id = `${s.name}/${e.key}`}
                <div class="flex flex-wrap items-center gap-2 rounded border border-line bg-panel px-2 py-1.5">
                  <span class="font-sans text-xs text-muted">{e.label}</span>
                  <code class="font-mono text-[11px] text-ink-dim">{e.key}</code>
                  <code class="ml-auto max-w-[22rem] truncate font-mono text-[11px] text-fg">
                    {shown[id] ?? '••••••••••••'}
                  </code>
                  <!-- Copy WITHOUT looking is the common case — pasting into a
                       .env needs the clipboard, not the screen. -->
                  <button type="button" title="Copy without showing" onclick={() => void copy(s, e.key)} class="text-muted hover:text-fg">
                    {#if copied === id}<Check size={13} aria-hidden="true" />{:else}<Copy size={13} aria-hidden="true" />{/if}
                  </button>
                  <button
                    type="button"
                    title={shown[id] ? 'Hide' : 'Reveal — this is recorded'}
                    onclick={() => void reveal(s, e.key)}
                    class="text-muted hover:text-fg"
                  >
                    {#if shown[id]}<EyeOff size={13} aria-hidden="true" />{:else}<Eye size={13} aria-hidden="true" />{/if}
                  </button>
                </div>
              {/each}
            </div>

            <!-- WHO HAS IT, in the two flavours that mean different things —
                 people who can READ it and agents that can SPEND it. Summarised
                 here; changed in the dialog. -->
            <div class="mt-2 flex flex-wrap items-center gap-1.5">
              <span class="font-mono text-[10px] text-ink-dim">shared with</span>
              <span class="font-sans text-xs text-muted">
                {s.readers.length === 0 ? 'nobody' : `${s.readers.length} ${s.readers.length === 1 ? 'person' : 'people'}`}
                {#if s.grants.length > 0}· {s.grants.length} agent{s.grants.length === 1 ? '' : 's'}{/if}
              </span>
              <Button size="sm" variant="ghost" onclick={() => (sharingName = s.name)}>
                <Share2 size={13} aria-hidden="true" />
                Share
              </Button>
              {#if searching && s.folderId}
                <!-- SEARCH CROSSES FOLDERS, so a result has to say where it
                     lives or the next click is a hunt. -->
                <span class="inline-flex items-center gap-1 font-mono text-[10px] text-ink-dim">
                  <Folder size={11} aria-hidden="true" />
                  {folderName(s.folderId)}
                </span>
              {/if}
              <!-- ONE ⋯ MENU instead of a rank of controls. The first version
                   put a full-width dropdown inline here and it was illegible —
                   a form control squeezed between chips, fighting everything
                   around it. Row actions belong in the menu idiom the rest of
                   the app already uses, and right-clicking the row opens the
                   same items. -->
              <button
                type="button"
                title="More"
                onclick={(e) => rowMenu(e, s)}
                class="ml-auto grid h-7 w-7 shrink-0 place-items-center rounded-md text-muted transition-colors hover:bg-hover hover:text-fg"
              >
                <MoreHorizontal size={14} aria-hidden="true" />
              </button>
              </div>
            </li>
            {/each}
          </ul>
        {/if}
      {/each}
    {/if}

    <div>
      {#if !open}
        <Button size="sm" onclick={() => (open = true)}>
          <Plus size={14} aria-hidden="true" />
          Save a secret
        </Button>
      {:else}
        <div class="rounded-md border border-line p-3" transition:slide>
          <label class="block font-sans text-xs text-muted">
            What is it
            <input bind:value={title} placeholder="Staging Stripe key" class="mt-1 w-full rounded border border-line bg-panel px-2 py-1 font-sans text-xs text-fg" />
          </label>
          <label class="mt-2 block font-sans text-xs text-muted">
            Note (optional)
            <input bind:value={note} placeholder="For the checkout rewrite — rotate after launch" class="mt-1 w-full rounded border border-line bg-panel px-2 py-1 font-sans text-xs text-fg" />
          </label>

          <p class="mt-3 font-mono text-[10px] text-ink-dim">entries</p>
          {#each entries as e, i (i)}
            <div class="mt-1 grid gap-1 sm:grid-cols-[1fr_1fr_2fr_auto]">
              <input bind:value={e.key} placeholder="secret_key" class="rounded border border-line bg-panel px-2 py-1 font-mono text-xs text-fg" />
              <input bind:value={e.label} placeholder="Secret key" class="rounded border border-line bg-panel px-2 py-1 font-sans text-xs text-fg" />
              <input bind:value={e.value} type="password" autocomplete="off" placeholder="value" class="rounded border border-line bg-panel px-2 py-1 font-mono text-xs text-fg" />
              <Button size="sm" variant="ghost" disabled={entries.length === 1} onclick={() => (entries = entries.filter((_, n) => n !== i))} title="Remove">
                <X size={13} aria-hidden="true" />
              </Button>
            </div>
          {/each}
          <Button size="sm" variant="ghost" class="mt-1" onclick={() => (entries = [...entries, { key: '', label: '', value: '' }])}>
            <Plus size={13} aria-hidden="true" />
            Another entry
          </Button>

          <label class="mt-3 block font-sans text-xs text-muted">
            Spendable at (optional) — hosts, space or comma separated
            <input bind:value={hosts} placeholder="api.stripe.com" class="mt-1 w-full rounded border border-line bg-panel px-2 py-1 font-mono text-xs text-fg" />
            <span class="mt-1 block text-[11px] text-ink-dim">
              Only matters once an agent can spend it: Talaria then refuses to substitute it into anything bound elsewhere.
            </span>
          </label>

          <div class="mt-3 flex items-center gap-2">
            <Button size="sm" onclick={save} disabled={busy || !title.trim() || !entries.some((e) => e.key.trim() && e.value)}>Save</Button>
            <Button
              size="sm"
              variant="ghost"
              onclick={() => {
                reset()
                open = false
              }}
              disabled={busy}>Cancel</Button
            >
          </div>
        </div>
      {/if}
    </div>
  {/if}

  {#if msg}<p class="font-sans text-xs text-danger">{msg}</p>{/if}
</div>

<ContextMenu {menu} />

{#if sharing}
  <SecretShareModal
    title={sharing.title}
    readers={sharing.readers}
    grants={sharing.grants}
    ownerUserId={sharing.ownerUserId}
    handle={`«secret:${sharing.name}»`}
    canManage={mine(sharing)}
    onShare={(u, on) => vault.share(sharing.name, u, on)}
    onGrant={(a, on) => vault.grant(sharing.name, a, on)}
    onClose={() => (sharingName = null)}
  />
{/if}

{#if sharingFolder}
  <!-- SAME DIALOG, ONE LEVEL UP. Sharing a folder covers what lands in it
       LATER, which is the whole reason it beats sharing four secrets by hand. -->
  <SecretShareModal
    title={sharingFolder.name}
    readers={sharingFolder.readers}
    grants={sharingFolder.grants}
    ownerUserId={sharingFolder.ownerUserId}
    canManage={sharingFolder.ownerUserId === meId}
    onShare={(u, on) => vault.shareFolder(sharingFolder.id, { userId: u }, on)}
    onGrant={(a, on) => vault.shareFolder(sharingFolder.id, { agentModel: a }, on)}
    onClose={() => (sharingFolderId = null)}
  />
{/if}
