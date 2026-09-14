// A PEM whose newlines were stripped in transit (the admin form's single-line
// password input strips them on paste — the HTML value sanitizer) must still
// sign: the browser-side mangling is unambiguous and repairable at parse time.
// This is the regression test for the "PEM type label invalid" report.

use talaria_api::github::app_jwt_at;

#[test]
fn a_newline_stripped_pem_still_signs() {
    let f: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/github-jwt.json")).expect("fixture parses");
    let case = &f["cases"][0];
    let strip =
        |whole: &str| -> String { whole.chars().filter(|c| *c != '\n' && *c != '\r').collect() };

    // The traditional label (what GitHub ships), stripped to one line.
    let traditional = include_str!("fixtures/github-jwt-traditional.pem");
    let jwt = app_jwt_at(
        case["appId"].as_str().unwrap(),
        &strip(traditional),
        case["nowSecs"].as_i64().unwrap(),
    )
    .expect("the newline-stripped traditional label parses");
    assert_eq!(jwt, case["jwt"].as_str().unwrap());

    // The PKCS#8 fixture's shape, stripped the same way.
    let pkcs8 = f["privateKeyPem"].as_str().unwrap();
    let jwt = app_jwt_at(
        case["appId"].as_str().unwrap(),
        &strip(pkcs8),
        case["nowSecs"].as_i64().unwrap(),
    )
    .expect("the newline-stripped PKCS#8 label parses");
    assert_eq!(jwt, case["jwt"].as_str().unwrap());
}
