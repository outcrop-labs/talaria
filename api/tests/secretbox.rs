// Cross-language secretbox vectors. The committed fixture is produced by the
// TS side (scripts/gen-secretbox-vectors.mjs, which runs secretbox.ts's own
// deriveKek); this suite proves the Rust api opens what TS sealed and seals
// what TS would — byte-for-byte, both directions, no runtime coupling between
// the languages. If this file fails after a secretbox change on EITHER side,
// the two have drifted: fix the code, then regenerate with `bun run api:vectors`.

use base64::Engine;
use std::collections::HashMap;
use talaria_api::secretbox::{IV_LEN, SecretBox, derive_kek};

const FIXTURE: &str = include_str!("fixtures/secretbox.json");

#[derive(serde::Deserialize)]
struct Fixture {
    root: String,
    kek_hex: String,
    deks: Vec<DekVec>,
    open: Vec<OpenVec>,
    seal: Vec<SealVec>,
}

#[derive(serde::Deserialize)]
struct DekVec {
    version: u32,
    dek_hex: String,
    #[serde(default)]
    active: bool,
    wrap_iv_hex: String,
    wrapped: String,
}

#[derive(serde::Deserialize)]
struct OpenVec {
    #[allow(dead_code)]
    name: String,
    token: String,
    /// None = the vector must FAIL to open (tampered).
    plaintext: Option<String>,
}

#[derive(serde::Deserialize)]
struct SealVec {
    #[allow(dead_code)]
    name: String,
    #[serde(default)]
    wrap: bool,
    version: u32,
    iv_hex: String,
    #[serde(default)]
    plaintext: String,
    token: String,
}

fn unhex(s: &str) -> Vec<u8> {
    assert!(
        s.len().is_multiple_of(2),
        "hex literal has a half byte: {s}"
    );
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("hex literal has a non-hex char"))
        .collect()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn fixture() -> Fixture {
    serde_json::from_str(FIXTURE).expect("committed fixture parses")
}

/// The box exactly as the fixture describes it: every DEK version loaded,
/// active flagged by the fixture.
fn box_for(f: &Fixture) -> (SecretBox, [u8; 32]) {
    let kek = derive_kek(&f.root);
    let deks: HashMap<u32, [u8; 32]> = f
        .deks
        .iter()
        .map(|d| {
            (
                d.version,
                unhex(&d.dek_hex).try_into().expect("DEK is 32 bytes"),
            )
        })
        .collect();
    let active = f
        .deks
        .iter()
        .find(|d| d.active)
        .map(|d| d.version)
        .expect("fixture marks an active DEK");
    (SecretBox::from_parts(kek, deks, Some(active)), kek)
}

fn iv(hex_str: &str) -> [u8; IV_LEN] {
    unhex(hex_str).try_into().expect("fixture IV is 12 bytes")
}

#[test]
fn kek_derivation_matches_ts_byte_for_byte() {
    // THE scrypt pin: same salt, same N/r/p, same 32 bytes. Every other
    // assertion in this file is downstream of this one.
    let f = fixture();
    assert_eq!(hex(&derive_kek(&f.root)), f.kek_hex);
}

#[test]
fn dek_wraps_reproduce_ts_bytes_both_directions() {
    let f = fixture();
    let (sb, kek) = box_for(&f);
    for d in &f.deks {
        let dek: [u8; 32] = unhex(&d.dek_hex).try_into().unwrap();
        let mine = SecretBox::wrap_dek_with_iv(&kek, &dek, &iv(&d.wrap_iv_hex)).unwrap();
        assert_eq!(mine, d.wrapped, "v{} wrap drifted from TS", d.version);

        // And the asymmetry, from the open side: opening the wrap yields the
        // STANDARD PADDED base64 STRING of the key, not the raw bytes
        // (secretbox.ts wrapDek). An implementation that wrapped raw bytes
        // fails here.
        let opened = sb.open(&d.wrapped).unwrap();
        assert_eq!(
            opened,
            base64::engine::general_purpose::STANDARD.encode(dek),
            "v{} wrap plaintext",
            d.version
        );
    }
}

#[test]
fn opens_every_fixture_case_including_tampered() {
    let f = fixture();
    let (sb, _) = box_for(&f);
    for case in &f.open {
        match &case.plaintext {
            Some(expected) => {
                let got = sb
                    .open(&case.token)
                    .unwrap_or_else(|e| panic!("case {} should open: {e}", case.name));
                assert_eq!(got, *expected, "case {}", case.name);
            }
            None => assert!(
                sb.open(&case.token).is_err(),
                "case {} must fail to open",
                case.name
            ),
        }
    }
}

#[test]
fn seals_byte_identical_tokens() {
    let f = fixture();
    let (sb, kek) = box_for(&f);
    for case in &f.seal {
        let mine = if case.wrap {
            let dek: [u8; 32] = unhex(
                &f.deks
                    .iter()
                    .find(|d| d.version == case.version)
                    .unwrap()
                    .dek_hex,
            )
            .try_into()
            .unwrap();
            SecretBox::wrap_dek_with_iv(&kek, &dek, &iv(&case.iv_hex))
        } else {
            sb.seal_with_iv(case.version, &iv(&case.iv_hex), &case.plaintext)
        };
        assert_eq!(mine.unwrap(), case.token, "case {}", case.name);
    }
}
