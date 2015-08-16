//! Implements a 3-way merge, using the algorithm described in:
//!
//! Sanjeev Khanna , Keshav Kunal , Benjamin C. Pierce, A formal investigation of Diff3. Proceedings
//! of the 27th international conference on Foundations of software technology and theoretical
//! computer science, December 12-14, 2007, New Delhi, India.

extern crate num;

use std::cmp::Ordering;
use std::iter::FromIterator;
use std::str::CharIndices;

use ::START_MARKER;
use ::END_MARKER;
use ::longest_common_subsequence;
use ::longest_common_subsequence::CommonSubsequence;
use timer::Timer;

/// Represents the states of a 4-state machine representing the traversal through `old` to find
/// stable and unstable chunks: at any given moment, the part of `old` under consideration is either
/// unmatched in both `new` and `other`, matched in only one, or matched in both.
#[derive(Debug)]
enum MatchState {
    NeitherMatch,
    /// Parameters are the offsets into old and new where the match started.
    OnlyNewMatches(usize, usize),
    /// Parameters are the offsets into old and other where the match started.
    OnlyOtherMatches(usize, usize),
    /// Parameters are the offsets into old, new, and other where the match started.
    BothMatch(usize, usize, usize),
}

/// Represents the transitions in the 4-state machine representing the traversal through `old` to
/// find stable and unstable chunks: at any index, either `new` or `other` may either start matching
/// `old` (if it were not already matching `old` at that index), or stop matching `old` (if if was
/// already matching `old` at that index).
///
/// For the set of valid (state, transition) pairs, see `calculate_next_state()`.
#[derive(Debug, PartialEq, Eq)]
enum MatchStateTransition {
    /// Offset into old, offset into new
    NewStartsMatching(usize, usize),
    /// Offset into old, offset into new
    NewStopsMatching(usize, usize),
    /// Offset into old, offset into other
    OtherStartsMatching(usize, usize),
    /// Offset into old, offset into other
    OtherStopsMatching(usize, usize),
}

use merge::MatchState::*;
use merge::MatchStateTransition::*;

/// Orders MatchStateTransitions by their offset into old (which is the ordering that
/// `calculate_match_state_transitions()` cares about).
impl Ord for MatchStateTransition {
    fn cmp(&self, other: &MatchStateTransition) -> Ordering {
        let self_offset = match self {
            &NewStartsMatching(ref offset, _) => offset,
            &NewStopsMatching(ref offset, _) => offset,
            &OtherStartsMatching(ref offset, _) => offset,
            &OtherStopsMatching(ref offset, _) => offset,
        };
        let other_offset = match other {
            &NewStartsMatching(ref offset, _) => offset,
            &NewStopsMatching(ref offset, _) => offset,
            &OtherStartsMatching(ref offset, _) => offset,
            &OtherStopsMatching(ref offset, _) => offset,
        };
        match self_offset.cmp(other_offset) {
            Ordering::Less | Ordering::Greater => self_offset.cmp(other_offset),
            Ordering::Equal => {
                // For transitions at the same offset in old, we put stops before starts to minimize
                // the number of empty chunks output, and put New* before Other* arbitrarily.
                let self_type = match self {
                    &NewStopsMatching(..) => 1,
                    &OtherStopsMatching(..) => 2,
                    &NewStartsMatching(..) => 3,
                    &OtherStartsMatching(..) => 4,
                };
                let other_type = match other {
                    &NewStopsMatching(..) => 1,
                    &OtherStopsMatching(..) => 2,
                    &NewStartsMatching(..) => 3,
                    &OtherStartsMatching(..) => 4,
                };
                self_type.cmp(&other_type)
            }
        }
    }
}

