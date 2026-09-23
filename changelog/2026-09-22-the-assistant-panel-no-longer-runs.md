- **The assistant panel no longer runs past the app height when docked.** In
  flow mode (≥1400px) the panel's aside turned `relative` with no height of
  its own, and a block child of the collapse pane is auto-height — so the
  flex-1 transcript stopped scrolling and grew to its full content height,
  pushing the composer below the bottom of the app. The aside now carries the
  nav rail's own methodology (`h-full` inside the `h-full` CollapsePane), so
  header, scrolling transcript, and composer always compose to exactly the
  pane's height. Overlay mode (below 1400px) was already definite
  (`absolute inset-y-0`) and is unchanged.
