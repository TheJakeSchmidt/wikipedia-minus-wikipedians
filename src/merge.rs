use std::cmp::Ordering;
use std::iter::FromIterator;

use ::longest_common_subsequence;
use ::longest_common_subsequence::CommonSubsequence;

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
/// For the set of valid (state, transition) pairs, see `merge()`.
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

/// Reprpesents the  TODO: finish comment. Or, just use tuples (usize, usize, usize, bool) for this.
#[derive(Debug)]
enum ChunkEnd {
    Stable(usize, usize, usize),
    Unstable(usize, usize, usize),
}

#[derive(Debug, PartialEq, Eq)]
enum Chunk {
    // TODO: change this to (start, end), instead of (start, length). Lengths are basically never useful.
    // Parameters: The offset into old, and length of the chunk.
    Stable(usize, usize),
    // Parameters: The (offset into string, length) of the chunk for old, new, and other
    // respectively.
    Unstable((usize, usize), (usize, usize), (usize, usize)),
}

fn try_merge(old: &str, new: &str, other: &str) -> String {
    let new_lcs = longest_common_subsequence::get_longest_common_subsequence(old, new);
    let other_lcs = longest_common_subsequence::get_longest_common_subsequence(old, other);
    let mut byte_slices = Vec::<&[u8]>::new();
    for chunk in parse(new_lcs, other_lcs) {
        match chunk {
            Chunk::Stable(start, length) => {
                println!("Stable chunk: {:?}", &old.as_bytes()[start..(start+length)]);
                byte_slices.push(&old.as_bytes()[start..(start+length)]);
            },
            Chunk::Unstable((old_start, old_length), (new_start, new_length),
                            (other_start, other_length)) => {
                let old_chunk = &old.as_bytes()[old_start..(old_start+old_length)];
                let new_chunk = &new.as_bytes()[new_start..(new_start+new_length)];
                let other_chunk = &other.as_bytes()[other_start..(other_start+other_length)];
                if old_chunk == new_chunk && old_chunk != other_chunk {
                    // Changed only in other
                    println!("Changed only in other: {:?}", other_chunk);
                    byte_slices.push(other_chunk);
                } else if old_chunk != new_chunk && old_chunk == other_chunk {
                    // Changed only in new
                    println!("Changed only in new: {:?}", new_chunk);
                    byte_slices.push(new_chunk);
                } else if old_chunk != new_chunk && new_chunk == other_chunk {
                    // Falsely conflicting, i.e. changed identically in both new and other
                    println!("Falsely conflicting: {:?}", new_chunk);
                    byte_slices.push(new_chunk);
                } else if (old_chunk != new_chunk && old_chunk != other_chunk &&
                           new_chunk != other_chunk) {
                    // Truly conflicting 
                    println!("Truly conflicting.");
                    return new.to_owned();
                }
            },
        }
    }

    let mut bytes: Vec<u8> = Vec::new();
    for byte_slice in byte_slices {
        bytes.extend(byte_slice);
    }
    String::from_utf8(bytes).unwrap()
}