impl PartialOrd for MatchStateTransition {
    fn partial_cmp(&self, other: &MatchStateTransition) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Reprpesents the end of a chunk.
#[derive(Debug)]
enum ChunkEnd {
    /// Parameters: The end offset (exclusive) of the end of the chunk in old, new, and other.
    Stable(usize, usize, usize),
    /// Parameters: The end offset (exclusive) of the end of the chunk in old, new, and other.
    Unstable(usize, usize, usize),
}

#[derive(Debug, PartialEq, Eq)]
enum Chunk {
    /// Parameters: The start offset and length of the chunk in old.
    Stable(usize, usize),
    /// Parameters: The (start offset, length) of the chunk in old, new, and other
    /// respectively.
    Unstable((usize, usize), (usize, usize), (usize, usize)),
}

#[derive(Clone)]
struct Words<'a> {
    underlying_string: &'a str,
    char_indices: CharIndices<'a>,
    current_index: usize,
}

impl<'a> Words<'a> {
    fn new(underlying_string: &'a str) -> Words<'a> {
        Words {
            underlying_string: underlying_string,
            char_indices: underlying_string.char_indices(),
            current_index: 0,
        }
    }
}

impl<'a> Iterator for Words<'a> {
    type Item = &'a [u8];

    // TODO: these iterators have .clone().next() called a bunch of times. Might be better to
    // pre-calculate the next result.

    fn next(&mut self) -> Option<&'a [u8]> {
        let start = self.current_index;
        // Find the next space
        let mut looped = false;
        loop {
            match self.char_indices.next() {
                Some((_, ch)) if ch == ' ' || ch == '\r' || ch == '\n' || ch == '\t' => { break; },
                Some((_, ch)) => { looped = true; },
                None => {
                    if looped {
                        // TODO: Does this make a copy, and then take a reference to the copy? That
                        // wouldn't be ideal.
                        return Some(&self.underlying_string.as_bytes()[start..]);
                    } else {
                        return None;
                    }
                },
            }
        }
        // Then, find the beginning of the next word
        loop {
            match self.char_indices.clone().next() {
                Some((i, ch)) if !(ch == ' ' || ch == '\r' || ch == '\n' || ch == '\t') => {
                    self.current_index = i;
                    return Some(&self.underlying_string.as_bytes()[start..i]);
                },
                Some((_, ch)) => { self.char_indices.next(); },
                None => { return Some(&self.underlying_string.as_bytes()[start..]); },
            }
        }
    }
}

#[derive(Clone)]
pub struct Merger {
    /// The size (in bytes) above which a diff is automatically skipped, without any attempt to
    /// merge.
    diff_size_limit: usize,
}

impl Merger {
    pub fn new(diff_size_limit: usize) -> Merger {
        Merger { diff_size_limit: diff_size_limit }
    }

