// What makes two questions the same piece of work, written the same way every time.
//
// A key is composed rather than hashed: every part is length-prefixed, so no part can end in
// a way that makes it look like the start of the next one. `a` + `bc` and `ab` + `c` are two
// keys, not one, and a key stays readable — it carries no path and none of the content.
pub(super) fn compose(prefix: &str, parts: &[&str]) -> String {
    let mut key = String::from(prefix);
    key.push(':');
    for part in parts {
        key.push_str(&part.len().to_string());
        key.push(':');
        key.push_str(part);
    }
    key
}

#[cfg(test)]
mod tests {
    use super::compose;

    #[test]
    fn a_part_cannot_be_mistaken_for_the_start_of_the_next_one() {
        assert_ne!(compose("k", &["a", "bc"]), compose("k", &["ab", "c"]));
    }

    #[test]
    fn the_same_parts_compose_the_same_key() {
        assert_eq!(compose("k", &["a", "bc"]), compose("k", &["a", "bc"]));
        assert_ne!(compose("k", &["a", "b"]), compose("other", &["a", "b"]));
    }
}
