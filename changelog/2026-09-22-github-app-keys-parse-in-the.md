- **GitHub App keys parse in the label GitHub actually ships** — App keys
  download as traditional `-----BEGIN RSA PRIVATE KEY-----` (SEC1) PEMs, and
  the signer read only PKCS#8, so every real key died at install time with
  `PKCS#8 ASN.1 error: PEM type label invalid` while the frozen fixture (a
  PKCS#8 key) kept the suite green. The parser now takes PKCS#8 first, then
  the SEC1 label — the same key either way, and a companion fixture pins the
  traditional label to the same signed bytes.
