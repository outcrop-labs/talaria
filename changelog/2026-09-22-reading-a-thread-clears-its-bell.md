- **Reading a thread clears its bell rows.** Opening an agent chat, plan, or
  research discussion advanced the read cursor — the unread pill went away —
  but the notification rows pointing at the thread never learned, so the bell
  kept counting a thread you were visibly reading, and the only way to clear
  it was clicking the notification itself. The read route now sweeps: when
  the advanced cursor covers the thread's latest turn, the bell rows pointing
  at that thread (this reader's, at the href the writer filed) mark read
  alongside it. The href is computed by the same function the writer uses to
  file it, so the two ends cannot drift apart, and a read that stops short of
  the latest turn still leaves the bell alone — there is genuinely something
  left to read. The response carries how many rows cleared, so the client
  refetches the bell only when it actually changed; a failed sweep logs and
  still returns the cursor's ok (marking read must not 500 because the bell
  hiccupped).
