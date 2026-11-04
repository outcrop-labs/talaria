- **Changed: consecutive chat messages from one sender now read as one block, as in Slack. The first message shows the avatar, name and time. Later messages are continuation lines, and hovering one shows its own time in the avatar column.** This applies in Comms channels, DMs and ticket discussions, the thread panel, agent chat, and work, research and plan conversations. A new block starts when the sender changes, when more than five minutes pass between messages, at a new day, or at a scheduled agent run. In the thread panel, the root message always stands alone. The rules are one pure function (`ui/src/lib/message-groups.ts`) that every list calls. A streaming message is grouped the same way it will be when it finishes, so arriving text never moves a header. Every message keeps its own `<time datetime>`, with the full date as its hover title, and a visually hidden author name. Assistive tech therefore hears who sent each line and when, even where the screen shows that only once. There is no API or storage change.

  Verified: 15 new unit tests cover the grouping rules. They include the same sender inside the threshold, a sender change, a gap of exactly the threshold and one millisecond over, a day boundary, a deleted message, thread replies, system notices, a missing time, and a streaming last message. Further tests cover the gutter and full-time formatters. svelte-check reports 0 errors and 0 warnings. The change was exercised in headless Chromium against a fresh worktree stack (app :5309, api :5409), seeded with a channel, a DM and an agent conversation. Each had runs from two people and an agent, with a gap over the threshold. Checks covered:
  - grouped blocks, and the time shown on hover in the gutter;
  - reactions, including one added live on a continuation line;
  - a thread opened from a block, with grouped replies under the root;
  - a live edit of a continuation line;
  - an attachment and the `(edited)` mark inside a block;
  - two live sends: the first started a new block after the gap, and the second joined it;
  - deleting a block's first message, which moved the header to the next line;
  - the `interrupted` failed state, and an agent's tool-only turn inside a block.

  Both themes were checked at 1280 px and 760 px, with no horizontal scroll. Playwright's accessibility snapshot showed a time element and an author for every line.