    /// Attempts a 3-way merge, merging `new` and `other` under the assumption that both diverged from
    /// `old`. If the strings do not merge together cleanly, returns `new`. Marks regions merged from
    /// `other` by putting `START_MARKER`, then `marker`, then `START_MARKER` at the beginning, and
    /// `END_MARKER`, `marker`, and `END_MARKER` at the end.
    /// TODO: describe return value
    pub fn try_merge(&self, old: &str, new: &str, other: &str, marker: &str) -> (String, bool) {
        let mut old_words = Words::new(old);
        let mut new_words = Words::new(new);
        let mut other_words = Words::new(other);

        // It entirely too long to calculate diffs this large. Our latency budget doesn't cover it.
        if num::abs(old.len() as i64 - other.len() as i64) > self.diff_size_limit as i64 {
            info!("Skipped large diff");
            // TODO: I should probably count this as a timeout. Experiment with that and see if it
            // works.
            return (new.to_owned(), false);
        }

        let new_lcs = longest_common_subsequence::get_longest_common_subsequence(
            old_words.clone(), new_words.clone());
        let other_lcs = longest_common_subsequence::get_longest_common_subsequence(
            old_words.clone(), other_words.clone());
        let (new_lcs, other_lcs) = match (new_lcs, other_lcs) {
            (Some(new_lcs), Some(other_lcs)) => (new_lcs, other_lcs),
            _ => { info!("Timed out computing LCS"); return (new.to_owned(), true); },
        };

        let mut bytes = Vec::<u8>::new();
        // TODO: See if these count()s are taking too long (they probably are). If they are, get the
        // iterator sizes in some other way, piggybacking off the iterator traversals in either this
        // file or longest_common_subsequence.rs.
        for chunk in parse(new_lcs, other_lcs, old_words.clone().count(), new_words.clone().count(),
                           other_words.clone().count()) {
            match chunk {
                Chunk::Stable(start, length) => {
                    for _ in 0..length {
                        bytes.extend(old_words.next().unwrap());
                        new_words.next().unwrap();
                        other_words.next().unwrap();
                    }
                },
                Chunk::Unstable((old_start, old_length), (new_start, new_length),
                                (other_start, other_length)) => {
                    let mut old_chunk: Vec<u8> = Vec::new();
                    let mut new_chunk: Vec<u8> = Vec::new();
                    let mut other_chunk: Vec<u8> = Vec::new();
                    for _ in 0..old_length {
                        old_chunk.extend(old_words.next().unwrap());
                    }
                    for _ in 0..new_length {
                        new_chunk.extend(new_words.next().unwrap());
                    }
                    for _ in 0..other_length {
                        other_chunk.extend(other_words.next().unwrap());
                    }

                    if old_chunk == new_chunk && old_chunk != other_chunk {
                        // Changed only in other
                        bytes.extend(START_MARKER.as_bytes());
                        bytes.extend(marker.as_bytes());
                        bytes.extend(START_MARKER.as_bytes());
                        bytes.extend(other_chunk);
                        bytes.extend(END_MARKER.as_bytes());
                        bytes.extend(marker.as_bytes());
                        bytes.extend(END_MARKER.as_bytes());
                    } else if old_chunk != new_chunk && old_chunk == other_chunk {
                        // Changed only in new
                        bytes.extend(new_chunk);
                    } else if old_chunk != new_chunk && new_chunk == other_chunk {
                        // Falsely conflicting, i.e. changed identically in both new and other
                        bytes.extend(new_chunk);
                    } else if (old_chunk != new_chunk && old_chunk != other_chunk &&
                               new_chunk != other_chunk) {
                        // Truly conflicting
                        // In a normal 3-way merge program, this means a failed merge requiring user
                        // intervention. Since we have no user to intervene and want to keep as much
                        // vandalism as possible, we keep other_chunk here and keep going.
                        bytes.extend(START_MARKER.as_bytes());
                        bytes.extend(marker.as_bytes());
                        bytes.extend(START_MARKER.as_bytes());
                        bytes.extend(other_chunk);
                        bytes.extend(END_MARKER.as_bytes());
                        bytes.extend(marker.as_bytes());
                        bytes.extend(END_MARKER.as_bytes());
                    }
                },
            }
        }
        (String::from_utf8(bytes).unwrap(), false)
    }
}

/// Calculates a "diff3 parse" as described in Khanna, Kunal, and Pierce 2007, given the longest
/// common subsequences between `old` and `new` and between `old` and `other`. This is an
/// implementation of the algorithm given in Figure 2 of that paper, using the state machine
/// described in `MatchState`, `MatchStateTransition`, and `calculate_next_state`.
fn parse(new_lcs: CommonSubsequence, other_lcs: CommonSubsequence, old_len: usize,
         new_len: usize, other_len: usize) -> Vec<Chunk> {
    let match_state_transitions = calculate_match_state_transitions(new_lcs, other_lcs);

    let mut chunk_ends: Vec<ChunkEnd> = Vec::new();
    let mut match_state = NeitherMatch;
    for transition in match_state_transitions {
        match calculate_chunk_end(&match_state, &transition) {
            Some(chunk_end) => chunk_ends.push(chunk_end),
            None => (),
        }
        match_state = calculate_next_state(&match_state, &transition);
    }
    chunk_ends.push(ChunkEnd::Unstable(old_len, new_len, other_len));

    let mut chunks: Vec<Chunk> = Vec::with_capacity(chunk_ends.len());
    let mut old_offset = 0;
    let mut new_offset = 0;
    let mut other_offset = 0;
    for chunk_end in chunk_ends {
        match chunk_end {
            ChunkEnd::Stable(old, new, other) => {
                if old != old_offset {
                    chunks.push(Chunk::Stable(old_offset, old - old_offset));
                    old_offset = old;
                    new_offset = new;
                    other_offset = other;
                }
            },
            ChunkEnd::Unstable(old, new, other) => {
                if old != old_offset || new != new_offset || other != other_offset {
                    chunks.push(Chunk::Unstable(
                        (old_offset, old - old_offset), (new_offset, new - new_offset),
                        (other_offset, other - other_offset)));
                    old_offset = old;
                    new_offset = new;
                    other_offset = other;
                }
            },
        }
    }
    chunks
}

