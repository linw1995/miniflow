use mf_telemetry::{
    Count,
    transport::{Completeness, DeliveryAdmission, DeliveryError, DeliveryTracker, MissingRange},
};

fn count(value: i64) -> Count {
    Count::try_from(value).unwrap()
}

#[test]
fn terminal_delivery_bounds_gaps_and_late_arrivals_close_them() {
    let mut delivery = DeliveryTracker::new(8);
    delivery.receive(count(6), "end", true).unwrap();
    delivery.receive(count(2), "second", false).unwrap();
    delivery.receive(count(4), "fourth", false).unwrap();
    let pending = delivery.snapshot(2);
    assert_eq!(pending.final_sequence, Some(count(6)));
    assert_eq!(pending.known_missing_count, 3);
    assert_eq!(
        pending.known_missing_ranges,
        [
            MissingRange { first: 1, last: 1 },
            MissingRange { first: 3, last: 3 }
        ]
    );
    assert_eq!(pending.omitted_missing_ranges, 1);
    assert_ne!(pending.completeness, Completeness::Complete);
    for sequence in [5, 3, 1] {
        delivery.receive(count(sequence), "late", false).unwrap();
    }
    assert_eq!(delivery.watermark(), 6);
    assert_eq!(delivery.snapshot(0).completeness, Completeness::Complete);
}

#[test]
fn a_contiguous_prefix_does_not_verify_an_absent_tail() {
    let mut delivery = DeliveryTracker::new(2);
    let empty = delivery.snapshot(64);
    assert_eq!(empty.known_missing_count, 0);
    assert_ne!(empty.completeness, Completeness::Complete);
    for sequence in 1..100 {
        delivery.receive(count(sequence), sequence, false).unwrap();
        if sequence > 2 {
            assert!(delivery.witness(count(sequence - 2)).is_none());
        }
    }
    let active = delivery.snapshot(64);
    assert_eq!(active.known_missing_count, 0);
    assert_eq!(active.final_sequence, None);
    assert_ne!(active.completeness, Completeness::Complete);
    delivery.receive(count(100), 100, true).unwrap();
    assert_eq!(delivery.snapshot(64).completeness, Completeness::Complete);
}

#[test]
fn retransmissions_preserve_identity_and_report_unverifiable_old_payloads() {
    let mut delivery = DeliveryTracker::new(2);
    for sequence in 1..=3 {
        assert_eq!(
            delivery.receive(count(sequence), sequence, false).unwrap(),
            DeliveryAdmission::New
        );
    }
    assert_eq!(
        delivery.receive(count(3), 3, false).unwrap(),
        DeliveryAdmission::Duplicate
    );
    assert_eq!(
        delivery.receive(count(3), 99, false).unwrap(),
        DeliveryAdmission::Conflict
    );
    assert_eq!(
        delivery.receive(count(1), 99, false).unwrap(),
        DeliveryAdmission::UnverifiedDuplicate
    );
    assert_eq!(delivery.watermark(), 3);
    delivery.receive(count(4), 4, true).unwrap();
    let snapshot = delivery.snapshot(64);
    assert_eq!(snapshot.known_missing_count, 0);
    assert_eq!(snapshot.protocol_conflicts, 1);
    assert_eq!(snapshot.unverified_retransmissions, 1);
    assert_ne!(snapshot.completeness, Completeness::Complete);
    assert_eq!(
        delivery.receive(count(1), 1, true).unwrap(),
        DeliveryAdmission::Conflict
    );
}

#[test]
fn final_boundaries_cannot_truncate_delivery_or_admit_a_later_suffix() {
    let mut delivery = DeliveryTracker::new(8);
    delivery.receive(count(4), "fourth", false).unwrap();
    assert_eq!(
        delivery.receive(count(3), "end", true).unwrap(),
        DeliveryAdmission::Conflict
    );
    assert_eq!(delivery.snapshot(64).final_sequence, None);
    delivery.receive(count(5), "end", true).unwrap();
    for (sequence, terminal) in [(6, false), (6, true), (2, true)] {
        assert_eq!(
            delivery
                .receive(count(sequence), "invalid", terminal)
                .unwrap(),
            DeliveryAdmission::Conflict
        );
    }
    assert_eq!(delivery.snapshot(64).final_sequence, Some(count(5)));
    assert_eq!(delivery.snapshot(64).known_missing_count, 3);
    assert_eq!(
        delivery.receive(count(5), "end", false).unwrap(),
        DeliveryAdmission::Conflict
    );
    assert_eq!(
        delivery.receive(count(4), "fourth", true).unwrap(),
        DeliveryAdmission::Conflict
    );
}

