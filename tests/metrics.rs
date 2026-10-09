// SPDX-License-Identifier: BSD-3-Clause
use imanlib::metrics::{parse_stat, read_memory, read_text, MemoryInfo, MEMINFO_LIMIT, STAT_LIMIT};
use std::fs;

#[test]
fn shared_memory_reader_is_bounded_read_only_and_reports_invalid_files() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("meminfo");
    let contents = "MemTotal: 100 kB\nMemAvailable: 40 kB\n";

    assert!(read_memory(&path).is_err());
    assert!(read_memory(dir.path()).is_err());
    fs::write(&path, contents).unwrap();
    assert_eq!(
        read_memory(&path).unwrap(),
        MemoryInfo {
            total_bytes: 100 * 1024,
            available_bytes: 40 * 1024,
        }
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), contents);
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    assert!(read_text(&path, 2).is_err());
    fs::write(&path, "invalid meminfo\n").unwrap();
    assert_eq!(read_memory(&path).unwrap_err().to_string(), "invalid meminfo");
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
fn memory_info_uses_available_not_free_and_accepts_field_order_and_zero() {
    for (contents, available_kib) in [
        (
            "MemTotal: 256 kB\nMemFree: 64 kB\nMemAvailable: 128 kB\n",
            128,
        ),
        (
            "MemAvailable:\t0 kB\nUnknown: arbitrary\nMemFree: 64 kB\nMemTotal: 256 kB\n",
            0,
        ),
    ] {
        assert_eq!(
            MemoryInfo::parse(contents),
            Some(MemoryInfo {
                available_bytes: available_kib * 1024,
                total_bytes: 256 * 1024,
            }),
            "{contents}"
        );
    }
}

#[test]
fn unavailable_or_malformed_memory_info_is_unknown_not_a_free_memory_estimate() {
    for contents in [
        "",
        "MemTotal: 100 kB\nMemFree: 80 kB\nCached: 10 kB\n",
        "MemAvailable: 80 kB\n",
        "MemTotal: 0 kB\nMemAvailable: 0 kB\n",
        "MemTotal: 100 kB\nMemAvailable: 101 kB\n",
        "MemTotal: 100 kB\nMemAvailable: -1 kB\n",
        "MemTotal: 100 kB\nMemAvailable: bad kB\n",
        "MemTotal: 100\nMemAvailable: 10 kB\n",
        "MemTotal: 100 MB\nMemAvailable: 10 kB\n",
        "MemTotal: 100 kB\nMemAvailable: 10 KiB\n",
        "MemTotal: 100 kB\nMemAvailable: 10 kB extra\n",
        "MemTotal: 100 kB\nMemTotal: 100 kB\nMemAvailable: 10 kB\n",
        "MemTotal: 100 kB\nMemAvailable: 10 kB\nMemAvailable: 10 kB\n",
        "MemTotal: 18446744073709551615 kB\nMemAvailable: 10 kB\n",
        "MemTotal: 100 kB\nMemAvailable: 18446744073709551616 kB\n",
    ] {
        assert_eq!(MemoryInfo::parse(contents), None, "{contents}");
    }
}

#[cfg(target_os = "linux")]
#[test]
fn live_linux_procfs_is_readable_despite_its_zero_metadata_length() {
    let path = std::path::Path::new("/proc/meminfo");
    assert_eq!(fs::metadata(path).unwrap().len(), 0);

    let info = read_memory(path).expect("Linux memory telemetry");

    assert!(info.total_bytes > 0);
    assert!(info.available_bytes <= info.total_bytes);
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
