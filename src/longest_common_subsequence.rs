//! This file implements a longest-common-subsequence algorithm, needed for the 3-way merge
//! algorithm in merge.rs. This is essentially an edit-distance algorithm, but retains complementary
//! information. Only the matching subsequences are needed for the 3-way merge, whereas an edit
//! distance algorithm produces the opposite, a detailed account of what changed in the non-matching
//! subsequences.
//!
//! This is based heavily on the algorithm in:
//!
//! Miller, W. and Myers, E. W. (1985), A file comparison program. Softw: Pract. Exper., 15:
//! 1025–1040. doi: 10.1002/spe.4380151102
//!
//! ...but is written in a very different style than the program in the appendix of that paper. The
//! text of the paper describes the algorithm in a very understandable way by talking in terms of
//! small tasks ("move right", "move down", "slide down the diagonal"), but the C code in the
//! appendix is written in a very deconstructed (is that the word?) style, using an array called
//! last_d that I found it harder to think in terms of.
//!
//! I instead implemented the algorithm with a work queue, on which each task is a possible
//! longest-common-subsequence for a given pair of substrings starting at 0, and the work to be done
//! is the 3 steps:
//!
//! 1. Slide down the diagonal (i.e., move forward in both strings) as far as possible
//! 2. Enqueue a new task to try going right (i.e., farther in string 1)
//! 3. Enqueue a new task to try going down (i.e., farther in string 2)
//!
//! I didn't realize this until after implementing it, but I believe this is, essentailly, A*, with
//! the optimization of sliding down the diagonals as far as possible at each step.
//!
//! Because the priority function used for the work queue gives exactly the same prioritization as
//! edit distance, I believe the algorithm written this way has the same performance characteristics
//! as described in the paper (although there is overhead in popping each task off the work queue,
//! and storing extra tasks of low priority in the work queue).

extern crate num;
extern crate time;

use std::cmp::Ord;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::collections::binary_heap::BinaryHeap;
use std::ops::Index;
use std::ops::IndexMut;
use std::str::CharIndices;

// TODO: constructor for these?
#[derive(PartialEq, Clone, Debug)]
pub struct CommonSubstring {
    /// The byte offset into str1 of the beginning of the common substring.
    pub str1_offset: usize,
    /// The byte offset into str2 of the beginning of the common substring.
    pub str2_offset: usize,
    /// The length of the common substring in bytes.
    pub size_bytes: usize,
}

#[derive(PartialEq, Clone, Debug)]
pub struct CommonSubsequence {
    pub common_substrings: Vec<CommonSubstring>,
    /// The total length of the common subsequence in bytes.
    pub size_bytes: usize,
}

/// A Task represents a step of the algorithm that needs to be done. A Task records a possible
/// longest common subsequence up to a particular offset in each string. Executing a Task means
/// moving as far forward in both strings as possible (for as long as they match, starting at the
/// Task's offsets), then enqueuing Tasks to try moving one word farther in each string.
struct Task<'a> {
    /// The highest offset in str1 which has been searched
    str1_offset: usize,
    /// The highest offset in str2 which has been searched
    str2_offset: usize,
    /// The iterator representing the current position in str1
    str1_words: Words<'a>,
    /// The iterator representing the current position in str2
    str2_words: Words<'a>,
    /// The common subsequence which is known so far.
    common_subsequence: CommonSubsequence,
}

impl<'a> PartialEq for Task<'a> {
    fn eq(&self, other: &Task) -> bool {
        self.str1_offset == other.str1_offset && 
            self.str2_offset == other.str2_offset && 
            self.str1_words.clone().next() == other.str1_words.clone().next() &&
            self.str2_words.clone().next() == other.str2_words.clone().next() &&
            self.common_subsequence == other.common_subsequence
    }
}

impl<'a> Eq for Task<'a> {}

impl<'a> PartialOrd for Task<'a> {
    fn partial_cmp(&self, other: &Task) -> Option<Ordering> {
        return Some(self.cmp(other));
    }
}

