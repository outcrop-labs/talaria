- **A GitHub App private key pasted into the admin panel parses — the
  single-line input was stripping its line breaks.** The Key field was a
  password input, and the HTML value sanitizer deletes LF/CR from anything
  pasted into one, so a real `.pem` arrived (and was stored,
  envelope-encrypted) as `-----BEGIN RSA PRIVATE KEY-----base64…` on one
  line — `github app key parse: PKCS#1 ASN.1 error: PEM error: PEM type
  label invalid`, every time, with nothing visible wrong on the user's side.
  Two fixes: the field is now a textarea (multi-line paste survives
  verbatim), and the signer repairs a newline-stripped PEM at parse time —
  the stored shape is unambiguous, so installs already holding a mangled
  key start working without re-pasting. Verified: the stripped form of both
  PEM labels (GitHub's PKCS#1 and PKCS#8) signs the pinned JWT bytes
  (`api/tests/github_pem_repair.rs`), reproducing the exact reported error
  before the fix.

### Changed
