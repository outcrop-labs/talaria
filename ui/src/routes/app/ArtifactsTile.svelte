<script lang="ts">
  import { Folder } from '@lucide/svelte'
  import ArtifactRowShell from './ArtifactRowShell.svelte'
  import { KIND_ICON, type RowInteraction } from './artifacts'

  // The grid view's cell — a desktop icon view, not a card. Big mark, name
  // centred underneath, no border and no meta line: the density and the silence
  // ARE the idiom, and the list view is where Kind/Owner/Modified live. Same
  // selection and drag grammar as ArtifactsRow, so switching views never
  // changes what a click does.
  //
  // The frame is ArtifactRowShell's, and the props are the browser's one
  // RowInteraction — so this file is exactly the GRID view: the column of the
  // cell, the mark, and the name under it. The corner the checkbox sits in
  // comes back in as checkboxClass.
  let { row, ...interaction }: RowInteraction = $props()

  const Icon = $derived(row.kind ? KIND_ICON[row.kind] : Folder)
  // An icon view is big enough to be its own thumbnail: uploaded images show
  // themselves rather than a generic paperclip.
  const thumb = $derived(
    row.artifact?.storageRef && row.artifact.contentType?.startsWith('image/') ? `/api/uploads/${row.artifact.storageRef}` : null,
  )
</script>

<ArtifactRowShell
  {row}
  {...interaction}
  title={`${row.name} · ${row.kindLabel}`}
  buttonClass="flex w-full flex-col items-center gap-1.5 rounded-lg px-1 py-2.5"
  checkboxClass="absolute left-1.5 top-1.5 h-3.5 w-3.5"
>
  <span class="grid h-12 w-12 shrink-0 place-items-center">
    {#if thumb}
      <img src={thumb} alt="" class="max-h-12 max-w-12 rounded border border-line-subtle object-contain" />
    {:else if row.icon}
      <span class="text-[34px] leading-none">{row.icon}</span>
    {:else}
      <Icon size={38} strokeWidth={1.25} class={row.type === 'folder' ? 'text-accent' : 'text-muted'} />
    {/if}
  </span>
  <!-- Two lines, then an ellipsis — a filename is the one thing an icon view
       must never hide, and one line hides most of them. -->
  <span class="line-clamp-2 w-full break-words px-0.5 text-center font-sans text-[11px] leading-snug text-fg">
    {row.name}
  </span>
</ArtifactRowShell>