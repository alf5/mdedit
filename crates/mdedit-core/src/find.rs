//! Find and replace over the markdown text.

use std::ops::Range;

use crate::buffer::Edit;

fn eq(a: char, b: char, case: bool) -> bool {
    a == b || (!case && a.to_lowercase().eq(b.to_lowercase()))
}

/// Every non-overlapping match of `query`, in order. `case` makes it
/// case-sensitive.
pub fn find_all(text: &str, query: &str, case: bool) -> Vec<Range<usize>> {
    if query.is_empty() {
        return Vec::new();
    }
    if case {
        return text
            .match_indices(query)
            .map(|(i, m)| i..i + m.len())
            .collect();
    }
    let q: Vec<char> = query.chars().collect();
    let mut out = Vec::new();
    let mut from = 0;
    let indices: Vec<(usize, char)> = text.char_indices().collect();
    let mut i = 0;
    while i < indices.len() {
        if indices[i].0 < from {
            i += 1;
            continue;
        }
        let mut k = 0;
        while k < q.len() && i + k < indices.len() && eq(indices[i + k].1, q[k], false) {
            k += 1;
        }
        if k == q.len() {
            let start = indices[i].0;
            let end = indices.get(i + k).map_or(text.len(), |(b, _)| *b);
            out.push(start..end);
            from = end;
            i += k;
        } else {
            i += 1;
        }
    }
    out
}

/// The match to go to from `pos`: the first starting at or after it going
/// forward, the last ending before it going back; wrapping around.
pub fn next_match(matches: &[Range<usize>], pos: usize, forward: bool) -> Option<usize> {
    if matches.is_empty() {
        return None;
    }
    if forward {
        Some(matches.iter().position(|m| m.start >= pos).unwrap_or(0))
    } else {
        Some(
            matches
                .iter()
                .rposition(|m| m.end <= pos)
                .unwrap_or(matches.len() - 1),
        )
    }
}

pub fn replace_all(text: &str, query: &str, case: bool, with: &str) -> Vec<Edit> {
    find_all(text, query, case)
        .into_iter()
        .map(|r| Edit::replace(r, with))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::apply;

    #[test]
    fn finds_with_and_without_case() {
        let t = "Foo foo FOO";
        assert_eq!(find_all(t, "foo", true), vec![4..7]);
        assert_eq!(find_all(t, "foo", false).len(), 3);
        assert_eq!(find_all("Straße", "STRASSE", false).len(), 0);
        assert_eq!(find_all("ÉCOLE école", "école", false), vec![0..6, 7..13]);
        assert!(find_all(t, "", false).is_empty());
        assert_eq!(find_all("aaa", "aa", true), vec![0..2]);
    }

    #[test]
    fn steps_through_matches_and_wraps() {
        let m = vec![0..1, 5..6, 9..10];
        assert_eq!(next_match(&m, 2, true), Some(1));
        assert_eq!(next_match(&m, 10, true), Some(0));
        assert_eq!(next_match(&m, 5, false), Some(0));
        assert_eq!(next_match(&m, 0, false), Some(2));
    }

    #[test]
    fn replaces_everything() {
        let mut t = "a b a".to_string();
        let edits = replace_all(&t, "A", false, "xy");
        apply(&mut t, &edits);
        assert_eq!(t, "xy b xy");
    }
}
