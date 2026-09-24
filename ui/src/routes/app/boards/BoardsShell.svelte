<script lang="ts">
  import type { Snippet } from 'svelte'
  import CreateBoardModal from '@/components/board/CreateBoardModal.svelte'
  import BoardsSublist from '@/components/app/BoardsSublist.svelte'
  import { navigate, route } from '@/router'

  // THE BOARDS SIDEBAR — the boards view's own picker, the piece of shell
  // chrome that left with the nav rail (TALA-84). Same pane the expanded rail
  // was (208px, sidebar ground, right border), with the BoardsSublist living
  // inside it — groups, drag-to-team, archived, footer actions all unchanged,
  // only their address changed. It is a LAYOUT at the '/boards' level
  // (router.ts), so the board, the index, and the ticket overlay all get it
  // while every other view keeps the whole width.
  //
  // The create modal rides here because its trigger (the sublist footer's
  // "New board") is inside the pane; the rail used to own it for the same
  // reason. `creating` is this shell's, the footer's button only flips it.
  let { children }: { children: Snippet } = $props()

  let creating = $state(false)
</script>

<div class="flex h-full min-h-0 min-w-0">
  <aside class="flex w-[208px] shrink-0 flex-col border-r border-line bg-sidebar" aria-label="Boards">
    <div class="flex h-9 shrink-0 items-center px-3 pt-1">
      <div class="flex h-6 items-center px-2 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">
        Boards
      </div>
    </div>
    <div class="flex min-h-0 flex-1 flex-col overflow-y-auto px-2 pb-3">
      <BoardsSublist activePath={route.pathname} onNew={() => (creating = true)} onTeams={() => void navigate('/teams')} />
    </div>
  </aside>
  <div class="min-h-0 min-w-0 flex-1 overflow-hidden">
    {@render children()}
  </div>
  <CreateBoardModal open={creating} onClose={() => (creating = false)} />
</div>
