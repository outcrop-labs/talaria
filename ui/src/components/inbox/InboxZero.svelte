<script lang="ts">
  import { Inbox } from '@lucide/svelte'
</script>

<section class="grid min-h-[430px] place-items-center border-b border-line text-center">
  <div>
    <span class="mx-auto mb-4 grid h-12 w-12 place-items-center rounded-full border border-success/50 text-success"><Inbox size={20} /></span>
    <h2 class="font-sans text-[32px] font-light tracking-[-0.02em] text-fg">Inbox zero</h2>
    <p class="mt-2 font-sans text-sm text-muted">No decisions are waiting. Your assistant is still available below.</p>
  </div>
</section>
