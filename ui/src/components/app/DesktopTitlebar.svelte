<script lang="ts">
  import { themedTitlebar, windowDrag } from '@/lib/desktop-titlebar'
  import DesktopWindowControls from './DesktopWindowControls.svelte'

  // The themed titlebar as a band of its own, for screens with no tab bar to
  // carry the window controls: sign-in and the session hold. Renders nothing
  // in a browser or under the OS titlebar.
</script>

{#if themedTitlebar()}
  <div class="flex h-9 shrink-0 items-center border-b border-line bg-sidebar px-1 select-none" use:windowDrag role="presentation">
    <DesktopWindowControls at="left" />
    <div class="min-w-0 flex-1"></div>
    <DesktopWindowControls at="right" />
  </div>
{/if}
