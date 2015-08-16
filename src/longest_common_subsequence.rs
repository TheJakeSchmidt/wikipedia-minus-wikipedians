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
//! longest-common-subsequence for a given pair of regions starting at 0, and the work to be done is
//! the 3 steps:
//!
//! 1. Slide down the diagonal (i.e., move forward in both iterators) as far as possible
//! 2. Enqueue a new task to try going right (i.e., farther in iterator 1)
//! 3. Enqueue a new task to try going down (i.e., farther in iterator 2)
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

#[derive(PartialEq, Clone, Debug)]
pub struct CommonRegion {
    /// The offset into iter1 of the beginning of the common region.
    pub iter1_offset: usize,
    /// The offset into iter2 of the beginning of the common region.
    pub iter2_offset: usize,
    /// The length of the region.
    pub size: usize,
}

impl CommonRegion {
    pub fn new(iter1_offset: usize, iter2_offset: usize, size: usize) -> CommonRegion {
        CommonRegion {
            iter1_offset: iter1_offset,
            iter2_offset: iter2_offset,
            size: size,
        }
    }
}

#[derive(PartialEq, Clone, Debug)]
pub struct CommonSubsequence {
    pub common_regions: Vec<CommonRegion>,
    /// The total length of the common subsequence.
    pub size: usize,
}

impl CommonSubsequence {
    pub fn new(common_regions: Vec<CommonRegion>) -> CommonSubsequence {
        let size = (&common_regions).into_iter().fold(0, |sum, region| sum + region.size);
        CommonSubsequence {
            common_regions: common_regions,
            size: size,
        }
    }
}

/// A Task represents a step of the algorithm that needs to be done. A Task records a possible
/// longest common subsequence up to a particular offset in each iterator. Executing a Task means
/// moving as far forward in both iterators as possible (for as long as they match, starting at the
/// Task's offsets), then enqueuing Tasks to try moving one item farther in each iterator.
struct Task<T, I> where I: Iterator<Item=T> + Clone {
    /// The highest offset in iter1 which has been searched
    iter1_offset: usize,
    /// The highest offset in iter2 which has been searched
    iter2_offset: usize,
    iter1: I,
    iter2: I,
    /// The common subsequence which is known so far.
    common_subsequence: CommonSubsequence,
}

impl<T, I> PartialEq for Task<T, I> where I: Iterator<Item=T> + Clone {
    fn eq(&self, other: &Task<T, I>) -> bool {
        self.iter1_offset == other.iter1_offset &&
            self.iter2_offset == other.iter2_offset &&
            //self.iter1.clone().next() == other.iter1.clone().next() &&
            //self.iter2.clone().next() == other.iter2.clone().next() &&
            self.common_subsequence == other.common_subsequence
    }
}

impl<T, I> Eq for Task<T, I> where I: Iterator<Item=T> + Clone {}

impl<T, I> PartialOrd for Task<T, I> where I: Iterator<Item=T> + Clone {
    fn partial_cmp(&self, other: &Task<T, I>) -> Option<Ordering> {
        return Some(self.cmp(other));
    }
}

