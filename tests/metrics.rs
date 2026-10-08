// SPDX-License-Identifier: BSD-3-Clause
use imanlib::metrics::{parse_stat, read_memory, read_text, MemoryInfo, MEMINFO_LIMIT, STAT_LIMIT};
use std::fs;

#[test]
fn shared_memory_reader_has_explicit_errors_and_binary_units_without_gui_formatting() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("meminfo");
    assert!(read_memory(&path).is_err());
    fs::write(&path, "MemTotal: 100 kB\nMemAvailable: 0 kB\n").unwrap();
    assert_eq!(
        read_memory(&path).unwrap(),
        MemoryInfo {
            total_bytes: 102400,
            available_bytes: 0
        }
    );
    assert!(read_text(&path, 2).is_err());
    fs::write(&path, [0xff, 0xfe]).unwrap();
    assert!(read_memory(&path).is_err());
    let huge = format!(
        "MemTotal: 100 kB\nMemAvailable: 1 kB\n{}",
        " ".repeat(MEMINFO_LIMIT)
    );
    assert!(MemoryInfo::parse(&huge).is_none());
    fs::write(&path, huge).unwrap();
    assert!(read_memory(&path).is_err());
    assert!(parse_stat(&"x".repeat(STAT_LIMIT + 1)).is_err());
}

#[test]
fn selected_meminfo_counters_distinguish_missing_zero_invalid_and_overflow() {
    use imanlib::metrics::parse_kib_fields;
    assert_eq!(parse_kib_fields("Dirty: 0 kB\nWriteback: 2 kB\n", ["Dirty", "Writeback"]).unwrap(), [Some(0), Some(2048)]);
    assert_eq!(parse_kib_fields("MemTotal: 8 kB\n", ["Dirty", "Writeback"]).unwrap(), [None, None]);
    for text in ["Dirty: 1 kB\nDirty: 2 kB\n", "Dirty: -1 kB\n", "Dirty: 1 KB\n", "Dirty: 1 kB extra\n", "Dirty: 18446744073709551615 kB\n"] {
        assert!(parse_kib_fields(text, ["Dirty", "Writeback"]).is_err(), "{text}");
    }
}

#[test]
fn unavailable_metric_never_becomes_zero_or_unbounded_error_text() {
    use imanlib::metrics::Reading;
    let good: Reading<u64> = Ok(0).into();
    assert_eq!(good.value, Some(0));
    assert!(good.error.is_none());
    let bad: Reading<u64> = Err(std::io::Error::other("\n".repeat(1024))).into();
    assert!(bad.value.is_none());
    let message = bad.error.unwrap();
    assert_eq!(message.len(), 120);
    assert!(!message.contains('\n'));
}
