//! ICU-root-ish string compare for ASCII ids (`localeCompare` case fold).
pub fn collating_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let fold = a
        .chars()
        .flat_map(char::to_lowercase)
        .cmp(b.chars().flat_map(char::to_lowercase));
    if fold != Ordering::Equal {
        return fold;
    }
    let case = a
        .chars()
        .map(|c| c.is_uppercase() as u8)
        .cmp(b.chars().map(|c| c.is_uppercase() as u8));
    if case != Ordering::Equal {
        return case;
    }
    a.cmp(b)
}