impl<T, I> Ord for Task<T, I> where I: Iterator<Item=T> + Clone {
    fn cmp(&self, other: &Task<T, I>) -> Ordering {
        // This value is -1 times the edit distance implied by the task's common subsequence: the
        // edit distance between two strings (in this case, iterators) with no matching characters
        // (items) is iter1_offset + iter2_offset (with the edit algorithm being to delete each
        // character/item in one string/iterator and insert each character/item in the other), and
        // each matching character/item decreases that by two (because it saves one deletion and one
        // insertion).
        //
        // The lowest (implied) edit distance makes for the best priority function here for either of
        // two reasons:
        //
        // - Conceptualizing this algorithm as a version of the algorithm in Miller and Myers 1985:
        // using the edit distance is identical to the algorithm in the paper, which starts at edit
        // distance 0 and finds how far into the two strings it's possible to go with each edit
        // distance.
        // - Conceptualizing this algorithm as an implementation of A* on a grid like in the figures
        // in Miller and Myers 1975: this heuristic is admissible because (-iter1_offset +
        // -iter2_offset) is the negative Manhattan distance to the goal, plus a constant (the
        // Manhattan distance from the start node to the goal node).
        let self_value = self.common_subsequence.size as i64 * 2
            - self.iter1_offset as i64 - self.iter2_offset as i64;
        let other_value = other.common_subsequence.size as i64 * 2
            - other.iter1_offset as i64 - other.iter2_offset as i64;
        if self_value > other_value {
            return Ordering::Greater;
        } else if self_value < other_value {
            return Ordering::Less;
        }

        // For tasks that are doing equally well by the above calculation, we prefer the one whose
        // offsets are closer together. Since the diffs tend to be small relative to text size for
        // Wikipedia articles, such tasks are more likely to end up winners.
        let self_offset_diff = num::abs(self.iter2_offset as i64 - self.iter1_offset as i64);
        let other_offset_diff = num::abs(other.iter2_offset as i64 - other.iter1_offset as i64);
        if self_offset_diff < other_offset_diff {
            return Ordering::Greater; // If self is closer, its value is greater
        } else if self_offset_diff > other_offset_diff {
            return Ordering::Less;
        }

        // For tasks that are doing equally well by the above two calculations, we prefer the one
        // that is farther along, under the assumption that it's more likely to be a winner.
        if self.iter1_offset + self.iter2_offset > other.iter1_offset + other.iter2_offset {
            return Ordering::Greater;
        } else if self.iter1_offset + self.iter2_offset < other.iter1_offset + other.iter2_offset {
            return Ordering::Less;
        }

        // If none of the above calculations could differentiate between the two, we
        // descend into arbitrary metrics.
        if self.iter1_offset > other.iter1_offset {
            return Ordering::Greater;
        } else if self.iter1_offset < other.iter1_offset {
            return Ordering::Less;
        } else if self.iter2_offset > other.iter2_offset {
            return Ordering::Greater;
        } else if self.iter2_offset < other.iter2_offset {
            return Ordering::Less;
        } else {
            // At this point, the two are at the same offsets, with the same common subsequence
            // size. The one who has a bigger common region earliest wins, for no good reason.
            for (self_common_subsequence, other_common_subsequence) in
                (&self.common_subsequence.common_regions).into_iter().zip(
                    (&other.common_subsequence.common_regions).into_iter()) {
                if self_common_subsequence.size > other_common_subsequence.size {
                    return Ordering::Greater;
                } else {
                    return Ordering::Less;
                }
            }
        }
        Ordering::Equal
    }
}