/// From the LCS's for `old`/`new` and `old`/`other`, constructs a vector representing the state
/// transitions over the course of the string.
fn calculate_match_state_transitions(new_lcs: CommonSubsequence, other_lcs: CommonSubsequence) ->
    Vec<MatchStateTransition> {
    let mut match_state_transitions = Vec::from_iter(
        new_lcs.common_regions.into_iter().flat_map(|common_substring| vec![
            NewStartsMatching(common_substring.iter1_offset, common_substring.iter2_offset),
            NewStopsMatching(
                common_substring.iter1_offset + common_substring.size,
                common_substring.iter2_offset + common_substring.size)].into_iter()).chain(
            other_lcs.common_regions.into_iter().flat_map(|common_substring| vec![
                OtherStartsMatching(common_substring.iter1_offset, common_substring.iter2_offset),
                OtherStopsMatching(
                    common_substring.iter1_offset + common_substring.size,
                    common_substring.iter2_offset + common_substring.size)].into_iter())));
    match_state_transitions.sort();
    match_state_transitions
}

/// Given a match state and the transition out of it, calculates the ChunkEnd of the chunk output
/// upon that transition (if any).
fn calculate_chunk_end(match_state: &MatchState, transition: &MatchStateTransition) -> Option<ChunkEnd> {
    match (match_state, transition) {
        (&OnlyNewMatches(previous_old_offset, previous_new_offset),
         &OtherStartsMatching(current_old_offset, current_other_offset)) => {
            Some(ChunkEnd::Unstable(
                current_old_offset,
                previous_new_offset + (current_old_offset - previous_old_offset),
                current_other_offset))
        },
        (&OnlyOtherMatches(previous_old_offset, previous_other_offset),
         &NewStartsMatching(current_old_offset, current_new_offset)) => {
            Some(ChunkEnd::Unstable(
                current_old_offset, current_new_offset,
                previous_other_offset + (current_old_offset - previous_old_offset)))
        },
        (&BothMatch(previous_old_offset, _, previous_other_offset),
         &NewStopsMatching(current_old_offset, current_new_offset)) => {
            let length = current_old_offset - previous_old_offset;
            Some(ChunkEnd::Stable(
                current_old_offset, current_new_offset, previous_other_offset + length))
        }
        (&BothMatch(previous_old_offset, previous_new_offset, _),
         &OtherStopsMatching(current_old_offset, current_other_offset)) => {
            let length = current_old_offset - previous_old_offset;
            Some(ChunkEnd::Stable(
                current_old_offset, previous_new_offset + length, current_other_offset))
        }
        _ => None,
    }
}

