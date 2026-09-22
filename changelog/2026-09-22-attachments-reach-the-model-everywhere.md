- **Attachments reach the model everywhere.** Two asymmetries closed: channel
  replies now hand image attachments to agents as data-URL image blocks
  (1:1 chat already did — group channels were text-only), and TEXTUAL file
  uploads (markdown, csv, json, code, ...) now contribute their contents to
  the prompt in both paths — send, queued-turn history rebuilds, and channel
  transcripts — clipped like ref chips. File bytes re-read only for the
  recent transcript tail; images capped per reply. Verified live with a real
  agent: quotes a token from an attached file in a DM and in a channel, and
  perceives an attached image identically in both paths.
