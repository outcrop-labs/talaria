// The slug an install takes, and the ones it refuses before anything is
// cloned. Install refuses the client router's config keys as slugs
// (talaria_users::RESERVED_APP_SLUGS): /x/<slug> is a path the router
// matches, and those words are its own.

use talaria_apps::install_slug;
use talaria_users::RESERVED_APP_SLUGS;

#[test]
fn an_install_takes_the_repo_name_or_the_override() {
    let ok = |url: &str, slug: Option<&str>| install_slug(url, slug).expect(url);
    assert_eq!(ok("https://github.com/acme/crm", None), "crm");
    assert_eq!(ok("https://github.com/acme/CRM.git", None), "crm");
    assert_eq!(ok("https://github.com/acme/talaria-app-wiki", None), "wiki");
    assert_eq!(
        ok("https://github.com/acme/hooks", Some("team-hooks")),
        "team-hooks"
    );
}

#[test]
fn an_install_refuses_a_slug_the_router_reserves() {
    for word in RESERVED_APP_SLUGS {
        // From the repo's name, from a talaria-app- repo, and as an override.
        for (url, slug) in [
            (format!("https://github.com/acme/{word}"), None),
            (
                format!("https://github.com/acme/talaria-app-{word}.git"),
                None,
            ),
            ("https://github.com/acme/crm".to_string(), Some(*word)),
            (
                "https://github.com/acme/crm".to_string(),
                Some(&*word.to_uppercase()),
            ),
        ] {
            let err = install_slug(&url, slug).expect_err(&url);
            assert!(err.contains("is reserved"), "{url} {slug:?}: {err}");
        }
    }
}

#[test]
fn an_install_refuses_a_slug_that_breaks_the_rule() {
    let err = install_slug("https://github.com/acme/my_app", None).expect_err("underscore");
    assert!(err.contains("not a usable app slug"), "{err}");
    assert!(install_slug("https://github.com/acme/", None).is_err());
}
