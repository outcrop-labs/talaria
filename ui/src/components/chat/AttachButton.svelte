<script lang="ts">
  import { BookOpen, FolderUp, Gem, ImagePlus, Upload } from '@lucide/svelte'
  import { cn } from '@/lib/cn'
  import WaitingMark from '@/components/ui/WaitingMark.svelte'
  import { popPanel, popRow, tileBase } from '@/components/chat/chat-chrome'
  import { pop, POPOVER } from '@/lib/motion'
  import RefPicker from './RefPicker.svelte'
  import { uploadFile, type Attachment } from '@/lib/attachments'

  // The attach affordance — a context menu, not a bare file browser: attach
  // KNOWLEDGE docs, ARTIFACTS, or uploaded files. Knowledge/artifact picks
  // become reference chips (refType set); the server carries their content to
  // the model and persists the chip on the message.
  let { onAttach, disabled }: { onAttach: (a: Attachment) => void; disabled?: boolean } = $props()

  let fileRef = $state<HTMLInputElement | null>(null)
  let photoRef = $state<HTMLInputElement | null>(null)
  let folderRef = $state<HTMLInputElement | null>(null)
  let wrapRef = $state<HTMLDivElement | null>(null)
  let busy = $state(false)
  let open = $state(false)
  let pick = $state<'kb-doc' | 'artifact' | null>(null)

  const pickFiles = async (files: FileList | null) => {
    if (!files?.length) return
    busy = true
    try {
      for (const f of Array.from(files)) {
        const r = await uploadFile(f)
        if ('id' in r) onAttach(r)
      }
    } finally {
      busy = false
      if (fileRef) fileRef.value = ''
    }
  }

  const item = cn(popRow, 'text-[13px] text-fg')
</script>

<svelte:document
  onmousedown={(e) => {
    if (open && !wrapRef?.contains(e.target as Node)) {
      open = false
      pick = null
    }
  }}
/>

<div bind:this={wrapRef} class="relative">
  <input bind:this={fileRef} type="file" multiple hidden onchange={(e) => void pickFiles(e.currentTarget.files)} />
  <input bind:this={photoRef} type="file" accept="image/*" multiple hidden onchange={(e) => void pickFiles(e.currentTarget.files)} />
  <!-- React set webkitdirectory via an effect (JSX strips the unknown attr); Svelte passes it through directly. -->
  <input bind:this={folderRef} type="file" multiple hidden webkitdirectory onchange={(e) => void pickFiles(e.currentTarget.files)} />
  <!-- The `+` attach tile: 36×36, radius 6, hairline border, mono glyph (spec §7). -->
  <button
    type="button"
    title="Attach knowledge, files, or an upload"
    disabled={disabled || busy}
    onclick={() => {
      open = !open
      pick = null
    }}
    class={cn(tileBase, 'font-mono text-base leading-none')}
  >
    {#if busy}
      <WaitingMark site="chat/attach" size={12} />
    {:else}
      <span aria-hidden="true">+</span>
    {/if}
  </button>
  {#if open}
    <div in:pop={POPOVER} class={cn(popPanel, 'absolute bottom-full left-0 z-30 mb-1.5 w-64')}>
      {#if pick}
        <RefPicker
          kind={pick}
          onPick={(a) => {
            onAttach(a)
            open = false
            pick = null
          }}
        />
      {:else}
        <button type="button" class={item} onclick={() => (pick = 'kb-doc')}>
          <BookOpen size={13} class="text-muted" /> Knowledge doc
        </button>
        <button type="button" class={item} onclick={() => (pick = 'artifact')}>
          <Gem size={13} class="text-muted" /> File
        </button>
        <button
          type="button"
          class={item}
          onclick={() => {
            open = false
            fileRef?.click()
          }}
        >
          <Upload size={13} class="text-muted" /> Upload file
        </button>
        <button
          type="button"
          class={item}
          onclick={() => {
            open = false
            photoRef?.click()
          }}
        >
          <ImagePlus size={13} class="text-muted" /> Add photos
        </button>
        <button
          type="button"
          class={item}
          onclick={() => {
            open = false
            folderRef?.click()
          }}
        >
          <FolderUp size={13} class="text-muted" /> Add folder
        </button>
      {/if}
    </div>
  {/if}
</div>