/// Given a match state and the transition out of it, calculates the next state in the state
/// machine.
fn calculate_next_state(match_state: &MatchState, transition: &MatchStateTransition) -> MatchState {
    match (match_state, transition) {
        (&NeitherMatch, &NewStartsMatching(old, new))   => OnlyNewMatches(old, new),
        (&NeitherMatch, &OtherStartsMatching(old, new)) => OnlyOtherMatches(old, new),

        (&OnlyNewMatches(previous_old_offset, previous_new_offset),
         &OtherStartsMatching(current_old_offset, current_other_offset)) => {
            let length = current_old_offset - previous_old_offset;
            BothMatch(current_old_offset, previous_new_offset + length,
                      current_other_offset)
        },
        (&OnlyNewMatches(_, _), &NewStopsMatching(_, _)) => NeitherMatch,

        (&OnlyOtherMatches(previous_old_offset, previous_other_offset),
         &NewStartsMatching(current_old_offset, current_new_offset))   => {
            let length = current_old_offset - previous_old_offset;
            BothMatch(current_old_offset, current_new_offset,
                      previous_other_offset + length)
        },
        (&OnlyOtherMatches(_, _), &OtherStopsMatching(_, _))  => NeitherMatch,

        (&BothMatch(old, new, other), &NewStopsMatching(_, _)) => OnlyOtherMatches(old, other),
        (&BothMatch(old, new, other), &OtherStopsMatching(_, _))  => OnlyNewMatches(old, new),

        (state, transition) => {
            unreachable!("Illegal transition {:?} from state {:?}", transition, state);
        },
    }
}

#[cfg(test)]
mod tests {
    use super::{Chunk, calculate_match_state_transitions, parse, try_merge, Words};
    use super::MatchStateTransition::*;
    use ::{START_MARKER, END_MARKER};
    use longest_common_subsequence::{CommonSubsequence, CommonRegion};
    use regex::Regex;

    #[test]
    fn test_words_with_no_spaces_at_beginning_or_end() {
        let mut words = Words::new("0 1 2 3");
        assert_eq!(Some("0 ".as_bytes()), words.next());
        assert_eq!(Some("1 ".as_bytes()), words.next());
        assert_eq!(Some("2 ".as_bytes()), words.next());
        assert_eq!(Some("3".as_bytes()), words.next());
        assert_eq!(None, words.next());
    }

    #[test]
    fn test_words_with_spaces_at_beginning_and_end() {
        let mut words = Words::new(" 0 1 2 3 ");
        assert_eq!(Some(" ".as_bytes()), words.next());
        assert_eq!(Some("0 ".as_bytes()), words.next());
        assert_eq!(Some("1 ".as_bytes()), words.next());
        assert_eq!(Some("2 ".as_bytes()), words.next());
        assert_eq!(Some("3 ".as_bytes()), words.next());
        assert_eq!(None, words.next());
    }

    #[test]
    fn test_words_with_multiple_spaces() {
        let mut words = Words::new("  0  1\r\n\t2  3  ");
        assert_eq!(Some("  ".as_bytes()), words.next());
        assert_eq!(Some("0  ".as_bytes()), words.next());
        assert_eq!(Some("1\r\n\t".as_bytes()), words.next());
        assert_eq!(Some("2  ".as_bytes()), words.next());
        assert_eq!(Some("3  ".as_bytes()), words.next());
        assert_eq!(None, words.next());
    }

    #[test]
    fn test_words_with_multibyte_characters() {
        let mut words = Words::new("  0  1\r\n\tさようなら  3  ");
        assert_eq!(Some("  ".as_bytes()), words.next());
        assert_eq!(Some("0  ".as_bytes()), words.next());
        assert_eq!(Some("1\r\n\t".as_bytes()), words.next());
        assert_eq!(Some("さようなら  ".as_bytes()), words.next());
        assert_eq!(Some("3  ".as_bytes()), words.next());
        assert_eq!(None, words.next());
    }

    // TODO: Add test for timeout

    #[test]
    fn test_try_merge_empty() {
        assert_eq!(("".to_string(), false), try_merge("", "", "", ""));
    }

