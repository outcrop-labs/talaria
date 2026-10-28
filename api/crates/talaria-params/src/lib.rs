// Path params. Binding {id} straight into `$N::uuid` makes a non-uuid id an
// uncaught Postgres error, which the route then reports as a 500. That is a
// client address, not a server fault — agents pass ticket refs, board names,
// and seqs, and a granted caller must not see Internal Server Error for one.
// Some(gate) is the response to return; the Option shape keeps clippy's
// large-Err lint quiet.
//
// THE ONE SHAPE CHECK. `uuid_shaped` is the question every gate here asks, and
// the one a store asks of an id something else made up before it casts it.
// It is Postgres' own uuid input, so it is never stricter than the cast (a
// braced, upper-case or hyphen-less id still reaches the read) and never
// looser (an id it passes never fails the cast).

use axum::http::StatusCode;
use axum::response::Response;
use talaria_error::house_error;

/// Would Postgres' `::uuid` cast take this string? Exactly its input rule
/// (`string_to_uuid` in src/backend/utils/adt/uuid.c): optional braces
/// around 32 hex digits in either case, with one optional hyphen after any
/// group of four digits but the last, and nothing else — no whitespace, no
/// `urn:uuid:` prefix. `uuid::Uuid::parse_str` is not this: it takes the urn
/// form, which the cast refuses, and refuses the odd hyphenation the cast
/// takes.
pub fn uuid_shaped(s: &str) -> bool {
    let b = s.as_bytes();
    let (braced, mut rest) = match b.split_first() {
        Some((b'{', rest)) => (true, rest),
        _ => (false, b),
    };
    for byte in 0..16 {
        match rest {
            [hi, lo, tail @ ..] if hi.is_ascii_hexdigit() && lo.is_ascii_hexdigit() => rest = tail,
            _ => return false,
        }
        if byte % 2 == 1
            && byte < 15
            && let [b'-', tail @ ..] = rest
        {
            rest = tail;
        }
    }
    matches!((braced, rest), (true, [b'}']) | (false, []))
}

pub fn uuid_gate(module: &str, action: &str, id: &str) -> Option<Response> {
    if uuid_shaped(id) {
        return None;
    }
    tracing::warn!("[{module}] non-uuid id on {action}: {id:?}");
    Some(house_error(StatusCode::BAD_REQUEST, "not a uuid"))
}

/// The same shape, answering "not found" instead: a path id that cannot name
/// a row names nothing, and a 404 says only that. On most routes it is also
/// the answer for a well-formed id that names nothing. Where a route answers
/// that id its own way — a 403 that never says whether the row exists, an
/// empty list, an idempotent ok, "not a managed agent" — a malformed id still
/// gets this 404: no row can have it, so the 404 tells a prober nothing the
/// 403 hides. Call it after the route's auth (and after any body check the
/// route runs before its lookup), so who may ask is answered first. Most
/// routes take this one; the boards, plans, research, teams and workflows
/// routes answered `uuid_gate`'s 400 first and keep it.
pub fn uuid_gate_404(id: &str) -> Option<Response> {
    if uuid_shaped(id) {
        return None;
    }
    Some(house_error(StatusCode::NOT_FOUND, "not found"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every spelling Postgres' uuid input takes, and only those. Each case
    /// was cast for real: `select '<id>'::uuid` on postgres 16.
    #[test]
    fn the_shape_is_postgres_own_uuid_input() {
        for id in [
            "6113b5bc-6935-46e1-a67c-60bd58daf4f0",
            "6113B5BC-6935-46E1-A67C-60BD58DAF4F0",
            "{6113b5bc-6935-46e1-a67c-60bd58daf4f0}",
            "6113b5bc693546e1a67c60bd58daf4f0",
            "{6113b5bc693546e1a67c60bd58daf4f0}",
            "6113-b5bc-6935-46e1-a67c-60bd-58da-f4f0",
            "6113b5bc6935-46e1a67c-60bd58daf4f0",
        ] {
            assert!(uuid_shaped(id), "{id}");
        }
        for id in [
            "",
            "not-a-uuid",
            "dex-dexdept",
            "TALA-41",
            "42",
            "6113b5bc-6935-46e1-a67c-60bd58daf4f",
            "6113b5bc-6935-46e1-a67c-60bd58daf4f0a",
            "6113b5bc-6935-46e1-a67c-60bd58daf4fg",
            "6113b5bc-6935-46e1-a67c-60bd58daf4f0;drop",
            "{6113b5bc-6935-46e1-a67c-60bd58daf4f0",
            "6113b5bc-6935-46e1-a67c-60bd58daf4f0}",
            "urn:uuid:6113b5bc-6935-46e1-a67c-60bd58daf4f0",
            " 6113b5bc-6935-46e1-a67c-60bd58daf4f0",
            "6113b5bc-6935-46e1-a67c-60bd58daf4f0 ",
            "6113b5b-c6935-46e1-a67c-60bd58daf4f0",
            "6113b5bc--6935-46e1-a67c-60bd58daf4f0",
            "-6113b5bc-6935-46e1-a67c-60bd58daf4f0",
            "6113b5bc-6935-46e1-a67c-60bd58daf4f0-",
            "6113b5bc_6935_46e1_a67c_60bd58daf4f0",
            "６113b5bc-6935-46e1-a67c-60bd58daf4f0",
        ] {
            assert!(!uuid_shaped(id), "{id}");
        }
    }

    #[test]
    fn the_gates_pass_what_casts_and_answer_the_rest() {
        let id = "6113b5bc-6935-46e1-a67c-60bd58daf4f0";
        assert!(uuid_gate("t", "read", id).is_none());
        assert!(uuid_gate_404(id).is_none());
        for bad in [
            "dex-dexdept",
            "urn:uuid:6113b5bc-6935-46e1-a67c-60bd58daf4f0",
        ] {
            assert_eq!(
                uuid_gate("t", "read", bad).map(|r| r.status()),
                Some(StatusCode::BAD_REQUEST)
            );
            assert_eq!(
                uuid_gate_404(bad).map(|r| r.status()),
                Some(StatusCode::NOT_FOUND)
            );
        }
    }
}
