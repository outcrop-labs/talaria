<script lang="ts">
  import { Building2, ExternalLink, Globe, Lock, Users } from '@lucide/svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import CopyButton from '@/components/ui/CopyButton.svelte'
  import Modal from '@/components/ui/Modal.svelte'
  import { relativeTime } from '@/lib/fleet'
  import type { ArtifactFolder } from '@/lib/artifacts'
  import { ancestry, VISIBILITY_LABEL, type Row } from './artifacts'

  // Properties — the desktop file browser's Get Info, in Talaria's voice. It
  // reports; it never edits. Every field the editor already lets you change
  // (title, sharing, official, routing) is changed THERE, so this dialog can
  // stay a single trustworthy read of what the record actually says.
  let {
    row,
    folders,
    artifacts,
    placeLabel,
    onClose,
    onManageAccess,
  }: {
    row: Row
    folders: ArtifactFolder[]
    /** Only used for the folder contents tally. */
    artifacts: { id: string; folderId: string | null }[]
    placeLabel: string
    onClose: () => void
    onManageAccess: () => void
  } = $props()

  const a = $derived(row.artifact)
  /** The folder record behind a folder row. Folders carry the same access
   *  columns as artifacts, so everything below reads one shape. */
  const self = $derived(row.type === 'folder' ? (folders.find((f) => f.id === row.id) ?? null) : null)
  const access = $derived(a ?? self)

  /** Where this thing sits, spelled as a path. */
  const location = $derived.by(() => {
    const parentId = row.type === 'folder' ? (folders.find((f) => f.id === row.id)?.parentId ?? null) : (a?.folderId ?? null)
    const chain = ancestry(parentId, folders)
    return [placeLabel, ...chain.map((f) => f.name)].join(' / ')
  })

  /** Folders report what they hold, one level down — the tally a person opens
   *  Properties on a folder to see. */
  const contents = $derived.by(() => {
    if (row.type !== 'folder') return null
    const subfolders = folders.filter((f) => f.parentId === row.id).length
    const files = artifacts.filter((x) => x.folderId === row.id).length
    const part = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`
    if (!subfolders && !files) return 'Empty'
    return [subfolders ? part(subfolders, 'folder', 'folders') : null, files ? part(files, 'file', 'files') : null].filter(Boolean).join(', ')
  })

  const created = $derived(row.type === 'folder' ? (folders.find((f) => f.id === row.id)?.createdAt ?? null) : (a?.createdAt ?? null))
  const publicUrl = $derived(a?.publicSlug ? `${location_origin()}/a/${a.publicSlug}` : null)
  function location_origin() {
    return typeof window === 'undefined' ? '' : window.location.origin
  }
</script>

{#snippet field(label: string, value: string | null | undefined)}
  {#if value}
    <div class="grid grid-cols-[7rem_minmax(0,1fr)] items-baseline gap-3 py-1.5">
      <span class="font-mono text-[10px] font-semibold uppercase tracking-wide text-muted">{label}</span>
      <span class="min-w-0 break-words font-sans text-[13px] text-fg">{value}</span>
    </div>
  {/if}
{/snippet}

<Modal open {onClose} title="Properties" width="max-w-md">
  <!-- Header: the thing itself, named the way the browser names it. -->
  <div class="mb-3 flex items-start gap-3 border-b border-line-subtle pb-3">
    <span class="grid h-10 w-10 shrink-0 place-items-center rounded-lg bg-card2/50 text-2xl leading-none">
      {row.icon ?? (row.type === 'folder' ? '📁' : '📄')}
    </span>
    <div class="min-w-0 flex-1">
      <div class="break-words font-sans text-sm font-semibold text-fg">{row.name}</div>
      <div class="mt-1 flex flex-wrap items-center gap-1.5">
        <Chip>{row.kindLabel}</Chip>
        {#if access && !access.ownerUserId}<Chip title="Owned by the organization, not by a person">Workspace</Chip>{/if}
        {#if a?.official}<Chip tone="accent">Official</Chip>{/if}
      </div>
    </div>
  </div>

  <div class="divide-y divide-line-subtle">
    {@render field('Location', location)}
    {@render field('Contents', contents)}
    <!-- Belongs-to comes before who-can-see-it, because it's the question
         people actually get wrong: an org agent's report has no owner and is
         the workspace's, however it happens to be shared. -->
    {@render field('Belongs to', access ? (!access.ownerUserId ? 'The workspace (no personal owner)' : row.owner === 'me' ? 'You' : row.owner) : null)}
    {@render field('Created by', access?.createdBy)}
    {@render field('Created', created ? relativeTime(created) : null)}
    {@render field('Modified', `${relativeTime(row.modified)}${a?.updatedBy ? ` by ${a.updatedBy}` : ''}`)}
    {@render field('File type', a?.contentType)}
    <!-- RAG routing is the one field with no plain-English home elsewhere in
         the browser, and it decides whether a model can ever see this. -->
    {@render field('Assistant access', a ? (a.ragRouting === 'none' ? 'Not used for answers' : a.ragRouting === 'auto' ? 'Automatic' : a.ragRouting) : null)}

    {#if access}
      <!-- Sharing gets its own block, not another read-only row: it's the one
           property people come here to CHANGE. The values stay a summary; the
           real editing happens in the same PermissionsModal the editor and the
           knowledgebase use, so access means one thing app-wide. -->
      <div class="py-2.5">
        <div class="mb-1.5 flex items-center gap-2">
          <span class="font-mono text-[10px] font-semibold uppercase tracking-wide text-muted">Sharing</span>
          <button type="button" onclick={onManageAccess} class="ml-auto flex items-center gap-1 font-mono text-[10px] uppercase tracking-[0.05em] text-muted transition-colors hover:text-fg">
            <Users size={11} /> Manage access
          </button>
        </div>
        <div class="rounded-xl border border-line-subtle bg-surface p-3">
          <div class="flex items-start gap-2.5">
            <span class="mt-0.5 shrink-0 text-muted">
              {#if access.visibility === 'public'}<Globe size={14} />{:else if access.visibility === 'org'}<Building2 size={14} />{:else}<Lock size={14} />{/if}
            </span>
            <div class="min-w-0">
              <div class="font-sans text-[13px] text-fg">{VISIBILITY_LABEL[access.visibility] ?? access.visibility}</div>
              <div class="mt-0.5 font-sans text-[11px] text-muted">
                {#if access.visibility === 'private'}
                  Only you and the people and agents you add{row.type === 'folder' ? ' can see this folder.' : '.'}
                {:else if access.visibility === 'org'}
                  Anyone signed in to this workspace can read {row.type === 'folder' ? 'it and browse inside.' : 'it.'}
                {:else}
                  Live on the internet to anyone holding the link.
                {/if}
              </div>
              <div class="mt-1.5 font-mono text-[10px] uppercase tracking-[0.05em] text-muted">
                Editing: {access.editPolicy === 'org' ? 'everyone in the workspace' : 'the owner and named editors'}
              </div>
              {#if row.type === 'folder'}
                <!-- Say the limit out loud. Folder access governs the FOLDER;
                     each file inside still carries its own, so sharing a folder
                     is not a blanket grant over everything in it. -->
                <div class="mt-1.5 font-sans text-[11px] text-muted">Files inside keep their own sharing.</div>
              {/if}
            </div>
          </div>
        </div>
      </div>
    {/if}

    {#if publicUrl}
      <div class="grid grid-cols-[7rem_minmax(0,1fr)] items-center gap-3 py-1.5">
        <span class="font-mono text-[10px] font-semibold uppercase tracking-wide text-muted">Public link</span>
        <span class="flex min-w-0 items-center gap-1.5">
          <a href={publicUrl} target="_blank" rel="noreferrer" class="min-w-0 truncate font-mono text-[11px] text-accent hover:underline">{publicUrl}</a>
          <CopyButton value={publicUrl} />
        </span>
      </div>
    {/if}

    {#if a?.googleFileUrl}
      <div class="grid grid-cols-[7rem_minmax(0,1fr)] items-center gap-3 py-1.5">
        <span class="font-mono text-[10px] font-semibold uppercase tracking-wide text-muted">Google Drive</span>
        <a href={a.googleFileUrl} target="_blank" rel="noreferrer" class="flex items-center gap-1 font-sans text-[13px] text-accent hover:underline">
          Open in Drive <ExternalLink size={11} />
        </a>
      </div>
    {/if}

    <!-- The id is what a support conversation or an API call actually needs;
         mono, and copyable, because nobody retypes a uuid. -->
    <div class="grid grid-cols-[7rem_minmax(0,1fr)] items-center gap-3 py-1.5">
      <span class="font-mono text-[10px] font-semibold uppercase tracking-wide text-muted">Identifier</span>
      <span class="flex min-w-0 items-center gap-1.5">
        <span class="min-w-0 truncate font-mono text-[11px] text-muted">{row.id}</span>
        <CopyButton value={row.id} />
      </span>
    </div>
  </div>
</Modal>
