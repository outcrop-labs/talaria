<script lang="ts">
  import { navigate } from '@/router'

  // Channels live inside Comms now (alongside Relays and DMs).
  $effect(() => {
    void navigate('/comms', { replace: true })
  })
</script>
