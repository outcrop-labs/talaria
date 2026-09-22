<script lang="ts">
  import { Building2, Folder, Globe } from '@lucide/svelte'
  import { cn } from '@/lib/cn'
  import { relativeTime } from '@/lib/fleet'
  import ArtifactRowShell from './ArtifactRowShell.svelte'
  import { KIND_ICON, ROW_GRID, type RowInteraction } from './artifacts'

  // One line in the Files browser — a folder or a file, same geometry either
  // way. The grid template lives in artifacts.ts (ROW_GRID) so the header and
  // every line stay locked together as the pane resizes.
  //
  // The whole row is ONE button, and the checkbox is a sibling in its own
  // lane: a click anywhere on the button opens (the interaction we chose),
  // the checkbox is a real control (Checkbox bare — the row around it is the
  // hit context), and BOTH are permanently visible. The checkbox used to
  // float over the kind icon and swap with it on hover — hiding the thing you
  // were about to click was the bug that retired that.
  //
  // The frame is ArtifactRowShell's, and the props are the browser's one
  // RowInteraction — so this file is exactly the LIST view: its column grid,
  // and the four cells inside the button. The checkbox lane's own left offset
  // comes back in as checkboxClass.
  let { row, ...interaction }: RowInteraction = $props()

  const Icon = $derived(row.kind ? KIND_ICON[row.kind] : Folder)
</script>

<ArtifactRowShell
  {row}
  {...interaction}
  buttonClass={cn('grid w-full items-center gap-3 rounded-lg px-2 py-1.5 text-left', ROW_GRID)}
  checkboxClass="absolute left-[15px] top-1/2 h-3.5 w-3.5 -translate-y-1/2"
>
  <!-- Track 1: the checkbox lane. The Checkbox itself is absolutely
       centered over it (a bare input cannot be a grid child of the button
       without nesting interactive elements), so the track reserves the
       space and keeps name/kind/owner/modified aligned with the header. -->
  <span aria-hidden="true"></span>

  <span class="flex min-w-0 items-center gap-2">
    <!-- The kind icon, always visible — selection must never hide what a
         thing IS. -->
    <span class="grid h-5 w-5 shrink-0 place-items-center">
      {#if row.icon}
        <span class="text-[15px] leading-none">{row.icon}</span>
      {:else}
        <Icon size={15} class={row.type === 'folder' ? 'text-accent' : 'text-muted'} />
      {/if}
    </span>
    <span class="min-w-0 truncate font-sans text-sm text-fg">{row.name}</span>
    <!-- Public is the one access state worth interrupting a filename for:
         everything else is between colleagues, this one is the internet. -->
    {#if row.artifact?.visibility === 'public'}
      <Globe size={12} class="shrink-0 text-accent" aria-label="Public on the internet" />
    {/if}
  </span>

  <span class="truncate font-mono text-[11px] tracking-[0.05em] text-muted">{row.kindLabel}</span>
  <span class="flex min-w-0 items-center gap-1.5 font-sans text-[13px] text-muted">
    {#if row.scope === 'workspace'}
      <!-- Ownerless: the organization's file, and the name beside it is
           whichever agent wrote it rather than a person who owns it. -->
      <Building2 size={12} class="shrink-0 opacity-70" aria-label="Owned by the workspace" />
    {/if}
    <span class="truncate">{row.owner}</span>
  </span>
  <span class="truncate font-mono text-[11px] tracking-[0.05em] text-muted">{relativeTime(row.modified)}</span>
</ArtifactRowShell>