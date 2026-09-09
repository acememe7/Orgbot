//! Comparing what two pieces of writing are *about*.
//!
//! Several places need the same question answered — is this review finding the
//! one from last cycle, is this new task the one that already exists — and all
//! of them face the same problem: a model rarely writes the same thing twice
//! the same way. Comparing strings exactly finds nothing; comparing them with
//! a model costs a turn.
//!
//! So this compares content words. It catches reordering, filler and padding,
//! which is most of how phrasing drifts. It does not catch synonyms or
//! morphology — "absent" and "missing" read as different subjects — and that
//! limit is deliberate: guessing at those would merge things that are
//! genuinely distinct, which is worse than missing a match.

/// Words that carry no subject on their own.
const NOISE: &[&str] = &[
    "the", "a", "an", "is", "are", "was", "were", "be", "been", "to", "of", "in", "on", "for",
    "and", "or", "not", "no", "it", "its", "this", "that", "with", "without", "should", "must",
    "may", "can", "does", "do", "has", "have", "there", "here", "but", "add", "make", "please",
    "need", "needs", "needed", "task", "work",
];

/// The words of a phrase that carry meaning, lowercased.
pub fn content_words(text: &str) -> Vec<String> {
    text.to_lowercase()
        // Hyphens and underscores stay inside words, so an identifier like
        // `B3-C` survives as one token instead of shedding the letter that
        // distinguishes it from `B3-E`.
        .split(|c: char| !(c.is_alphanumeric() || c == '-' || c == '_'))
        .map(|word| word.trim_matches(['-', '_']))
        // A digit is often the only thing distinguishing two pieces of work —
        // "step 0" from "step 1", "B3-C" from "B3-E" — so anything carrying a
        // number is kept however short it is.
        .filter(|word| {
            (word.len() > 2 || word.chars().any(char::is_numeric))
                && !NOISE.contains(word)
        })
        .map(str::to_string)
        .collect()
}

/// How much two phrases are about the same thing, from 0 to 1.
///
/// Jaccard overlap of content words. Two phrases with no content words at all
/// fall back to comparing them outright, so very short text is not treated as
/// matching everything.
pub fn similarity(a: &str, b: &str) -> f32 {
    let (mut left, mut right) = (content_words(a), content_words(b));
    left.sort();
    left.dedup();
    right.sort();
    right.dedup();
    if left.is_empty() || right.is_empty() {
        return f32::from(u8::from(a.trim().eq_ignore_ascii_case(b.trim())));
    }
    let shared = left.iter().filter(|word| right.contains(word)).count();
    let union = left.len() + right.len() - shared;
    shared as f32 / union as f32
}

/// How alike two pieces of work must be to be treated as the same one.
///
/// Higher than the threshold used for review findings: creating a task is
/// cheap to get wrong in one direction and expensive in the other. Refusing a
/// genuinely new task because it shares vocabulary with an old one blocks
/// work; letting a near-duplicate through only adds a record someone can
/// merge later.
pub const DUPLICATE_THRESHOLD: f32 = 0.6;

/// Whether two pieces of work look like the same thing.
pub fn looks_like_the_same_work(a: &str, b: &str) -> bool {
    similarity(a, b) >= DUPLICATE_THRESHOLD
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_text_is_identical() {
        assert_eq!(similarity("rebuild the container", "rebuild the container"), 1.0);
    }

    #[test]
    fn word_order_and_filler_do_not_matter() {
        assert!(looks_like_the_same_work(
            "restore the missing B3-C source",
            "the B3-C source, restore it",
        ));
    }

    #[test]
    fn different_subjects_are_not_the_same_work() {
        assert!(!looks_like_the_same_work(
            "restore the missing B3-C source",
            "write the release notes",
        ));
    }

    /// The limit, stated rather than hidden: synonyms read as different.
    #[test]
    fn synonyms_are_not_recognised() {
        assert!(!looks_like_the_same_work(
            "the lock is absent",
            "the mutex is missing",
        ));
    }

    /// Boilerplate verbs must not make everything look alike.
    #[test]
    fn common_task_words_carry_no_subject() {
        assert!(!looks_like_the_same_work(
            "add a task to make the parser work",
            "add a task to make the writer work",
        ));
    }

    /// Numbered work is not the same work.
    #[test]
    fn a_number_is_enough_to_tell_two_things_apart() {
        assert!(!looks_like_the_same_work("step 0", "step 1"));
        assert!(!looks_like_the_same_work(
            "restore the B3-C source",
            "restore the B3-E source",
        ));
        assert!(looks_like_the_same_work("step 4", "the step 4"));
    }

    #[test]
    fn empty_text_matches_only_empty_text() {
        assert_eq!(similarity("", ""), 1.0);
        assert_eq!(similarity("", "something"), 0.0);
    }
}
