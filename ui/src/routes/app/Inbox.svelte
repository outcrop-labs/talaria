<script lang="ts">
  import { navigate } from '@/router'

  // `/inbox` is kept as a permanent alias: it is the URL people typed and
  // linked before the Inbox had one of its own, and a dead link is a worse
  // outcome than a redirect that never goes away.
  $effect(() => {
    void navigate('/home/:tab', { params: { tab: 'inbox' }, replace: true })
  })
</script>
