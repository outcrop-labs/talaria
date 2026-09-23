- **A mention in a plain channel lands on the channel, not the channel
  index.** The mention notification's href was `/channels` — a page that
  now redirects to the comms root — so the reader arrived nowhere specific
  and went looking for the conversation themselves, which is the work the
  notification was supposed to save. It now carries
  `/comms/channel/{id}` like every other channel-shaped notification, and
  the brief's accessibility check passes it for the members it is for.
  Verified by posting a live @mention through the real route.
