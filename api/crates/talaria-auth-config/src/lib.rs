// Auth configuration from the environment — what the Google login flow needs:
//
//   AUTH_PUBLIC_URL        external origin for OAuth redirect URIs (optional —
//                          falls back to the request origin)
//   AUTH_ALLOWED_DOMAINS   comma-separated email domains allowed Google sign-in
//   AUTH_ALLOWED_EMAILS    comma-separated exact emails allowed Google sign-in
//
// (The Google client credentials and the login toggle live in
// google_client.rs — the Admin UI record with env fallback, not this file.)
//
// Read per request: a config flip must not depend on process boot order.

/// The GOOGLE allow-list gate — applied to Google-resolved identities only. A
/// password account was admitted by an admin (or the claim) when it was
/// created; its login checks the stored hash and nothing else.
///
/// No allow-list configured ⇒ anyone who authenticates is allowed.
#[derive(Default)]
pub struct AuthConfig {
    pub public_url: Option<String>,
    pub allowed_domains: Vec<String>,
    pub allowed_emails: Vec<String>,
}

/// env list: split on commas, trim, lowercase, drop empties.
fn list(var: &str) -> Vec<String> {
    std::env::var(var)
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .map(str::to_lowercase)
        .filter(|s| !s.is_empty())
        .collect()
}

pub fn get_auth_config() -> AuthConfig {
    AuthConfig {
        // one trailing slash falls away; an empty (or unset) value is None.
        public_url: std::env::var("AUTH_PUBLIC_URL")
            .ok()
            .map(|s| s.strip_suffix('/').unwrap_or(&s).to_string())
            .filter(|s| !s.is_empty()),
        allowed_domains: list("AUTH_ALLOWED_DOMAINS"),
        allowed_emails: list("AUTH_ALLOWED_EMAILS"),
    }
}

pub fn is_email_allowed(email: Option<&str>, cfg: &AuthConfig) -> bool {
    // No allow-list configured ⇒ anyone who authenticates is allowed.
    if cfg.allowed_domains.is_empty() && cfg.allowed_emails.is_empty() {
        return true;
    }
    let Some(email) = email else { return false };
    let e = email.to_lowercase();
    if cfg.allowed_emails.iter().any(|a| a == &e) {
        return true;
    }
    // the segment after the FIRST @ — nth(1), not the last.
    let domain = e.split('@').nth(1).unwrap_or_default();
    cfg.allowed_domains.iter().any(|d| d == domain)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(domains: &[&str], emails: &[&str]) -> AuthConfig {
        AuthConfig {
            public_url: None,
            allowed_domains: domains.iter().map(|s| s.to_string()).collect(),
            allowed_emails: emails.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn empty_lists_admit_everyone_but_only_with_an_email() {
        let c = cfg(&[], &[]);
        assert!(is_email_allowed(Some("anyone@anywhere.io"), &c));
        assert!(is_email_allowed(None, &c)); // no lists ⇒ allowed, email or not
    }

    #[test]
    fn lists_match_domain_or_exact_email() {
        let c = cfg(&["getboxie.com"], &["friend@example.org"]);
        assert!(is_email_allowed(Some("jon@getboxie.com"), &c));
        assert!(is_email_allowed(Some("JON@GETBOXIE.COM"), &c)); // case-folded
        assert!(is_email_allowed(Some("friend@example.org"), &c));
        assert!(!is_email_allowed(Some("stranger@example.org"), &c));
        assert!(!is_email_allowed(Some("jon@sub.getboxie.com"), &c)); // exact domain
        assert!(!is_email_allowed(None, &c));
    }
}