// TODO: doc comment
fn parse(new_lcs: CommonSubsequence, other_lcs: CommonSubsequence) -> Vec<Chunk> {
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
                    chunks.push(Chunk::Unstable((old_offset, old - old_offset),
                                                (new_offset, new - new_offset),
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

/// From the LCS's for `old`/`new` and `old`/`other`, constructs a vector representing the state transitions
/// over the course of the string.
fn calculate_match_state_transitions(new_lcs: CommonSubsequence, other_lcs: CommonSubsequence) ->
    Vec<MatchStateTransition> {
    let mut match_state_transitions = Vec::from_iter(
        new_lcs.common_substrings.into_iter().flat_map(|common_substring| vec![
            NewStartsMatching(common_substring.str1_offset, common_substring.str2_offset),
            NewStopsMatching(
                common_substring.str1_offset + common_substring.size_bytes,
                common_substring.str2_offset + common_substring.size_bytes)].into_iter()).chain(
            other_lcs.common_substrings.into_iter().flat_map(|common_substring| vec![
                OtherStartsMatching(common_substring.str1_offset, common_substring.str2_offset),
                OtherStopsMatching(
                    common_substring.str1_offset + common_substring.size_bytes,
                    common_substring.str2_offset + common_substring.size_bytes)].into_iter())));
    match_state_transitions.sort();
    match_state_transitions
}

// TODO: doc comment
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

// TODO: doc comment
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
    use super::{Chunk, calculate_match_state_transitions, parse, try_merge};
    use super::MatchStateTransition::*;
    use longest_common_subsequence::{CommonSubsequence, CommonSubstring};

    #[test]
    fn test_merge_clean() {
        let old = "First line.\n\nSecond line.\n";
        let new = "First line.\n\nSecond line changed.\n";
        let other = "First line changed.\n\nSecond line.\n";
        assert_eq!("First line changed.\n\nSecond line changed.\n".to_string(),
                   try_merge(old, new, other));
    }

    #[test]
    fn test_merge_conflicting() {
        let old = "First line.\n\nSecond line.\n";
        let new = "First line.\n\nSecond line changed one way.\n";
        let other = "First line changed.\n\nSecond line changed a different way.\n";
        assert_eq!(new, try_merge(old, new, other));
    }

    #[test]
    fn test_merge_special_characters() {
        let old = "First line.\n\nSecond line.\n";
        let new = "First line.\n\nSecond line 𐅃.\n";
        let other = "First line さようなら.\n\nSecond line.\n";
        assert_eq!("First line さようなら.\n\nSecond line 𐅃.\n".to_string(),
                   try_merge(old, new, other));
    }

    #[test]
    fn test_calculate_match_state_transitions() {
        // This is from figure 1 of diff3-short.pdf.
        // TODO: comment better.
        let new_lcs = CommonSubsequence {
            common_substrings: vec![
                CommonSubstring {
                    str1_offset: 0,
                    str2_offset: 0,
                    size_bytes: 1,
                },
                CommonSubstring {
                    str1_offset: 1,
                    str2_offset: 3,
                    size_bytes: 2,
                },
                CommonSubstring {
                    str1_offset: 5,
                    str2_offset: 5,
                    size_bytes: 1,
                }],
            size_bytes: 4,
            size_chars: 4,
        };
        let other_lcs = CommonSubsequence {
            common_substrings: vec![
                CommonSubstring {
                    str1_offset: 0,
                    str2_offset: 0,
                    size_bytes: 2,
                },
                CommonSubstring {
                    str1_offset: 3,
                    str2_offset: 2,
                    size_bytes: 2,
                },
                CommonSubstring {
                    str1_offset: 5,
                    str2_offset: 5,
                    size_bytes: 1,
                }],
            size_bytes: 5,
            size_chars: 5,
        };
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

    // TODO: Make sure this test covers all branches, transitions, states, etc.
    #[test]
    fn test_whatever() {
        // This is from figure 1 of diff3-short.pdf.
        // TODO: comment better.
        let new_lcs = CommonSubsequence {
            common_substrings: vec![
                CommonSubstring {
                    str1_offset: 0,
                    str2_offset: 0,
                    size_bytes: 1,
                },
                CommonSubstring {
                    str1_offset: 1,
                    str2_offset: 3,
                    size_bytes: 2,
                },
                CommonSubstring {
                    str1_offset: 5,
                    str2_offset: 5,
                    size_bytes: 1,
                }],
            size_bytes: 4,
            size_chars: 4,
        };
        let other_lcs = CommonSubsequence {
            common_substrings: vec![
                CommonSubstring {
                    str1_offset: 0,
                    str2_offset: 0,
                    size_bytes: 2,
                },
                CommonSubstring {
                    str1_offset: 3,
                    str2_offset: 2,
                    size_bytes: 2,
                },
                CommonSubstring {
                    str1_offset: 5,
                    str2_offset: 5,
                    size_bytes: 1,
                }],
            size_bytes: 5,
            size_chars: 5,
        };
        let expected = vec![Chunk::Stable(0, 1),
                            Chunk::Unstable((1, 0), (1, 2), (1, 0)),
                            Chunk::Stable(1, 1),
                            Chunk::Unstable((2, 3), (4, 1), (2, 3)),
                            Chunk::Stable(5, 1)];
        assert_eq!(expected, parse(new_lcs, other_lcs));
    }
}
