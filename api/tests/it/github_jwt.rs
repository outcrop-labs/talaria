// GitHub app JWT vectors, pinned to a frozen fixture. The fixture was
// produced by the pre-port TS signer and is ground truth now: nothing
// regenerates it, and this suite proves the Rust api signs the SAME bytes —
// header and payload literals, the iat-60/exp+540 offsets, base64url
// unpadded, and the PKCS#1 v1.5 SHA-256 signature itself. If this fails
// after a change here, the signer has drifted from the pinned recipe: fix
// the code, not the fixture.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use talaria_api::github::app_jwt_at;

const FIXTURE: &str = include_str!("fixtures/github-jwt.json");

#[derive(serde::Deserialize)]
struct Fixture {
    #[serde(rename = "privateKeyPem")]
    private_key_pem: String,
    cases: Vec<Case>,
}

#[derive(serde::Deserialize)]
struct Case {
    #[allow(dead_code)]
    name: String,
    #[serde(rename = "appId")]
    app_id: String,
    #[serde(rename = "nowSecs")]
    now_secs: i64,
    jwt: String,
}

#[test]
fn rust_signs_the_pinned_bytes() {
    let f: Fixture = serde_json::from_str(FIXTURE).expect("fixture parses");
    assert!(!f.cases.is_empty());
    for case in &f.cases {
        let jwt = app_jwt_at(&case.app_id, &f.private_key_pem, case.now_secs)
            .expect("fixture key parses");
        assert_eq!(jwt, case.jwt, "case {}", case.name);
    }
}

/// The label GitHub actually ships. App keys download from GitHub as
/// traditional `-----BEGIN RSA PRIVATE KEY-----` (SEC1) PEMs; the fixture is
/// PKCS#8, so a parser that reads only PKCS#8 passes this suite and fails
/// every real key at runtime. This companion fixture is the SAME key in the
/// traditional label (converted once, frozen with the same law as the JSON),
/// and it must sign the SAME bytes — only the PEM label differs.
#[test]
fn the_label_github_ships_signs_the_same_bytes() {
    let f: Fixture = serde_json::from_str(FIXTURE).expect("fixture parses");
    let case = &f.cases[0];
    let traditional = include_str!("fixtures/github-jwt-traditional.pem");
    let jwt =
        app_jwt_at(&case.app_id, traditional, case.now_secs).expect("the traditional label parses");
    assert_eq!(
        jwt, case.jwt,
        "same key, same bytes — only the PEM label differs"
    );
}

#[test]
fn header_and_payload_are_the_pinned_literals() {
    let f: Fixture = serde_json::from_str(FIXTURE).expect("fixture parses");
    let case = &f.cases[0];
    let mut parts = case.jwt.split('.');
    let (h, p, sig) = (
        parts.next().unwrap(),
        parts.next().unwrap(),
        parts.next().unwrap(),
    );
    assert_eq!(
        h,
        URL_SAFE_NO_PAD.encode(br#"{"alg":"RS256","typ":"JWT"}"#),
        "header is JSON.stringify's key order, no spaces"
    );
    assert_eq!(
        p,
        URL_SAFE_NO_PAD.encode(format!(
            r#"{{"iat":{},"exp":{},"iss":"{}"}}"#,
            case.now_secs - 60,
            case.now_secs + 9 * 60,
            case.app_id
        )),
        "payload: iat backdated 60s, exp +9m, iss verbatim"
    );
    // The signature segment is 256 bytes of RS256 for a 2048-bit key.
    assert_eq!(URL_SAFE_NO_PAD.decode(sig).unwrap().len(), 256);
}
