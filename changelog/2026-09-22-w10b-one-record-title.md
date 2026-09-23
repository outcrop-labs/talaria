- **W10b — one record title.** `components/ui/RecordTitle.svelte` replaces the
  emoji-trigger + editable-title cluster in the artifact, KB doc and KB space
  editors (−51 lines). Seven differences became props/snippets (`iconFallback`,
  `placeholder`, `class`, `meta`, and each editor's own commit/cancel condition);
  the component writes nothing itself, and read mode still shows the SAVED record
  so a failed save cannot surface unsaved text.
