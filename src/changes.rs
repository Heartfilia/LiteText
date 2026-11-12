//! Compact line fingerprints relative to the last successful disk save/read.
use std::hash::{Hash, Hasher};

pub(crate) fn fingerprint(text: &str) -> u64 {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hash);
    hash.finish()
}

pub(crate) fn lines(text: &str) -> Vec<u64> {
    text.split('\n').map(fingerprint).collect()
}

#[derive(Clone, Default)]
pub(crate) struct Changes {
    pub baseline: Option<Vec<u64>>,
    pub marked: Vec<bool>,
    pub revision: Option<u64>,
}

impl Changes {
    pub fn update(&mut self, current: &[u64], revision: u64) {
        if self.revision == Some(revision) {
            return;
        }
        self.revision = Some(revision);
        self.marked = vec![false; current.len()];
        let Some(baseline) = &self.baseline else {
            return;
        };
        if baseline.len() == current.len() {
            for (mark, (old, new)) in self.marked.iter_mut().zip(baseline.iter().zip(current)) {
                *mark = old != new;
            }
            return;
        }
        let prefix = baseline
            .iter()
            .zip(current)
            .take_while(|(a, b)| a == b)
            .count();
        let suffix = baseline[prefix..]
            .iter()
            .rev()
            .zip(current[prefix..].iter().rev())
            .take_while(|(a, b)| a == b)
            .count();
        let end = current.len() - suffix;
        self.marked[prefix..end].fill(true);
        // Align unchanged lines inside a small insert/delete region. Bound the LCS table
        // so a huge replacement cannot cause quadratic memory/time growth.
        let old = &baseline[prefix..baseline.len() - suffix];
        let new = &current[prefix..end];
        if !old.is_empty() && !new.is_empty() && old.len().saturating_mul(new.len()) <= 65_536 {
            let width = new.len() + 1;
            let mut table = vec![0u32; (old.len() + 1) * width];
            for a in (0..old.len()).rev() {
                for b in (0..new.len()).rev() {
                    table[a * width + b] = if old[a] == new[b] {
                        1 + table[(a + 1) * width + b + 1]
                    } else {
                        table[(a + 1) * width + b].max(table[a * width + b + 1])
                    };
                }
            }
            let (mut a, mut b) = (0, 0);
            while a < old.len() && b < new.len() {
                if old[a] == new[b] {
                    self.marked[prefix + b] = false;
                    a += 1;
                    b += 1;
                } else if table[(a + 1) * width + b] >= table[a * width + b + 1] {
                    self.marked[(prefix + b).min(current.len() - 1)] = true;
                    a += 1;
                } else {
                    b += 1;
                }
            }
        } else if !old.is_empty() && !new.is_empty() {
            // Match unique, ordered anchors across a large change region without an
            // unbounded diff table. Repeated ambiguous lines remain conservatively marked.
            let mut positions = std::collections::HashMap::new();
            for (i, &hash) in old.iter().enumerate() {
                positions
                    .entry(hash)
                    .and_modify(|p: &mut Option<usize>| *p = None)
                    .or_insert(Some(i));
            }
            let mut previous = None;
            for (i, hash) in new.iter().enumerate() {
                if let Some(Some(position)) = positions.get(hash)
                    && previous.is_none_or(|last| last < *position) {
                    self.marked[prefix + i] = false;
                    previous = Some(*position);
                }
            }
        }
        // A deleted whole line has no current line number: mark its adjacent surviving line.
        if prefix == end && baseline.len() != current.len() && !current.is_empty() {
            self.marked[prefix.min(current.len() - 1)] = true;
        }
    }

    pub fn is_marked(&self, line: usize) -> bool {
        self.marked.get(line).copied().unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_edits_insertions_deletions_and_clears_on_reversion() {
        let original = "a\r\n中文🙂\r\nc\r\n";
        let mut state = Changes {
            baseline: Some(lines(original)),
            ..Default::default()
        };
        state.update(&lines("a\r\nchanged\r\nc\r\n"), 1);
        assert_eq!(state.marked, [false, true, false, false]);
        state.update(&lines("a\r\nnew\n中文🙂\r\nc\r\n"), 2);
        assert_eq!(state.marked, [false, true, false, false, false]);
        state.update(&lines("a\r\nc\r\n"), 3);
        assert_eq!(state.marked, [false, true, false]);
        state.update(&lines(original), 4);
        assert!(state.marked.iter().all(|m| !m));
    }
}
