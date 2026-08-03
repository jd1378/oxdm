//! Custom request-header maps and the case-insensitive rules that hold
//! for every one of them.
//!
//! HTTP field names are case-insensitive (RFC 9110 §5.1), but the maps
//! we carry them in (`IndexMap<String, String>`, insertion-ordered so
//! the user's list keeps its shape) are not. Without these helpers
//! `x-api-key` and `X-API-Key` are two entries that reqwest later folds
//! into one — so the header the user sees listed is not necessarily the
//! header that gets sent, and a per-job override silently fails to
//! override its global twin.
//!
//! Rule: a name matches an existing entry case-insensitively; on a
//! match the value is replaced **in place**, keeping the first
//! spelling and position. Position matters because it is the order the
//! user arranged, and the original spelling is as valid as any other.

use indexmap::IndexMap;

pub type HeaderMap = IndexMap<String, String>;

/// Do these two field names denote the same header?
pub fn header_name_eq(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
}

/// Set `name` to `value`, replacing any existing entry that names the
/// same header in different case.
pub fn upsert_header(map: &mut HeaderMap, name: &str, value: String) {
    match map.keys().position(|k| header_name_eq(k, name)) {
        Some(idx) => map[idx] = value,
        None => {
            map.insert(name.trim().to_owned(), value);
        }
    }
}

/// Is this header present under any spelling?
pub fn has_header(map: &HeaderMap, name: &str) -> bool {
    map.keys().any(|k| header_name_eq(k, name))
}

/// Collect edited rows into a storable map: names trimmed, nameless
/// rows dropped, and case-duplicates folded onto the first spelling
/// with the last value winning — the same resolution the wire performs,
/// applied where the user can still see the result.
pub fn normalize_headers<I>(pairs: I) -> HeaderMap
where
    I: IntoIterator<Item = (String, String)>,
{
    let mut out = HeaderMap::new();
    for (name, value) in pairs {
        if name.trim().is_empty() {
            continue;
        }
        upsert_header(&mut out, &name, value);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, &str)]) -> HeaderMap {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn upsert_replaces_a_differently_cased_entry_in_place() {
        let mut m = map(&[("Accept", "a"), ("X-Trace", "t")]);
        upsert_header(&mut m, "ACCEPT", "b".to_owned());
        assert_eq!(
            m.keys().collect::<Vec<_>>(),
            vec!["Accept", "X-Trace"],
            "the first spelling and its position survive"
        );
        assert_eq!(m["Accept"], "b");
    }

    #[test]
    fn upsert_appends_a_genuinely_new_name() {
        let mut m = map(&[("Accept", "a")]);
        upsert_header(&mut m, " X-Api-Key ", "k".to_owned());
        assert_eq!(m.keys().collect::<Vec<_>>(), vec!["Accept", "X-Api-Key"]);
    }

    #[test]
    fn normalize_folds_case_duplicates_and_drops_nameless_rows() {
        let rows = [("X-Api-Key", "one"), ("", "orphan"), ("x-api-key", "two")]
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()));
        let m = normalize_headers(rows);
        assert_eq!(m.len(), 1);
        assert_eq!(m["X-Api-Key"], "two", "the last value wins, like the wire");
    }

    #[test]
    fn has_header_ignores_case() {
        let m = map(&[("Cookie", "a=b")]);
        assert!(has_header(&m, "cookie"));
        assert!(!has_header(&m, "Cookies"));
    }
}