#[test]
fn a_full_reorder_window_accepts_the_next_contiguous_record() {
    let mut delivery = DeliveryTracker::new(2);
    delivery.receive(count(2), 2, false).unwrap();
    delivery.receive(count(3), 3, true).unwrap();
    delivery.receive(count(1), 1, false).unwrap();
    assert_eq!(delivery.watermark(), 3);
    assert_eq!(delivery.witness(count(1)), None);
    assert_eq!(delivery.witness(count(2)), Some(&2));
    assert_eq!(delivery.witness(count(3)), Some(&3));
    assert_eq!(delivery.snapshot(64).completeness, Completeness::Complete);
}

#[test]
fn exhausted_delivery_evidence_is_owned_by_transport() {
    let mut delivery = DeliveryTracker::new(2);
    delivery.receive(count(2), 2, false).unwrap();
    delivery.receive(count(4), 4, false).unwrap();
    assert!(matches!(
        delivery.receive(count(6), 6, true),
        Err(DeliveryError::ReorderWindowExhausted)
    ));
    let rejected = delivery.snapshot(64);
    assert!(rejected.verification_lost);
    assert_eq!(rejected.known_missing_count, 4);
    assert_eq!(rejected.final_sequence, None);
    for sequence in [1, 3, 5, 6] {
        delivery
            .receive(count(sequence), sequence, sequence == 6)
            .unwrap();
    }
    assert_eq!(delivery.snapshot(64).known_missing_count, 0);
    assert_ne!(delivery.snapshot(64).completeness, Completeness::Complete);
}

#[test]
fn sequence_limits_do_not_wrap_gap_counts_or_accept_zero() {
    let mut delivery = DeliveryTracker::new(2);
    assert!(matches!(
        delivery.receive(Count::ZERO, (), false),
        Err(DeliveryError::InvalidSequence)
    ));
    assert_eq!(delivery.snapshot(64).known_missing_count, 0);
    delivery.receive(count(i64::MAX), (), true).unwrap();
    let snapshot = delivery.snapshot(64);
    assert_eq!(snapshot.known_missing_count, (i64::MAX - 1) as u64);
    assert_eq!(
        snapshot.known_missing_ranges,
        [MissingRange {
            first: 1,
            last: i64::MAX - 1
        }]
    );
    assert_ne!(snapshot.completeness, Completeness::Complete);
}

#[test]
fn closing_collection_distinguishes_a_known_gap_from_an_unverified_tail() {
    let mut delivery = DeliveryTracker::new(4);
    delivery.receive(count(1), (), false).unwrap();
    assert_eq!(delivery.snapshot(64).completeness, Completeness::Collecting);
    delivery.receive(count(3), (), true).unwrap();
    assert_eq!(delivery.snapshot(64).completeness, Completeness::Collecting);
    delivery.close();
    let snapshot = delivery.snapshot(64);
    assert_eq!(snapshot.completeness, Completeness::Incomplete);
    assert_eq!(snapshot.known_missing_count, 1);
    assert!(matches!(
        delivery.receive(count(2), (), false),
        Err(DeliveryError::Closed)
    ));

    let mut abandoned = DeliveryTracker::new(4);
    abandoned.receive(count(1), (), false).unwrap();
    abandoned.close();
    let snapshot = abandoned.snapshot(64);
    assert_eq!(snapshot.completeness, Completeness::UnverifiedTail);
    assert_eq!(snapshot.known_missing_count, 0);
}

#[test]
fn local_rejections_remain_separate_from_sequence_gaps() {
    let mut delivery = DeliveryTracker::new(4);
    delivery.receive(count(1), (), false).unwrap();
    delivery.receive(count(2), (), true).unwrap();
    assert_eq!(delivery.snapshot(64).completeness, Completeness::Complete);
    delivery.record_local_drop(2);
    assert_eq!(delivery.snapshot(64).completeness, Completeness::Collecting);
    delivery.close();
    let snapshot = delivery.snapshot(64);
    assert_eq!(snapshot.completeness, Completeness::Incomplete);
    assert_eq!(snapshot.local_drops, 2);
    assert_eq!(snapshot.known_missing_count, 0);
}
