- **Sending while a reply streams no longer interrupts it — anywhere in the
  turn.** The server queue (`queue: true` + the chained follow-up turn) has
  always existed, but a message sent during the FIRST turn's opening window —
  after the request leaves, before the response headers carry the
  conversation id back, which the server can hold for minutes behind a
  restarting agent — took the fresh-send path and started a second stream,
  forking the thread. Those messages are now held locally and flushed the
  moment the id lands (or re-sent as a fresh turn if the first one died
  without producing one); a hold never leaks across a thread switch, and
  attachments stay live while streaming because the queue carries them. Stop
  now means stop, too: the turn freezes what was on screen instead of the
  live-resume poller re-animating the server's copy until it finished anyway.
  Verified: `npx tsc --noEmit`, `npm run typecheck` (0 errors), `npm test`
  (2476 passing), plus the queue paths walked by hand in the running app.
