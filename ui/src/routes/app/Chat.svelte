<script lang="ts">
  import { navigate } from '@/router'

  // Chat lives inside Comms now (Agents section of the unified sidebar).
  // `t=agent` tells Comms to land on the chat workspace — the first agent's
  // fresh thread (spec §7 composer + §10 patterns) — instead of the default
  // channel selection, so /chat still opens a chat surface.
  $effect(() => {
    void navigate('/comms', { search: { t: 'agent' }, replace: true })
  })
</script>
