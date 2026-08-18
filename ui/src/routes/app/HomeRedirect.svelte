<script lang="ts">
  import { navigate } from '@/router'

  // `/` is not a view. It bounces to the Inbox's own URL.
  //
  // The Inbox used to live AT the root, which meant it had no address of its
  // own: nothing to link to, nothing to bookmark, and a nav rail entry whose
  // path (`/`) is an ancestor of every route in the app — so it needed a
  // hand-written exemption to stop lighting on every view. Nesting it under
  // Home gives it a real URL and lets one rule decide every active state.
  //
  // REPLACE, not push: the root is housekeeping, and leaving it in history
  // would put a redirect between the person and the back button.
  $effect(() => {
    void navigate('/home/:tab', { params: { tab: 'inbox' }, replace: true })
  })
</script>
