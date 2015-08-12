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
    /// The total length of the common subsequence in characters.
    pub size_chars: usize,
}

/// A Task represents a step of the algorithm that needs to be done. A Task records a possible
/// longest common subsequence up to a particular offset in each string. Executing a Task means
/// moving as far forward in both strings as possible (for as long as they match, starting at the
/// Task's offsets), then enqueuing Tasks to try moving one character farther in each string.
struct Task<'a> {
    /// The highest offset in str1 which has been searched
    str1_offset: usize,
    /// The highest offset in str2 which has been searched
    str2_offset: usize,
    /// The iterator representing the current position in str1
    str1_chars: CharIndices<'a>,
    /// The iterator representing the current position in str2
    str2_chars: CharIndices<'a>,
    /// The common subsequence which is known so far.
    common_subsequence: CommonSubsequence,
}

impl<'a> PartialEq for Task<'a> {
    fn eq(&self, other: &Task) -> bool {
        self.str1_offset == other.str1_offset && 
            self.str2_offset == other.str2_offset && 
            self.str1_chars.clone().next() == other.str1_chars.clone().next() && 
            self.str2_chars.clone().next() == other.str2_chars.clone().next() && 
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
        let self_value = self.common_subsequence.size_chars as i64 * 2
            - self.str1_offset as i64 - self.str2_offset as i64;
        let other_value = self.common_subsequence.size_chars as i64 * 2
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

pub fn get_longest_common_subsequence(str1: &str, str2: &str) -> CommonSubsequence {
    let mut work_queue: BinaryHeap<Task> = BinaryHeap::new();
    let first_task =
        Task {
            str1_offset: 0,
            str2_offset: 0,
            common_subsequence:
                CommonSubsequence { common_substrings: vec![], size_bytes: 0, size_chars: 0 },
            str1_chars: str1.char_indices(),
            str2_chars: str2.char_indices(),
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
        let mut matching_chars = 0;
        let mut finished = false;
        loop {
            match (task.str1_chars.clone().next(), task.str2_chars.clone().next()) {
                (Some((index, str1_char)), Some((_, str2_char))) if str1_char == str2_char => {
                    matching_bytes += str1_char.len_utf8();
                    matching_chars += 1;
                    // We can only advance either iterator when we advance both iterators, so we
                    // clone them in the match condition and then advance them here. If we were to
                    // advance them both in the match condition above, when we reached the end of
                    // one string but not the other, we'd advance twice before cloning the iterator
                    // for the new task, and miss an offset.
                    task.str1_chars.next();
                    task.str2_chars.next();
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
            new_common_subsequence.size_chars += matching_chars;
        }

        if finished {
            return new_common_subsequence;
        }

        // 3a. Enqueue another task in the work queue that starts one character farther into str1
        // and at the same offset into str2.
        let new_str1_offset = task.str1_offset + matching_bytes;
        let new_str2_offset = task.str2_offset + matching_bytes;
        if new_str1_offset < str1.len() {
            match longest_known_common_subsequences.get(&(new_str1_offset + 1, new_str2_offset)) {
                Some(size) if size >= &new_common_subsequence.size_bytes => (),
                _ => {
                    let mut new_str1_chars = task.str1_chars.clone();
                    let next_char = new_str1_chars.next().unwrap().1;
                    work_queue.push(
                        Task {
                            str1_offset: new_str1_offset + next_char.len_utf8(),
                            str2_offset: new_str2_offset,
                            common_subsequence: new_common_subsequence.clone(),
                            str1_chars: new_str1_chars,
                            str2_chars: task.str2_chars.clone(),
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
        // one character farther into str2.
        if new_str2_offset < str2.len() {
            match longest_known_common_subsequences.get(&(new_str1_offset, new_str2_offset + 1)) {
                Some(size) if size >= &new_common_subsequence.size_bytes => (),
                _ => {
                    let mut new_str2_chars = task.str2_chars.clone();
                    let next_char = new_str2_chars.next().unwrap().1;
                    work_queue.push(
                        Task {
                            str1_offset: new_str1_offset,
                            str2_offset: new_str2_offset + next_char.len_utf8(),
                            common_subsequence: new_common_subsequence.clone(),
                            str1_chars: task.str1_chars.clone(),
                            str2_chars: new_str2_chars,
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
    use super::{get_longest_common_subsequence, CommonSubsequence, CommonSubstring};

    #[test]
    fn test_identical_strings() {
        let test_string = "test identical strings";
        let expected =
            CommonSubsequence {
                common_substrings: vec![
                    CommonSubstring { str1_offset: 0, str2_offset: 0, size_bytes: 22 }],
                size_bytes: 22, size_chars: 22 };
        assert_eq!(expected, get_longest_common_subsequence(&test_string, &test_string));
    }

    #[test]
    fn test_small_diff() {
        let test_string = "test string";
        let test_string2 = "test diff in middle string";
        let expected =
            CommonSubsequence {
                common_substrings: vec![
                    CommonSubstring { str1_offset: 0, str2_offset: 0, size_bytes: 5 },
                    CommonSubstring { str1_offset: 5, str2_offset: 20,size_bytes: 6 }],
                size_bytes: 11, size_chars: 11 };
        assert_eq!(expected, get_longest_common_subsequence(&test_string, &test_string2));
    }

    #[test]
    fn test_complicated_diff() {
        let test_string = "123456";
        let test_string2 = "124536";
        let expected = 
            CommonSubsequence {
                common_substrings: vec![
                    CommonSubstring { str1_offset: 0, str2_offset: 0, size_bytes: 2 },
                    CommonSubstring { str1_offset: 3, str2_offset: 2, size_bytes: 2 },
                    CommonSubstring { str1_offset: 5, str2_offset: 5, size_bytes: 1 }],
                size_bytes: 5, size_chars: 5 };
        assert_eq!(expected, get_longest_common_subsequence(&test_string, &test_string2));
    }

    #[test]
    fn test_no_characters_in_common() {
        let test_string = "abcdefg";
        let test_string2 = "12345678";
        let expected =
            CommonSubsequence { common_substrings: vec![], size_bytes: 0, size_chars: 0 };
        assert_eq!(expected, get_longest_common_subsequence(&test_string, &test_string2));
    }

    #[test]
    fn test_special_characters() {
        let test_string = "Test さよstring𐅃.";
        let test_string2 = "Test さようならstring.";
        let expected =
            CommonSubsequence {
                common_substrings: vec![
                    CommonSubstring { str1_offset: 0, str2_offset: 0, size_bytes: 11 },
                    CommonSubstring { str1_offset: 11, str2_offset: 20, size_bytes: 6 },
                    CommonSubstring { str1_offset: 21, str2_offset: 26, size_bytes: 1 }],
                size_bytes: 18, size_chars: 14 };
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
