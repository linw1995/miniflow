//! Bounded delivery evidence for a single sequenced transport stream.

use crate::{ContractError, Count, require};
use snafu::{Snafu, ensure};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Completeness {
    Collecting,
    Complete,
    Incomplete,
    UnverifiedTail,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MissingRange {
    pub first: i64,
    pub last: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeliverySnapshot {
    pub completeness: Completeness,
    pub known_missing_count: u64,
    pub known_missing_ranges: Vec<MissingRange>,
    pub omitted_missing_ranges: usize,
    pub final_sequence: Option<Count>,
    pub verification_lost: bool,
    pub protocol_conflicts: u64,
    pub unverified_retransmissions: u64,
    pub local_drops: u64,
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
    #[snafu(display("transport has finished collecting"))]
    Closed,
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
    local_drops: u64,
    closed: bool,
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
            local_drops: 0,
            closed: false,
        }
    }

    pub fn watermark(&self) -> i64 {
        self.watermark
    }

    pub fn witness(&self, sequence: Count) -> Option<&W> {
        self.witnesses.get(&sequence.get())
    }

    pub fn record_local_drop(&mut self, count: u64) {
        self.local_drops = self.local_drops.saturating_add(count);
    }

    pub fn close(&mut self) {
        self.closed = true;
    }

    pub fn receive(
        &mut self,
        sequence: Count,
        witness: W,
        terminal: bool,
    ) -> Result<DeliveryAdmission, DeliveryError> {
        let value = sequence.get();
        ensure!(!self.closed, ClosedSnafu);
        ensure!(value > 0, InvalidSequenceSnafu);
        let admission = if let Some(previous) = self.witnesses.get(&value) {
            if previous == &witness && terminal == (self.final_sequence == Some(sequence)) {
                DeliveryAdmission::Duplicate
            } else {
                DeliveryAdmission::Conflict
            }
        } else if self.final_sequence.is_some_and(|end| sequence > end)
            || (terminal && (self.final_sequence.is_some() || self.highest > value))
        {
            DeliveryAdmission::Conflict
        } else if value <= self.watermark {
            DeliveryAdmission::UnverifiedDuplicate
        } else {
            DeliveryAdmission::New
        };
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
        self.highest = self.highest.max(value);
        let can_evict = self
            .witnesses
            .first_key_value()
            .is_some_and(|(&first, _)| first <= self.watermark);
        if self.witnesses.len() == self.capacity
            && !can_evict
            && self.watermark.checked_add(1) != Some(value)
        {
            self.verification_lost = true;
            return ReorderWindowExhaustedSnafu.fail();
        }
        if terminal {
            self.final_sequence = Some(sequence);
        }
        self.witnesses.insert(sequence.get(), witness);
        while let Some(next) = self.watermark.checked_add(1) {
            if !self.witnesses.contains_key(&next) {
                break;
            }
            self.watermark = next;
        }
        if self.witnesses.len() > self.capacity {
            self.witnesses.pop_first();
        }
        Ok(admission)
    }

    pub fn snapshot(&self, maximum_ranges: usize) -> DeliverySnapshot {
        let upper = self.final_sequence.map_or(self.highest, Count::get);
        let mut snapshot = DeliverySnapshot {
            completeness: Completeness::Collecting,
            known_missing_count: 0,
            known_missing_ranges: Vec::new(),
            omitted_missing_ranges: 0,
            final_sequence: self.final_sequence,
            verification_lost: self.verification_lost,
            protocol_conflicts: self.protocol_conflicts,
            unverified_retransmissions: self.unverified_retransmissions,
            local_drops: self.local_drops,
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
                cursor = sequence.saturating_add(1);
            }
            if !self.witnesses.contains_key(&upper) {
                gap(cursor, upper);
            }
        }
        snapshot.completeness = match (snapshot.final_sequence, self.closed) {
            (None, false) => Completeness::Collecting,
            (None, true) => Completeness::UnverifiedTail,
            (Some(_), _)
                if snapshot.known_missing_count == 0
                    && snapshot.local_drops == 0
                    && !snapshot.verification_lost
                    && snapshot.protocol_conflicts == 0 =>
            {
                Completeness::Complete
            }
            (Some(_), false) => Completeness::Collecting,
            (Some(_), true) => Completeness::Incomplete,
        };
        snapshot
    }
}

/// Reserves delivery identity before encoding, independently of workflow structure.
#[derive(Debug)]
pub struct EventSequence {
    next: Option<i64>,
    closed: bool,
}

impl Default for EventSequence {
    fn default() -> Self {
        Self::new()
    }
}

impl EventSequence {
    pub fn new() -> Self {
        Self {
            next: Some(1),
            closed: false,
        }
    }

    pub fn reserve(&mut self) -> Result<Count, ContractError> {
        require(!self.closed, "delivery sequence is closed")?;
        let value = self
            .next
            .ok_or_else(|| crate::invalid("delivery sequence is exhausted"))?;
        self.next = value.checked_add(1);
        Count::try_from(value)
    }

    pub fn finish(&mut self) -> Result<Count, ContractError> {
        require(self.next != Some(1), "delivery sequence has not started")?;
        let sequence = self.reserve()?;
        self.closed = true;
        Ok(sequence)
    }
}

#[cfg(test)]
mod sequence_tests {
    use super::*;

    #[test]
    fn finalizes_or_reports_exhaustion_without_wrapping() {
        let mut terminal = EventSequence {
            next: Some(i64::MAX),
            closed: false,
        };
        assert_eq!(terminal.finish().unwrap().get(), i64::MAX);
        assert!(terminal.reserve().is_err());
        assert!(terminal.finish().is_err());
        let mut exhausted = EventSequence {
            next: Some(i64::MAX),
            closed: false,
        };
        assert_eq!(exhausted.reserve().unwrap().get(), i64::MAX);
        assert!(exhausted.reserve().is_err());
        assert!(exhausted.finish().is_err());
    }
}
