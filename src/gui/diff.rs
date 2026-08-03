//! How many settings a form has changed.
//!
//! Counting by hand means a `match` per field that silently stops being
//! true the moment someone adds one. Both sides serialize, so compare
//! the serialized forms instead and let the field list come from the
//! type itself.

use serde::Serialize;

/// Number of top-level fields that differ between two values of the same
/// type. Nested structures count as one field: a proxy whose host *and*
/// port changed is one changed setting, which is how the user thinks of
/// it. Values that fail to serialize count as unchanged — a diff is a
/// hint for a button label, never a correctness boundary.
pub fn count_changes<T: Serialize>(before: &T, after: &T) -> usize {
    changed_keys(before, after).len()
}

/// The differing field names. A count alone cannot be debugged: when a
/// form opens already "changed", this says which field failed to survive
/// the trip through the editor.
pub fn changed_keys<T: Serialize>(before: &T, after: &T) -> Vec<String> {
    let (Ok(serde_json::Value::Object(a)), Ok(serde_json::Value::Object(b))) =
        (serde_json::to_value(before), serde_json::to_value(after))
    else {
        return Vec::new();
    };
    a.iter()
        .filter(|(k, v)| b.get(*k).is_none_or(|other| other != *v))
        .map(|(k, _)| k.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Serialize)]
    struct Nested {
        host: String,
        port: u16,
    }

    #[derive(Serialize)]
    struct Form {
        name: String,
        count: u32,
        proxy: Nested,
    }

    fn form(name: &str, count: u32, host: &str, port: u16) -> Form {
        Form {
            name: name.into(),
            count,
            proxy: Nested {
                host: host.into(),
                port,
            },
        }
    }

    #[test]
    fn counts_changed_fields_and_treats_nesting_as_one() {
        let base = form("a", 1, "proxy.lan", 3128);
        assert_eq!(count_changes(&base, &form("a", 1, "proxy.lan", 3128)), 0);
        assert_eq!(count_changes(&base, &form("b", 1, "proxy.lan", 3128)), 1);
        assert_eq!(count_changes(&base, &form("b", 2, "proxy.lan", 3128)), 2);
        // Both halves of the proxy changed — still one setting.
        assert_eq!(count_changes(&base, &form("a", 1, "other.lan", 8080)), 1);
    }
}
