<script lang="ts">
  import { DownloadCloud, Search } from '@lucide/svelte'
  import { buttonClasses } from '@/components/ui/button'
  import { alert } from '@/components/ui/confirm.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Modal from '@/components/ui/Modal.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import { relativeTime } from '@/lib/fleet'

  interface DriveEntry {
    id: string
    name: string
    mimeType: string
    modifiedTime: string | null
    iconLink: string | null
    sizeBytes: number | null
  }

  // Browse the connected user's Google Drive and pull a file in as an artifact.
  let { onClose, onImported }: { onClose: () => void; onImported: (artifactId: string) => void } = $props()

  let q = $state('')
  let files = $state<DriveEntry[] | null>(null)
  // Named `status` rather than `state`: a variable called `state` collides
  // with the $state rune in Svelte's compiler.
  let status = $state<'loading' | 'ready' | 'not_connected' | 'reconnect' | 'error'>('loading')
  let importing = $state<string | null>(null)

  const load = async (query: string) => {
    status = 'loading'
    const r = await fetch(`/api/integrations/google/drive/files?q=${encodeURIComponent(query)}`).catch(() => null)
    if (!r) {
      status = 'error'
      return
    }
    if (r.ok) {
      files = ((await r.json()) as { files: DriveEntry[] }).files
      status = 'ready'
      return
    }
    const j = (await r.json().catch(() => null)) as { error?: string } | null
    if (j?.error === 'not_connected') {
      status = 'not_connected'
      return
    }
    if (j?.error === 'reconnect_needed') {
      status = 'reconnect'
      return
    }
    status = 'error'
  }
  // Debounced search.
  $effect(() => {
    const query = q
    const t = setTimeout(() => void load(query), query ? 300 : 0)
    return () => clearTimeout(t)
  })

  const doImport = async (fileId: string) => {
    importing = fileId
    try {
      const r = await fetch('/api/integrations/google/drive/import', {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ fileId }),
      })
      const j = (await r.json().catch(() => null)) as { artifact?: { id: string }; message?: string } | null
      if (r.ok && j?.artifact?.id) onImported(j.artifact.id)
      else await alert({ title: 'Import failed', message: j?.message ?? 'Import failed.' })
    } finally {
      importing = null
    }
  }

  const driveKind = (mime: string) =>
    mime === 'application/vnd.google-apps.document' ? '📄 Doc'
    : mime === 'application/vnd.google-apps.spreadsheet' ? '📊 Sheet'
    : mime === 'application/vnd.google-apps.presentation' ? '📽 Slides'
    : mime.startsWith('image/') ? '🖼 Image'
    : '📎 File'
</script>

<Modal open {onClose} width="max-w-lg">
  {#snippet title()}
    <span class="flex items-center gap-2">
      <DownloadCloud size={16} class="text-muted" /> Import from Google Drive
    </span>
  {/snippet}
  {#if status === 'not_connected' || status === 'reconnect'}
    <div class="py-6 text-center">
      <div class="mb-3 text-sm text-muted">
        {status === 'reconnect'
          ? 'Reconnect Google to grant Drive read access.'
          : 'Connect a Google account to browse your Drive.'}
      </div>
      <a href="/api/integrations/google/connect" class={buttonClasses({ size: 'sm' })}>
        {status === 'reconnect' ? 'Reconnect Google' : 'Connect Google'}
      </a>
    </div>
  {:else}
    <div class="mb-2 flex items-center gap-2">
      <Search size={14} class="shrink-0 text-muted" />
      <Input
        size="sm"
        autofocus
        bind:value={q}
        placeholder="Search your Drive"
      />
    </div>
    <div class="max-h-[50vh] min-h-[12rem] overflow-y-auto">
      {#if status === 'loading'}<SkeletonRows rows={6} class="px-2 py-3" />{/if}
      {#if status === 'error'}<div class="p-6 text-center text-xs text-danger">Couldn’t reach Google Drive.</div>{/if}
      {#if status === 'ready' && files && files.length === 0}<EmptyState variant="compact" title="No files found." />{/if}
      {#if status === 'ready' && files}
        {#each files as f (f.id)}
          <button
            type="button"
            disabled={!!importing}
            onclick={() => void doImport(f.id)}
            class="flex w-full items-center gap-3 rounded-md px-2 py-2 text-left transition-colors hover:dither-fill disabled:opacity-50"
          >
            <span class="w-14 shrink-0 text-[11px] text-muted">{driveKind(f.mimeType)}</span>
            <span class="min-w-0 flex-1 truncate text-sm text-fg">{f.name}</span>
            <span class="shrink-0 font-mono text-[10px] tracking-[0.05em] text-muted">
              {importing === f.id ? 'Importing' : f.modifiedTime ? relativeTime(f.modifiedTime) : ''}
            </span>
          </button>
        {/each}
      {/if}
    </div>
  {/if}
</Modal>
