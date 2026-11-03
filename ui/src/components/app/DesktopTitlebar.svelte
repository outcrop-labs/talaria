<script lang="ts">
  import { themedTitlebar, windowDrag } from '@/lib/desktop-titlebar'
  import DesktopWindowControls from './DesktopWindowControls.svelte'
</script>

{#if themedTitlebar()}
  <div class="flex h-9 shrink-0 items-center border-b border-line bg-sidebar px-1 select-none" use:windowDrag role="presentation">
    <DesktopWindowControls at="left" />
    <div class="min-w-0 flex-1"></div>
    <DesktopWindowControls at="right" />
  </div>
{/if}
