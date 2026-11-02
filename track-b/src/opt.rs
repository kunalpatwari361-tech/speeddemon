//! Allocation-free phrase verification for Track B.
//!
//! The index already narrows a multi-word phrase to documents containing every adjacent
//! pair. This final check verifies adjacency without materializing a Vec of document tokens
//! for every candidate document.

#[inline]
fn document_tokens(text: &[u8]) -> impl Iterator<Item = &[u8]> + Clone {
    text.split(|c| !c.is_ascii_alphanumeric()).filter(|word| !word.is_empty())
}

#[inline]
fn phrase_words(text: &[u8]) -> impl Iterator<Item = &[u8]> + Clone {
    text.split(|c| *c == b' ').filter(|word| !word.is_empty())
}

/// Match consecutive ASCII-alphanumeric tokens, case-insensitively, without heap allocation.
#[inline]
pub fn phrase_matches(document: &[u8], phrase: &[u8]) -> bool {
    let mut expected = phrase_words(phrase);
    let Some(first) = expected.next() else { return false; };
    let mut actual = document_tokens(document);

    while let Some(word) = actual.next() {
        if !word.eq_ignore_ascii_case(first) { continue; }
        let mut candidate = actual.clone();
        let mut rest = expected.clone();
        let mut matched = true;
        while let Some(want) = rest.next() {
            match candidate.next() {
                Some(got) if got.eq_ignore_ascii_case(want) => {},
                _ => { matched = false; break; },
            }
        }
        if matched { return true; }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::phrase_matches;
    #[test]
    fn matches_consecutive_tokens_case_insensitively() {
        assert!(phrase_matches(b"A fast brown FOX jumps", b"brown fox"));
    }
    #[test]
    fn rejects_non_adjacent_tokens() {
        assert!(!phrase_matches(b"A fast very brown fox", b"fast brown"));
    }
    #[test]
    fn respects_token_boundaries() {
        assert!(!phrase_matches(b"speeding demon", b"speed demon"));
    }
    #[test]
    fn matches_single_word_and_punctuation() {
        assert!(phrase_matches(b"hello, WORLD!", b"world"));
        assert!(phrase_matches(b"hello, world!", b"hello world"));
    }
}