impl<'a> Ord for Task<'a> {
    fn cmp(&self, other: &Task) -> Ordering {
        // This value is -1 times the edit distance implied by the task's common subsequence: the
        // edit distance between two strings with no matching characters is str1_offset +
        // str2_offset (with the edit algorithm being to delete each character in one string and
        // insert each character in the other), and each matching character decreases that by two
        // (because it saves one deletion and one insertion).
        //
        // The lowest (implied) edit distance makes for the best priority function here for either of
        // two reasons:
        //
        // - Conceptualizing this algorithm as a version of the algorithm in Miller and Myers 1985:
        // using the edit distance is identical to the algorithm in the paper, which starts at edit
        // distance 0 and finds how far into the two strings it's possible to go with each edit
        // distance.
        // - Conceptualizing this algorithm as an implementation of A* on a grid like in the figures
        // in Miller and Myers 1975: this heuristic is admissible because (-str1_offset +
        // -str2_offset) is the negative Manhattan distance to the goal, plus a constant (the
        // Manhattan distance from the start node to the goal node).
        let self_value = self.common_subsequence.size_bytes as i64 * 2
            - self.str1_offset as i64 - self.str2_offset as i64;
        let other_value = other.common_subsequence.size_bytes as i64 * 2
            - other.str1_offset as i64 - other.str2_offset as i64;
        if self_value > other_value {
            return Ordering::Greater;
        } else if self_value < other_value {
            return Ordering::Less;
        }

        // For tasks that are doing equally well by the above calculation, we prefer the one whose
        // offsets are closer together. Since the diffs tend to be small relative to text size for
        // Wikipedia articles, such tasks are more likely to end up winners.
        let self_offset_diff = num::abs(self.str2_offset as i64 - self.str1_offset as i64);
        let other_offset_diff = num::abs(other.str2_offset as i64 - other.str1_offset as i64);
        if self_offset_diff < other_offset_diff {
            return Ordering::Greater; // If self is closer, its value is greater
        } else if self_offset_diff > other_offset_diff {
            return Ordering::Less;
        }

        // For tasks that are doing equally well by the above two calculations, we prefer the one
        // that is farther along, under the assumption that it's more likely to be a winner.
        if self.str1_offset + self.str2_offset > other.str1_offset + other.str2_offset {
            return Ordering::Greater;
        } else if self.str1_offset + self.str2_offset < other.str1_offset + other.str2_offset {
            return Ordering::Less;
        }

        // If none of the above calculations could differentiate between the two, we
        // descend into arbitrary metrics.
        if self.str1_offset > other.str1_offset {
            return Ordering::Greater;
        } else if self.str1_offset < other.str1_offset {
            return Ordering::Less;
        } else if self.str2_offset > other.str2_offset {
            return Ordering::Greater;
        } else if self.str2_offset < other.str2_offset {
            return Ordering::Less;
        } else {
            // At this point, the two are at the same offsets, with the same common subsequence
            // size. The one who has a bigger common substring earliest wins, for no good reason.
            for (self_common_subsequence, other_common_subsequence) in
                (&self.common_subsequence.common_substrings).into_iter().zip(
                    (&other.common_subsequence.common_substrings).into_iter()) {
                if self_common_subsequence.size_bytes > other_common_subsequence.size_bytes {
                    return Ordering::Greater;
                } else {
                    return Ordering::Less;
                }
            }
        }
        Ordering::Equal
    }
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
    type Item = (usize, &'a [u8]);

    // TODO: these iterators have .clone().next() called a bunch of times. Might be better to
    // pre-calculate the next result.

    fn next(&mut self) -> Option<(usize, &'a [u8])> {
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
                        return Some((start, &self.underlying_string.as_bytes()[start..]));
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
                    return Some((start, &self.underlying_string.as_bytes()[start..i]));
                },
                Some((_, ch)) => { self.char_indices.next(); },
                None => { return Some((start, &self.underlying_string.as_bytes()[start..])); },
            }
        }
    }
}

