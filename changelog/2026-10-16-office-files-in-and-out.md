- **Office files, both directions: a .docx you attach is read, and an artifact
  you want to send becomes one.** Two halves of the same gap. An attached Word
  document reached the agent as a mime type and a byte count — the attachment
  pipeline could only read what `TEXT_MIME` admitted, so the format most of the
  business world writes in arrived as nothing at all, and the model answered a
  message whose subject was missing. And an artifact the agents had written
  could be read in the app but not sent anywhere: no file to attach to a mail,
  hand to a lawyer, or open in Word and edit. The only existing way out was
  `/export/google`, which needs a connected account and a round trip to Drive
  to hand you a file you already had.

  **A new leaf crate, `talaria-office`, with no `talaria-*` dependencies.**
  Nothing in it knows about a request, a database, an artifact or a user — it is
  bytes in, text out and text in, bytes out, which is exactly what lets the
  uploads path and the export route both depend on it without depending on each
  other. `office_kind(mime, filename)` names the document, `extract_text` reads
  one, `markdown_to_docx` and `grid_to_xlsx` write one. OOXML is a zip of XML
  parts, so the whole job is the zip crate plus a pull parser.

  **Reading keeps the structure a reader can act on and drops the rest.** docx:
  one line per `w:p`, tabs and breaks as themselves, table rows as tab-separated
  cells, field codes and tracked deletions excluded (they are text nodes too,
  and they are not words anyone wrote). xlsx: every sheet in **workbook order**,
  `## <sheet name>` then one row per line — shared strings, inline strings,
  numbers and booleans all read, sparse columns padded from the cell reference.
  pptx: `sldIdLst` order, `## Slide N`, every `a:t` run. Formatting, numbering,
  merges and drawings are dropped silently rather than approximated.

  **The mime is a claim and an extension is a guess, and the rule follows from
  that.** A `.docx` posted from a browser routinely arrives as
  `application/octet-stream`, so a generic mime (octet-stream, zip, empty) falls
  back to the filename. The reverse is deliberately *not* symmetric: a mime that
  specifically says `text/plain` beats a `.docx` on the end of the name. Every
  file `TEXT_MIME` already admitted still takes the same lossy-utf8 path it
  always did — the two sets cannot overlap.

  **Every byte reaching the reader came off an upload, so nothing there trusts a
  header.** Each part is refused above 20 MiB on the central directory's
  *declared* size — before a byte is inflated — and again afterwards if that
  header lied. Malformed XML comes back as `Malformed(detail)` rather than an
  unwrap; a column reference past Excel's own XFD is rejected, because padding
  out to it would be the whole attack; per-sheet text is capped because column
  padding is an amplifier. The caller still clips.

  **An export that cannot be opened is not an export**, so the writers emit the
  parts Word and Excel actually look for: `[Content_Types].xml` first with an
  override per part, `_rels/.rels`, the main part's own rels, a style sheet, and
  `docProps/core.xml` carrying the title. Lists are `ListParagraph` with their
  bullet or number written into the text — real numbering definitions are
  several times the file's size and buy a renumbering behaviour nobody asked an
  export for. Cell references run past Z properly, sheet names are sanitised to
  what Excel accepts, numbers that parse stay numbers so the sheet sums, and all
  text is escaped. Same input, same bytes.

  **`GET /api/artifacts/{id}/export?format=docx|xlsx`** is the twin of the
  Google export without Google: the same read gate on both arms, owner arm
  included, but no connected account, no network and nothing recorded — the
  bytes are built in process and answered as an attachment, with the RFC 6266
  filename pair so an accented title saves under its own name. A **sheet**
  becomes a one-sheet workbook from the grid the app renders it from; every
  other text kind becomes a .docx of its markdown. Which file an artifact can
  become is its **kind's** answer, not the caller's: a `file` artifact has no
  document to export (its bytes already download through `/api/uploads/{id}`),
  and a format the kind cannot take is a 400 with a sentence rather than a 200
  with an empty file.

  Verified: 34 tests in the new `talaria-office` crate — every fixture built by
  hand in the source, because a fixture you can read in the diff is worth more
  than one you have to open in Word. `office_kind` by mime with parameters and
  case, by extension under each generic mime, and the cases that must stay
  `None`; docx paragraphs, tabs, breaks and tables; two sheets in workbook order
  with one absolute rel target; two slides whose parts sit in the zip in reverse
  order; entity and character references (a separate parser event — a walk that
  handled only text would silently drop every ampersand and em dash, and that
  test is the regression); a 21 MiB entry refused as `TooLarge` with the fixture
  asserted under 200 KB, so nothing inflated; malformed, truncated, non-zip and
  empty bytes; every part of both writers re-parsed as well-formed XML; column
  names to XFD; sheet-name sanitising (an apostrophe on either end included);
  characters XML cannot carry dropped rather than written; a table nested in a
  cell staying inside that cell; a cut sheet saying `[truncated]`; and both
  round-trip properties. Extraction and the writers run off the executor
  (`spawn_blocking`), so a 20 MiB document never stalls the request thread. Plus 5 in
  `talaria-uploads` (one new: office files admitted by mime or by extension, a
  specific non-office mime beating a `.docx` name, and a ref chip never
  qualifying) and 5 in `talaria-routes-knowledge` (two new: the filename stem
  and the disposition pair). `cargo clippy --all-targets -- -D warnings` clean
  across the four touched crates; `bun run docs:api` regenerated (312 routes →
  313); `bun run check` green; `bun run gate` green. **Live** (`#[ignore]` module
  `office_export_live`, 5/5 against a fresh database): a doc exporting as a
  .docx that reads back, a sheet as an .xlsx whose rows come back in order, the
  three refusals, a stranger getting the read gate's answer, and a .docx uploaded
  through the real `POST /api/uploads` becoming prompt text with no raw zip in
  it. Not yet opened in Word or Excel themselves — the fixtures and the
  round trips are the proof of the packages until somebody double-clicks one.