/// Returns None if the calculation takes more than `time_limit_ms` milliseconds.
pub fn get_longest_common_subsequence<T, I>(iter1: I, iter2: I, time_limit_ms: u64) -> Option<CommonSubsequence>
    where I: Iterator<Item=T> + Clone,
          T: Eq {
    let timeout_ns = time::precise_time_ns() + time_limit_ms * 1_000_000;

    let mut work_queue: BinaryHeap<Task<T, I>> = BinaryHeap::new();
    let first_task =
        Task {
            iter1_offset: 0,
            iter2_offset: 0,
            common_subsequence: CommonSubsequence::new(vec![]),
            iter1: iter1,
            iter2: iter2,
        };
    work_queue.push(first_task);

    // Tracks the size of the longest common subsequence that's known so far up to each combination
    // of iter1_offset and iter2_offset. A Task whose common_subsequence does not have a size greater
    // than the corresponding value in this HashMap will not be inserted into the work queue.
    let mut longest_known_common_subsequences: HashMap<(usize, usize), usize> = HashMap::new();

    loop {
        if time::precise_time_ns() > timeout_ns {
            return None;
        }

        let mut task = work_queue.pop().unwrap();

        // 1. Move forward in both iterators for as long as they match.
        let mut matching_items = 0;
        let mut iter1_finished = false;
        let mut iter2_finished = false;
        loop {
            match (task.iter1.clone().next(), task.iter2.clone().next()) {
                (Some(ref iter1_item), Some(ref iter2_item)) if iter1_item == iter2_item => {
                    matching_items += 1;
                    // We can only advance either iterator when we advance both iterators, so we
                    // clone them in the match condition and then advance them here. If we were to
                    // advance them both in the match condition above, when we reached the end of
                    // one iterator but not the other, we'd advance twice before cloning the iterator
                    // for the new task, and miss an offset.
                    task.iter1.next();
                    task.iter2.next();
                },
                (None, None) => {
                    // We've reached the end of both iterators, and have our answer.
                    iter1_finished = true;
                    iter2_finished = true;
                    break;
                },
                (Some(..), None) => {
                    iter2_finished = true;
                    break;
                },
                (None, Some(..)) => {
                    iter1_finished = true;
                    break;
                },
                _ => {
                    break;
                },
            }
        }

        // 2. Add a new common region to the common subsequence if one of non-zero size was
        // found.
        let mut new_common_subsequence = task.common_subsequence.clone();
        if matching_items > 0 {
            new_common_subsequence.common_regions.push(
                CommonRegion::new(task.iter1_offset, task.iter2_offset, matching_items));
            new_common_subsequence.size += matching_items;
        }

        if iter1_finished && iter2_finished {
            return Some(new_common_subsequence);
        }

        // 3a. Enqueue another task in the work queue that starts one item farther into iter1 and at
        // the same offset into iter2.
        let new_iter1_offset = task.iter1_offset + matching_items;
        let new_iter2_offset = task.iter2_offset + matching_items;
        if !iter1_finished {
            match longest_known_common_subsequences.get(&(new_iter1_offset + 1, new_iter2_offset)) {
                Some(size) if size >= &new_common_subsequence.size => (),
                _ => {
                    let mut new_iter1 = task.iter1.clone();
                    new_iter1.next().unwrap();
                    work_queue.push(
                        Task {
                            iter1_offset: new_iter1_offset + 1,
                            iter2_offset: new_iter2_offset,
                            common_subsequence: new_common_subsequence.clone(),
                            iter1: new_iter1,
                            iter2: task.iter2.clone(),
                        });
                }
            }
            // This separate block is necessary because of issue 6393 - I can't insert() into
            // longest_known_common_subsequences in the match block on the get().
            match longest_known_common_subsequences.entry((new_iter1_offset + 1, new_iter2_offset)) {
                Entry::Occupied(ref entry) if entry.get() >= &new_common_subsequence.size => (),
                Entry::Occupied(mut entry) => { entry.insert(new_common_subsequence.size); }
                Entry::Vacant(entry) => { entry.insert(new_common_subsequence.size); }
            }

        }

        // 3b. Enqueue another task in the work queue that starts at the same offset into iter1 and
        // one item farther into iter2.
        if !iter2_finished {
            match longest_known_common_subsequences.get(&(new_iter1_offset, new_iter2_offset + 1)) {
                Some(size) if size >= &new_common_subsequence.size => (),
                _ => {
                    let mut new_iter2 = task.iter2.clone();
                    new_iter2.next().unwrap();
                    work_queue.push(
                        Task {
                            iter1_offset: new_iter1_offset,
                            iter2_offset: new_iter2_offset + 1,
                            common_subsequence: new_common_subsequence.clone(),
                            iter1: task.iter1.clone(),
                            iter2: new_iter2,
                        });
                },
            }
            // This separate block is necessary because of issue 6393 - I can't insert() into
            // longest_known_common_subsequences in a match block for
            // longest_known_common_subsequences.get().
            match longest_known_common_subsequences.entry((new_iter1_offset, new_iter2_offset + 1)) {
                Entry::Occupied(ref entry) if entry.get() >= &new_common_subsequence.size =>
                    (),
                Entry::Occupied(mut entry) => { entry.insert(new_common_subsequence.size); }
                Entry::Vacant(entry) => { entry.insert(new_common_subsequence.size); }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{get_longest_common_subsequence, CommonSubsequence, CommonRegion};

    #[test]
    fn test_lcs_identical_strings() {
        let test_string = "test identical strings";
        let expected = CommonSubsequence::new(vec![CommonRegion::new(0, 0, 22)]);
        assert_eq!(Some(expected),
                   get_longest_common_subsequence(test_string.chars(), test_string.chars()));
    }

    #[test]
    fn test_lcs_diff_in_middle() {
        let test_string = "test string";
        let test_string2 = "test diff in middle string";
        let expected =
            CommonSubsequence::new(vec![CommonRegion::new(0, 0, 5), CommonRegion::new(5, 20, 6)]);
        assert_eq!(Some(expected),
                   get_longest_common_subsequence(test_string.chars(), test_string2.chars()));
    }

    #[test]
    fn test_lcs_complicated_diff() {
        let test_string = "123456";
        let test_string2 = "124536";
        let expected =
            CommonSubsequence::new(vec![CommonRegion::new(0, 0, 2), CommonRegion::new(3, 2, 2),
                                        CommonRegion::new(5, 5, 1)]);
        assert_eq!(Some(expected),
                   get_longest_common_subsequence(test_string.chars(), test_string2.chars()));
    }

    #[test]
    fn test_lcs_no_words_in_common() {
        let test_string = "abcdefg";
        let test_string2 = "12345678";
        assert_eq!(Some(CommonSubsequence::new(vec![])),
                   get_longest_common_subsequence(test_string.chars(), test_string2.chars()));
    }

    #[test]
    fn test_lcs_special_characters() {
        let test_string = "Test さよstring𐅃.";
        let test_string2 = "Test さようなら string.";
        let expected =
            CommonSubsequence::new(
                vec![CommonRegion::new(0, 0, 7), CommonRegion::new(7, 11, 6),
                     CommonRegion::new(14, 17, 1)]);
        assert_eq!(Some(expected),
                   get_longest_common_subsequence(test_string.chars(), test_string2.chars()));
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
