<script lang="ts">
  import { cn } from '@/lib/cn'

  // Three dots that ripple while someone is typing. Each dot rises a
  // pixel and brightens in turn on a 1.2s loop. Under reduced motion nothing
  // moves: the dots stay put and only their opacity breathes.
  let { class: className, label }: { class?: string; label?: string } = $props()
</script>

<span class={cn('inline-flex items-end gap-[2px]', className)} role={label ? 'img' : undefined} aria-label={label} aria-hidden={label ? undefined : true}>
  <span class="dot"></span><span class="dot"></span><span class="dot"></span>
</span>

<style>
  .dot {
    width: 3px;
    height: 3px;
    border-radius: 9999px;
    background: currentColor;
    animation: typing-dot 1.2s cubic-bezier(0.45, 0, 0.55, 1) infinite;
  }
  .dot:nth-child(2) {
    animation-delay: 0.15s;
  }
  .dot:nth-child(3) {
    animation-delay: 0.3s;
  }
  @keyframes typing-dot {
    0%,
    60%,
    100% {
      opacity: 0.35;
      transform: translateY(0);
    }
    30% {
      opacity: 1;
      transform: translateY(-2px);
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .dot {
      animation-name: typing-dot-still;
    }
    @keyframes typing-dot-still {
      0%,
      60%,
      100% {
        opacity: 0.35;
      }
      30% {
        opacity: 1;
      }
    }
  }
</style>
