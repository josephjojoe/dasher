//! A bounded, append-only log used for captured console output and errors.

use std::collections::VecDeque;

/// Keeps the most recent `capacity` entries and counts the ones it dropped,
/// so a page that logs in a loop cannot grow host memory without bound.
#[derive(Debug)]
pub(crate) struct BoundedLog<T> {
    entries: VecDeque<T>,
    capacity: usize,
    dropped: u64,
}

impl<T> BoundedLog<T> {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            capacity,
            dropped: 0,
        }
    }

    pub(crate) fn push(&mut self, entry: T) {
        if self.capacity == 0 {
            self.dropped += 1;
            return;
        }
        if self.entries.len() == self.capacity {
            self.entries.pop_front();
            self.dropped += 1;
        }
        self.entries.push_back(entry);
    }

    /// Removes and returns every retained entry, oldest first.
    pub(crate) fn take(&mut self) -> Vec<T> {
        self.entries.drain(..).collect()
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &T> {
        self.entries.iter()
    }

    /// How many entries were discarded because the log was full.
    pub(crate) fn dropped(&self) -> u64 {
        self.dropped
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_the_newest_entries_and_counts_the_rest() {
        let mut log = BoundedLog::new(2);
        log.push(1);
        log.push(2);
        log.push(3);
        assert_eq!(log.iter().copied().collect::<Vec<_>>(), [2, 3]);
        assert_eq!(log.dropped(), 1);
        assert_eq!(log.take(), [2, 3]);
        assert_eq!(log.iter().count(), 0);
        // Taking the entries does not reset the drop count.
        assert_eq!(log.dropped(), 1);
    }

    #[test]
    fn zero_capacity_drops_everything() {
        let mut log = BoundedLog::new(0);
        log.push("a");
        assert_eq!(log.iter().count(), 0);
        assert_eq!(log.dropped(), 1);
    }
}