pub fn get_longest_common_subsequence(str1: &str, str2: &str) -> CommonSubsequence {
    let mut work_queue: BinaryHeap<Task> = BinaryHeap::new();
    let first_task =
        Task {
            str1_offset: 0,
            str2_offset: 0,
            common_subsequence:
                CommonSubsequence { common_substrings: vec![], size_bytes: 0 },
            str1_words: Words::new(str1),
            str2_words: Words::new(str2),
        };
    work_queue.push(first_task);

    // Tracks the size of the longest common subsequence that's known so far up to each combination
    // of str1_offset and str2_offset. A Task whose common_subsequence does not have a size greater
    // than the corresponding value in this HashMap will not be inserted into the work queue.
    let mut longest_known_common_subsequences: HashMap<(usize, usize), usize> = HashMap::new();

    loop {
        let mut task = work_queue.pop().unwrap();

        // 1. Move forward in both strings for as long as they match.
        let mut matching_bytes = 0;
        let mut matching_words = 0;
        let mut finished = false;
        loop {
            match (task.str1_words.clone().next(), task.str2_words.clone().next()) {
                (Some((_, str1_word)), Some((_, str2_word))) if str1_word == str2_word => {
                    matching_bytes += str1_word.len();
                    matching_words += 1;
                    // We can only advance either iterator when we advance both iterators, so we
                    // clone them in the match condition and then advance them here. If we were to
                    // advance them both in the match condition above, when we reached the end of
                    // one string but not the other, we'd advance twice before cloning the iterator
                    // for the new task, and miss an offset.
                    task.str1_words.next();
                    task.str2_words.next();
                },
                (None, None) => {
                    // We've reached the end of both strings, and have our answer.
                    finished = true;
                    break;
                },
                _ => {
                    break;
                },
            }
        }

        // 2. Add a new common substring to the common subsequence if one of non-zero size was
        // found.
        let mut new_common_subsequence = task.common_subsequence.clone();
        if matching_bytes > 0 {
            new_common_subsequence.common_substrings.push(
                CommonSubstring {
                    str1_offset: task.str1_offset,
                    str2_offset: task.str2_offset,
                    size_bytes: matching_bytes,
                });
            new_common_subsequence.size_bytes += matching_bytes;
        }

        if finished {
            println!("Last task priority: {}",
                     task.common_subsequence.size_bytes as i64 * 2
                     - task.str1_offset as i64 - task.str2_offset as i64);
            return new_common_subsequence;
        }

        // 3a. Enqueue another task in the work queue that starts one word farther into str1 and at
        // the same offset into str2.
        let new_str1_offset = task.str1_offset + matching_bytes;
        let new_str2_offset = task.str2_offset + matching_bytes;
        if new_str1_offset < str1.len() {
            match longest_known_common_subsequences.get(&(new_str1_offset + 1, new_str2_offset)) {
                Some(size) if size >= &new_common_subsequence.size_bytes => (),
                _ => {
                    let mut new_str1_words = task.str1_words.clone();
                    let next_word = new_str1_words.next().unwrap().1;
                    work_queue.push(
                        Task {
                            str1_offset: new_str1_offset + next_word.len(),
                            str2_offset: new_str2_offset,
                            common_subsequence: new_common_subsequence.clone(),
                            str1_words: new_str1_words,
                            str2_words: task.str2_words.clone(),
                        });
                }
            }
            // This separate block is necessary because of issue 6393 - I can't insert() into
            // longest_known_common_subsequences in the match block on the get().
            match longest_known_common_subsequences.entry((new_str1_offset + 1, new_str2_offset)) {
                Entry::Occupied(ref entry) if entry.get() >= &new_common_subsequence.size_bytes => (),
                Entry::Occupied(mut entry) => { entry.insert(new_common_subsequence.size_bytes); }
                Entry::Vacant(entry) => { entry.insert(new_common_subsequence.size_bytes); }
            }

        }

        // 3b. Enqueue another task in the work queue that starts at the same offset into str1 and
        // one word farther into str2.
        if new_str2_offset < str2.len() {
            match longest_known_common_subsequences.get(&(new_str1_offset, new_str2_offset + 1)) {
                Some(size) if size >= &new_common_subsequence.size_bytes => (),
                _ => {
                    let mut new_str2_words = task.str2_words.clone();
                    let next_word = new_str2_words.next().unwrap().1;
                    work_queue.push(
                        Task {
                            str1_offset: new_str1_offset,
                            str2_offset: new_str2_offset + next_word.len(),
                            common_subsequence: new_common_subsequence.clone(),
                            str1_words: task.str1_words.clone(),
                            str2_words: new_str2_words,
                        });
                },
            }
            // This separate block is necessary because of issue 6393 - I can't insert() into
            // longest_known_common_subsequences in a match block for
            // longest_known_common_subsequences.get().
            match longest_known_common_subsequences.entry((new_str1_offset, new_str2_offset + 1)) {
                Entry::Occupied(ref entry) if entry.get() >= &new_common_subsequence.size_bytes =>
                    (),
                Entry::Occupied(mut entry) => { entry.insert(new_common_subsequence.size_bytes); }
                Entry::Vacant(entry) => { entry.insert(new_common_subsequence.size_bytes); }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{get_longest_common_subsequence, CommonSubsequence, CommonSubstring, Words};

    #[test]
    fn test_words_with_no_spaces_at_beginning_or_end() {
        let mut words = Words::new("0 1 2 3");
        assert_eq!(Some((0, "0 ".as_bytes())), words.next());
        assert_eq!(Some((2, "1 ".as_bytes())), words.next());
        assert_eq!(Some((4, "2 ".as_bytes())), words.next());
        assert_eq!(Some((6, "3".as_bytes())), words.next());
        assert_eq!(None, words.next());
    }

    #[test]
    fn test_words_with_spaces_at_beginning_and_end() {
        let mut words = Words::new(" 0 1 2 3 ");
        assert_eq!(Some((0, " ".as_bytes())), words.next());
        assert_eq!(Some((1, "0 ".as_bytes())), words.next());
        assert_eq!(Some((3, "1 ".as_bytes())), words.next());
        assert_eq!(Some((5, "2 ".as_bytes())), words.next());
        assert_eq!(Some((7, "3 ".as_bytes())), words.next());
        assert_eq!(None, words.next());
    }

    #[test]
    fn test_words_with_multiple_spaces() {
        let mut words = Words::new("  0  1\r\n\t2  3  ");
        assert_eq!(Some((0, "  ".as_bytes())), words.next());
        assert_eq!(Some((2, "0  ".as_bytes())), words.next());
        assert_eq!(Some((5, "1\r\n\t".as_bytes())), words.next());
        assert_eq!(Some((9, "2  ".as_bytes())), words.next());
        assert_eq!(Some((12, "3  ".as_bytes())), words.next());
        assert_eq!(None, words.next());
    }

    #[test]
    fn test_words_with_multibyte_characters() {
        let mut words = Words::new("  0  1\r\n\tさようなら  3  ");
        assert_eq!(Some((0, "  ".as_bytes())), words.next());
        assert_eq!(Some((2, "0  ".as_bytes())), words.next());
        assert_eq!(Some((5, "1\r\n\t".as_bytes())), words.next());
        assert_eq!(Some((9, "さようなら  ".as_bytes())), words.next());
        assert_eq!(Some((26, "3  ".as_bytes())), words.next());
        assert_eq!(None, words.next());
    }

    #[test]
    fn test_lcs_identical_strings() {
        let test_string = "test identical strings";
        let expected =
            CommonSubsequence {
                common_substrings: vec![
                    CommonSubstring { str1_offset: 0, str2_offset: 0, size_bytes: 22 }],
                size_bytes: 22 };
        assert_eq!(expected, get_longest_common_subsequence(&test_string, &test_string));
    }

    #[test]
    fn test_lcs_diff_in_middle() {
        let test_string = "test string";
        let test_string2 = "test diff in middle string";
        let expected =
            CommonSubsequence {
                common_substrings: vec![
                    CommonSubstring { str1_offset: 0, str2_offset: 0, size_bytes: 5 },
                    CommonSubstring { str1_offset: 5, str2_offset: 20,size_bytes: 6 }],
                size_bytes: 11 };
        assert_eq!(expected, get_longest_common_subsequence(&test_string, &test_string2));
    }

    #[test]
    fn test_lcs_complicated_diff() {
        let test_string = "1 2 3 4 5 6";
        let test_string2 = "1 2 4 5 3 6";
        let expected = 
            CommonSubsequence {
                common_substrings: vec![
                    CommonSubstring { str1_offset: 0, str2_offset: 0, size_bytes: 4 },
                    CommonSubstring { str1_offset: 6, str2_offset: 4, size_bytes: 4 },
                    CommonSubstring { str1_offset: 10, str2_offset: 10, size_bytes: 1 }],
                size_bytes: 9 };
        assert_eq!(expected, get_longest_common_subsequence(&test_string, &test_string2));
    }

    #[test]
    fn test_lcs_no_words_in_common() {
        let test_string = "a b c d e f g";
        let test_string2 = "1 2 3 4 5 6 7 8";
        let expected =
            CommonSubsequence { common_substrings: vec![], size_bytes: 0 };
        assert_eq!(expected, get_longest_common_subsequence(&test_string, &test_string2));
    }

    #[test]
    fn test_lcs_special_characters() {
        let test_string = "Test さよ string 𐅃.";
        let test_string2 = "Test さよ うなら string .";
        let expected =
            CommonSubsequence {
                common_substrings: vec![
                    CommonSubstring { str1_offset: 0, str2_offset: 0, size_bytes: 12 },
                    CommonSubstring { str1_offset: 12, str2_offset: 22, size_bytes: 7 }],
                size_bytes: 19 };
        assert_eq!(expected, get_longest_common_subsequence(&test_string, &test_string2));
    }

    //use hyper::Client;
    //use time;
    //use wiki::Wiki;
    //
    //#[test]
    //fn test_wikipedia() {
    //    let wiki = Wiki::new("en.wikipedia.org".to_string(), 443, Client::new(), None);
    //    let revision = wiki.get_latest_revision("Metaphorical_Music").unwrap();
    //    let before = wiki.get_revision_content("Metaphorical_Music", revision.parentid).unwrap();
    //    let after = wiki.get_revision_content("Metaphorical_Music", revision.revid).unwrap();
    //    let start_time = time::precise_time_ns();
    //    get_longest_common_subsequence(&before, &after);
    //    println!("Time to find LCS: {} us", (time::precise_time_ns() - start_time) / 1000);
    //    assert!(false);
    //}
}
