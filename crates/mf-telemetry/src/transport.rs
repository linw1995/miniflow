//! Bounded delivery evidence for a single sequenced transport stream.

use crate::Count;
use snafu::{Snafu, ensure};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MissingRange {
    pub first: i64,
    pub last: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeliverySnapshot {
    pub known_missing_count: u64,
    pub known_missing_ranges: Vec<MissingRange>,
    pub omitted_missing_ranges: usize,
    pub final_sequence: Option<Count>,
    pub verification_lost: bool,
    pub protocol_conflicts: u64,
    pub unverified_retransmissions: u64,
}

impl DeliverySnapshot {
    /// Absence of gaps alone cannot verify a wholly missing suffix.
    pub fn is_complete(&self) -> bool {
        self.final_sequence.is_some()
            && self.known_missing_count == 0
            && !self.verification_lost
            && self.protocol_conflicts == 0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeliveryAdmission {
    New,
    Duplicate,
    UnverifiedDuplicate,
    Conflict,
}

#[derive(Debug, Snafu)]
pub enum DeliveryError {
    #[snafu(display("transport sequences must be positive"))]
    InvalidSequence,
    #[snafu(display("transport reorder window exhausted"))]
    ReorderWindowExhausted,
}

/// Witnesses identify payloads, independently of their business semantics.
///
/// A contiguous prefix is compacted without losing delivery evidence. Retained
/// witnesses still verify retransmissions; older payloads cannot be compared.
pub struct DeliveryTracker<W> {
    witnesses: BTreeMap<i64, W>,
    capacity: usize,
    watermark: i64,
    highest: i64,
    final_sequence: Option<Count>,
    verification_lost: bool,
    protocol_conflicts: u64,
    unverified_retransmissions: u64,
}

impl<W: PartialEq> DeliveryTracker<W> {
    pub fn new(capacity: usize) -> Self {
        assert!(capacity > 0, "transport witness capacity must be positive");
        Self {
            witnesses: BTreeMap::new(),
            capacity,
            watermark: 0,
            highest: 0,
            final_sequence: None,
            verification_lost: false,
            protocol_conflicts: 0,
            unverified_retransmissions: 0,
        }
    }

    pub fn watermark(&self) -> i64 {
        self.watermark
    }

    pub fn witness(&self, sequence: Count) -> Option<&W> {
        self.witnesses.get(&sequence.get())
    }

    pub fn retained_witnesses(&self) -> usize {
        self.witnesses.len()
    }

    pub fn unverified_retransmissions(&self) -> u64 {
        self.unverified_retransmissions
    }

    fn check(
        &self,
        sequence: Count,
        witness: &W,
        terminal: bool,
    ) -> Result<DeliveryAdmission, DeliveryError> {
        let sequence = sequence.get();
        ensure!(sequence > 0, InvalidSequenceSnafu);
        if let Some(previous) = self.witnesses.get(&sequence) {
            return Ok(
                if previous == witness
                    && terminal == self.final_sequence.is_some_and(|end| end.get() == sequence)
                {
                    DeliveryAdmission::Duplicate
                } else {
                    DeliveryAdmission::Conflict
                },
            );
        }
        if self.final_sequence.is_some_and(|end| sequence > end.get())
            || (terminal && (self.final_sequence.is_some() || self.highest > sequence))
        {
            return Ok(DeliveryAdmission::Conflict);
        }
        if sequence <= self.watermark {
            return Ok(DeliveryAdmission::UnverifiedDuplicate);
        }
        let can_evict = self
            .witnesses
            .first_key_value()
            .is_some_and(|(&first, _)| first <= self.watermark);
        ensure!(
            self.witnesses.len() < self.capacity
                || can_evict
                || self.watermark.checked_add(1) == Some(sequence),
            ReorderWindowExhaustedSnafu
        );
        Ok(DeliveryAdmission::New)
    }

    pub fn receive(
        &mut self,
        sequence: Count,
        witness: W,
        terminal: bool,
    ) -> Result<DeliveryAdmission, DeliveryError> {
        let result = self.check(sequence, &witness, terminal);
        if matches!(result, Err(DeliveryError::ReorderWindowExhausted)) {
            self.highest = self.highest.max(sequence.get());
            self.verification_lost = true;
        }
        let admission = result?;
        match admission {
            DeliveryAdmission::Conflict => {
                self.protocol_conflicts = self.protocol_conflicts.saturating_add(1);
                return Ok(admission);
            }
            DeliveryAdmission::UnverifiedDuplicate => {
                self.unverified_retransmissions = self.unverified_retransmissions.saturating_add(1);
                return Ok(admission);
            }
            DeliveryAdmission::Duplicate => return Ok(admission),
            DeliveryAdmission::New => {}
        }
        self.highest = self.highest.max(sequence.get());
        if terminal {
            self.final_sequence = Some(sequence);
        }
        if self.witnesses.len() == self.capacity
            && self
                .witnesses
                .first_key_value()
                .is_some_and(|(&first, _)| first <= self.watermark)
        {
            self.witnesses.pop_first();
        }
        self.witnesses.insert(sequence.get(), witness);
        while let Some(next) = self.watermark.checked_add(1) {
            if !self.witnesses.contains_key(&next) {
                break;
            }
            self.watermark = next;
        }
        while self.witnesses.len() > self.capacity {
            self.witnesses.pop_first();
        }
        Ok(admission)
    }

    pub fn snapshot(&self, maximum_ranges: usize) -> DeliverySnapshot {
        let upper = self.final_sequence.map_or(self.highest, Count::get);
        let mut snapshot = DeliverySnapshot {
            known_missing_count: 0,
            known_missing_ranges: Vec::new(),
            omitted_missing_ranges: 0,
            final_sequence: self.final_sequence,
            verification_lost: self.verification_lost,
            protocol_conflicts: self.protocol_conflicts,
            unverified_retransmissions: self.unverified_retransmissions,
        };
        let mut gap = |first, last| {
            snapshot.known_missing_count += (last - first + 1) as u64;
            if snapshot.known_missing_ranges.len() < maximum_ranges {
                snapshot
                    .known_missing_ranges
                    .push(MissingRange { first, last });
            } else {
                snapshot.omitted_missing_ranges += 1;
            }
        };
        if self.watermark < upper {
            let mut cursor = self.watermark + 1;
            for &sequence in self.witnesses.range(cursor..=upper).map(|(key, _)| key) {
                if sequence > cursor {
                    gap(cursor, sequence - 1);
                }
                // The last sequence may be i64::MAX.
                if sequence == upper {
                    return snapshot;
                }
                cursor = sequence + 1;
            }
            gap(cursor, upper);
        }
        snapshot
    }
}
