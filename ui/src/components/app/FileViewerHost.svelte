<script lang="ts">
  import Modal from '@/components/ui/Modal.svelte'
  import Button from '@/components/ui/Button.svelte'
  import FilePreview from '@/routes/app/FilePreview.svelte'
  import { downloadFile } from '@/lib/download-file'
  import { closeFileViewer, fileViewer } from '@/lib/file-viewer.svelte'

  // The one file modal. Preview when we can; name + type + "cannot preview"
  // when we cannot; Download either way. Mounted once in the app shell.
  const file = $derived(fileViewer())
  let busy = $state(false)
  let error = $state('')

  $effect(() => {
    if (file) {
      busy = false
      error = ''
    }
  })

  const download = async () => {
    if (!file || busy) return
    busy = true
    error = ''
    try {
      await downloadFile(file.url, file.title)
    } catch (e) {
      error = e instanceof Error ? e.message : 'Could not download'
    } finally {
      busy = false
    }
  }
</script>

<Modal
  open={file !== null}
  onClose={closeFileViewer}
  title={file?.title ?? 'File'}
  width="max-w-4xl"
  height="h-[85vh]"
>
  {#if file}
    <FilePreview
      url={file.url}
      title={file.title}
      contentType={file.contentType}
      sizeBytes={file.sizeBytes ?? null}
      showDownload={false}
    />
  {/if}
  {#snippet footer()}
    <div class="flex items-center justify-between gap-3">
      <div class="min-w-0 font-sans text-xs text-danger">{error}</div>
      <Button size="sm" disabled={busy || !file} onclick={() => void download()}>
        {busy ? 'Downloading' : 'Download'}
      </Button>
    </div>
  {/snippet}
</Modal>
