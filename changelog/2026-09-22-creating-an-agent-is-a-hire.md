- **Creating an agent is a hire that runs in the background, not a request the modal has
  to babysit**: "Create agent" now enqueues a durable `agent-hire` run (create the def,
  write starter skills, render the fleet, boot the container, wait out the healthcheck)
  and closes immediately. The roster shows the hire working through its phases — the
  run's own sentences (`rendering the fleet config`, `starting the container`), no fake
  percentage — and the finished agent's tile materializes over the strip without a
  refresh; a failure shows its sentence for ten minutes. A boot that runs to minutes on
  a cold pull used to live inside one POST: a modal that couldn't close, proxies timing
  out, and agents visible only after a reload nobody knew they needed. The one error the
  open modal still owns is a taken handle (409, fixable in place). Role templates moved
  onto the first step of the flow — "or start from a role" fills the review form and
  jumps to it, so a fresh install's entry path no longer hides behind a form that only
  exists after you describe an agent first.
