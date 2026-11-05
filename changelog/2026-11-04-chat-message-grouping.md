- **Changed: in the web app, consecutive chat messages from one sender now read as one block, as in Slack. The first message shows the avatar, name and time. Later messages are continuation lines, and hovering one shows its own time in the avatar column.** This applies in Comms channels, DMs and ticket discussions, the thread panel, agent chat, and work, research and plan conversations. It is web only: the native mobile app's rows are unchanged. A new block starts when:
  - the sender changes;
  - more than five minutes pass between messages;
  - a message is stamped earlier than the one above it, so a header cannot move when a client-drawn row takes the server's time;
  - a new day starts;
  - an agent's scheduled run posts.

  In the thread panel, the root message always stands alone. The rules are one pure function (`ui/src/lib/message-groups.ts`). Each list maps its rows to that function's inputs through a small tested helper. A streaming message is grouped the same way it will be when it finishes, so arriving text never moves a header. Every message keeps its own `<time datetime>`, with the full date as its hover title. A continuation line shows a short "3:05" but gives a screen reader the header's "03:05 PM", plus a visually hidden author name, so assistive tech hears who sent each line and when even where the screen shows that only once. There is no API or storage change.

  Verified:
  - **Unit tests.**
    - The grouping rules: the threshold pinned at exactly five minutes; the same sender inside it; a sender change; a gap of exactly the threshold and one millisecond over; a negative gap and a clock-skew pair; a day boundary; a deleted message; a fresh row; a missing time.
    - Each surface's helper, through the real grouping function: the thread root standing alone; a scheduled run starting fresh; a row the client drew joining the viewer's own run; two people on a shared plan; status ignored while a turn streams and once it settles.
    - Seven deliberate mutations (thread rule `i === 0`, the scheduled-run rule removed, status read in either mapper, a negative gap grouping, a 10-minute threshold, no viewer fallback) each fail a test.
  - **Type check.** svelte-check reports 0 errors and 0 warnings.
  - **Browser run.** Headless Chromium, against a fresh worktree stack seeded with a channel, a DM and an agent conversation. Each had runs from two people and an agent, with a gap over the threshold. Checks covered:
    - grouped blocks, and the time shown on hover in the gutter;
    - reactions, including one added live on a continuation line;
    - a thread opened from a block, with grouped replies under the root;
    - a live edit of a continuation line;
    - an attachment and the `(edited)` mark inside a block;
    - two live sends: the first started a new block after the gap, and the second joined it;
    - deleting a block's first message, which moved the header to the next line;
    - the `interrupted` failed state, and an agent's tool-only turn inside a block.
  - **Themes and widths.** Both themes, at 1280 px and 760 px, with no horizontal scroll.
  - **Accessibility.** Playwright's accessibility snapshot of the channel, the DM and the agent chat lists a time with AM/PM for every line, and an author for every continuation line.
