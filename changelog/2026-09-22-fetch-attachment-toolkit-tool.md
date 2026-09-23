- **`fetch_attachment` toolkit tool.** Agents can now READ the files attached
  to tickets and chats: text formats come back inline (clipped at 50k chars),
  images arrive as real MCP image blocks the model can see, and other binary
  formats report honest metadata instead of pretending. `get_ticket` now
  advertises the attachments array. Verified live: fleet render seeds skill +
  mount, MCP serves the tool, text/image/binary/404 behaviors all correct,
  11/11 checks.
