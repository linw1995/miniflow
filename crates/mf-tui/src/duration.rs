pub fn format_duration_ns(ns: u64) -> String {
    match ns {
        0..1_000 => format!("{ns}ns"),
        1_000..1_000_000 => format!("{:.1}µs", ns as f64 / 1_000.0),
        1_000_000..1_000_000_000 => format!("{:.1}ms", ns as f64 / 1_000_000.0),
        1_000_000_000..60_000_000_000 => format!("{:.1}s", ns as f64 / 1_000_000_000.0),
        60_000_000_000..3_600_000_000_000 => {
            format!("{:.1}min", ns as f64 / 60_000_000_000.0)
        }
        _ => format!("{:.1}h", ns as f64 / 3_600_000_000_000.0),
    }
}

#[cfg(test)]
mod tests {
    use super::format_duration_ns;

    #[test]
    fn chooses_a_unit_without_erasing_short_node_durations() {
        for (ns, expected) in [
            (42, "42ns"),
            (42_000, "42.0µs"),
            (42_000_000, "42.0ms"),
            (4_200_000_000, "4.2s"),
            (120_000_000_000, "2.0min"),
            (7_200_000_000_000, "2.0h"),
        ] {
            assert_eq!(format_duration_ns(ns), expected);
        }
    }
}