    #[test]
    fn test_try_merge_clean() {
        let old = "First sentence. Second sentence.";
        let new = "First sentence. Second sentence changed.";
        let other = "First sentence changed. Second sentence.";
        let expected = format!("First {}test{}sentence changed. {}test{}Second sentence changed.",
                               START_MARKER, START_MARKER, END_MARKER, END_MARKER);
        assert_eq!((expected, false), try_merge(old, new, other, "test"));
    }

    #[test]
    fn test_try_merge_conflicting() {
        let old = "First sentence. Second sentence.";
        let new = "First sentence. Second sentence changed one way.";
        let other = "First sentence changed. Second sentence changed a different way.";
        let expected = format!(
            "First {}123{}sentence changed. {}123{}Second {}123{}sentence changed a different way.{}123{}",
            START_MARKER, START_MARKER, END_MARKER, END_MARKER,
            START_MARKER, START_MARKER, END_MARKER, END_MARKER);
        assert_eq!((expected, false), try_merge(old, new, other, "123"));
    }

    #[test]
    fn test_try_merge_with_change_at_end() {
        let old = "Test string. ";
        let new = "Test 1 string. ";
        let other = "Test string. 2";
        let expected = format!("Test 1 string. {}test{}2{}test{}",
                               START_MARKER, START_MARKER, END_MARKER, END_MARKER);
        assert_eq!((expected, false), try_merge(old, new, other, "test"));
    }

    #[test]
    fn test_try_merge_special_characters() {
        let old = "First sentence. Second sentence.";
        let new = "First sentence. Second sentence 𐅃.";
        let other = "First sentence さようなら. Second sentence.";
        let expected = format!(
            "First {}test{}sentence さようなら. {}test{}Second sentence 𐅃.",
            START_MARKER, START_MARKER, END_MARKER, END_MARKER);
        assert_eq!((expected, false), try_merge(old, new, other, "test"));
    }

    #[test]
    fn test_calculate_match_state_transitions() {
        // This test case uses the strings from figure 1 of Khanna, Kunal, and Pierce 2007.
        let new_lcs = CommonSubsequence::new(vec![
            CommonRegion::new(0, 0, 1), CommonRegion::new(1, 3, 2), CommonRegion::new(5, 5, 1)]);
        let other_lcs = CommonSubsequence::new(vec![
            CommonRegion::new(0, 0, 2), CommonRegion::new(3, 2, 2), CommonRegion::new(5, 5, 1)]);
        let expected = vec![
            NewStartsMatching(0, 0),
            OtherStartsMatching(0, 0),

            NewStopsMatching(1, 1),
            NewStartsMatching(1, 3),

            OtherStopsMatching(2, 2),

            NewStopsMatching(3, 5),
            OtherStartsMatching(3, 2),

            OtherStopsMatching(5, 4),
            NewStartsMatching(5, 5),
            OtherStartsMatching(5, 5),

            NewStopsMatching(6, 6),
            OtherStopsMatching(6, 6)];
        assert_eq!(expected, calculate_match_state_transitions(new_lcs, other_lcs));
    }

    #[test]
    fn test_parse() {
        // This uses the strings from figure 1 of Khanna, Kunal, and Pierce 2007, but with an
        // extra unstable chunk at the end.
        let new_lcs = CommonSubsequence::new(vec![
            CommonRegion::new(0, 0, 1), CommonRegion::new(1, 3, 2), CommonRegion::new(5, 5, 1)]);
        let other_lcs = CommonSubsequence::new(vec![
            CommonRegion::new(0, 0, 2), CommonRegion::new(3, 2, 2), CommonRegion::new(5, 5, 1)]);
        let expected = vec![Chunk::Stable(0, 1),
                            Chunk::Unstable((1, 0), (1, 2), (1, 0)),
                            Chunk::Stable(1, 1),
                            Chunk::Unstable((2, 3), (4, 1), (2, 3)),
                            Chunk::Stable(5, 1),
                            Chunk::Unstable((6, 0), (6, 0), (6, 1))];
        assert_eq!(expected, parse(new_lcs, other_lcs, 6, 6, 7));
    }
}
