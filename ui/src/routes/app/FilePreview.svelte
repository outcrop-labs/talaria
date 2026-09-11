<script lang="ts">
  import { FileAudio, FileText, FileVideo, Paperclip } from '@lucide/svelte'
  import { onMount } from 'svelte'
  import { getText } from '@/lib/fetch-json'

  // The file preview — one component, every common filetype, no browser-view
  // dumps. The PICKER below is the single source of what "common" means:
  // images, PDF, plain text (code, markdown, csv, json…), audio, video.
  // Everything else gets the honest card — name, type, size, Download —
  // because pretending to render what we can't is worse than saying so.
  //
  // Text previews FETCH the bytes (they're not in the artifact row) and cap
  // at 512 KB: beyond that a browser tab is the right tool, and the card
  // says so. PDF/audio/video ride <embed>/<audio>/<video> — the browser's
  // own renderers, inside OUR chrome.
  let {
    url,
    title,
    contentType,
    sizeBytes = null,
  }: {
    url: string
    title: string
    contentType: string | null
    sizeBytes?: number | null
  } = $props()

  const family = $derived.by(() => {
    const ct = (contentType ?? '').toLowerCase()
    if (ct.startsWith('image/')) return 'image' as const
    if (ct === 'application/pdf') return 'pdf' as const
    if (ct.startsWith('audio/')) return 'audio' as const
    if (ct.startsWith('video/')) return 'video' as const
    if (
      ct.startsWith('text/') ||
      ct === 'application/json' ||
      ct === 'application/xml' ||
      /\.(md|txt|csv|json|ya?ml|toml|xml|html?|css|js|ts|tsx|jsx|py|rs|go|java|rb|sh|sql|log|env|ini|conf)$/i.test(title)
    ) {
      return 'text' as const
    }
    return 'other' as const
  })

  const humanSize = $derived(
    sizeBytes == null
      ? null
      : sizeBytes > 1024 * 1024
        ? `${(sizeBytes / 1024 / 1024).toFixed(1)} MB`
        : sizeBytes > 1024
          ? `${Math.round(sizeBytes / 1024)} KB`
          : `${sizeBytes} B`,
  )

  // Text preview state: fetched through the one HTTP door (getText — raw
  // bodies are what it exists for), hard-capped, no streaming UI.
  let text = $state<string | null>(null)
  let textError = $state(false)
  let truncated = $state(false)
  onMount(() => {
    if (family !== 'text') return
    getText(url)
      .then((t) => {
        if (t.length > 512 * 1024) {
          text = t.slice(0, 512 * 1024)
          truncated = true
        } else {
          text = t
        }
      })
      .catch(() => (textError = true))
  })
</script>

{#if family === 'image'}
  <img src={url} alt={title} class="mx-auto max-h-[70vh] rounded-lg border border-line" />
{:else if family === 'pdf'}
  <!-- embed, not iframe: the browser's own PDF chrome inside ours; fallback
       text rides the <embed> for the engines without one. -->
  <embed src={url} type="application/pdf" class="h-[70vh] w-full rounded-lg border border-line" />
{:else if family === 'audio'}
  <div class="mx-auto flex max-w-xl flex-col items-center gap-3 rounded-lg border border-line bg-panel p-6">
    <FileAudio size={22} class="text-accent" />
    <div class="min-w-0 truncate font-sans text-sm text-fg">{title}</div>
    <audio controls src={url} class="w-full"></audio>
  </div>
{:else if family === 'video'}
  <video controls src={url} class="mx-auto max-h-[70vh] w-full rounded-lg border border-line"></video>
{:else if family === 'text'}
  {#if textError}
    <div class="font-sans text-sm text-muted">Preview did not load — download still works.</div>
  {:else if text === null}
    <div class="font-sans text-sm text-muted">Loading preview…</div>
  {:else}
    <div class="overflow-hidden rounded-lg border border-line">
      <pre class="max-h-[65vh] overflow-auto whitespace-pre-wrap break-words p-4 font-mono text-xs leading-relaxed text-fg">{text}</pre>
    </div>
    {#if truncated}
      <div class="mt-2 font-mono text-[10px] uppercase tracking-[0.05em] text-muted">
        First 512 KB shown — download for the whole file
      </div>
    {/if}
  {/if}
{:else}
  <!-- The honest card: what it is, how big, where to get it. -->
  <div class="mx-auto flex max-w-xl flex-col items-center gap-3 rounded-lg border border-line bg-panel p-8 text-center">
    {#if family === 'other'}
      <Paperclip size={22} class="text-accent" />
    {:else if family === 'text'}
      <FileText size={22} class="text-accent" />
    {:else if family === 'audio'}
      <FileAudio size={22} class="text-accent" />
    {:else}
      <FileVideo size={22} class="text-accent" />
    {/if}
    <div class="min-w-0 break-words font-sans text-sm text-fg">{title}</div>
    <div class="font-mono text-[11px] text-muted">{contentType ?? 'file'}{humanSize ? ` · ${humanSize}` : ''}</div>
    <a href={url} target="_blank" rel="noreferrer" class="mt-1 rounded-md border border-line bg-surface px-3 py-1.5 font-sans text-sm text-fg transition-colors hover:bg-raised">
      Download
    </a>
  </div>
{/if}
