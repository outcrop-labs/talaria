- **Generated API reference: 22 hidden method rows recovered.** The generator's method-span
  scanner ran over raw source, so a comment inside a handler whose text contained backticks
  and an apostrophe (`` `channels.ts`'s ``) read as a phantom string to the scanner and
  de-synced its brace depth — `POST /api/channels/{id}/messages`, `PUT/DELETE
  /api/tasks/{id}`, and 19 more rows never rendered (their body schemas attached to the
  wrong row). Comments are now stripped length-preservingly before scanning, so offsets and
  `// doc:` note positions survive; 343 → 365 method rows across 18 group files.
