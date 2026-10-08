// SPDX-License-Identifier: BSD-3-Clause
//! Shared Linux metric values and bounded readers; no observer or implicit I/O.
use serde::Serialize;
use std::{
    fs::File,
    io::{self, Read},
    path::Path,
};

pub const MEMINFO_LIMIT: usize = 64 * 1024;
pub const STAT_LIMIT: usize = 8192;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct MemoryInfo {
    /// Kernel estimate for new allocations, including reclaimable caches.
    pub available_bytes: u64,
    /// Usable RAM reported by Linux, not the installed physical capacity.
    pub total_bytes: u64,
}

impl MemoryInfo {
    pub fn parse(meminfo: &str) -> Option<Self> {
        if meminfo.len() > MEMINFO_LIMIT {
            return None;
        }
        let [available, total] = parse_kib_fields(meminfo, ["MemAvailable", "MemTotal"]).ok()?;
        let available_bytes = available?;
        let total_bytes = total?;
        (total_bytes > 0 && available_bytes <= total_bytes).then_some(Self {
            available_bytes,
            total_bytes,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Task {
    pub id: u32,
    pub start_ticks: u64,
    pub name: String,
    pub cpu_ticks: u64,
}

/// comm may contain spaces and parentheses; counters follow its LAST closing parenthesis.
pub fn parse_stat(text: &str) -> io::Result<Task> {
    let bad = || io::Error::other("invalid proc stat");
    if text.len() > STAT_LIMIT {
        return Err(bad());
    }
    let (id, rest) = text.split_once(" (").ok_or_else(bad)?;
    let (name, rest) = rest.rsplit_once(") ").ok_or_else(bad)?;
    let fields: Vec<_> = rest.split_ascii_whitespace().collect();
    if fields.len() < 20 || fields[0].len() != 1 {
        return Err(bad());
    }
    let id = id.parse::<u32>().map_err(|_| bad())?;
    if id == 0 {
        return Err(bad());
    }
    let number = |i: usize| fields[i].parse::<u64>().map_err(|_| bad());
    Ok(Task {
        id,
        start_ticks: number(19)?,
        name: name
            .chars()
            .take(16)
            .map(|c| if c.is_control() { '?' } else { c })
            .collect(),
        cpu_ticks: number(11)?.checked_add(number(12)?).ok_or_else(bad)?,
    })
}

/// Read procfs contents despite zero metadata length; reject oversized or non-UTF8 data.
pub fn read_text(path: &Path, limit: usize) -> io::Result<String> {
    let mut text = String::new();
    let bound = u64::try_from(limit)
        .ok()
        .and_then(|n| n.checked_add(1))
        .ok_or_else(|| io::Error::other("invalid metric input limit"))?;
    File::open(path)?.take(bound).read_to_string(&mut text)?;
    if text.len() > limit {
        return Err(io::Error::other("metric input exceeds limit"));
    }
    Ok(text)
}

pub fn read_memory(path: &Path) -> io::Result<MemoryInfo> {
    MemoryInfo::parse(&read_text(path, MEMINFO_LIMIT)?)
        .ok_or_else(|| io::Error::other("invalid meminfo"))
}

/// Selected meminfo counters. Missing fields stay unknown; malformed values fail.
pub fn parse_kib_fields<const N: usize>(text: &str, names: [&str; N]) -> io::Result<[Option<u64>; N]> {
    let bad = || io::Error::other("invalid meminfo counters");
    if text.len() > MEMINFO_LIMIT { return Err(bad()); }
    let mut values = [None; N];
    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else { continue; };
        let Some(index) = names.iter().position(|name| *name == key) else { continue; };
        let mut fields = value.split_whitespace();
        let bytes = fields.next().and_then(|s| s.parse::<u64>().ok())
            .and_then(|n| n.checked_mul(1024)).ok_or_else(bad)?;
        if fields.next() != Some("kB") || fields.next().is_some() || values[index].replace(bytes).is_some() {
            return Err(bad());
        }
    }
    Ok(values)
}

/// A failed or absent source must never look like a successful zero measurement.
#[derive(Clone, Debug, Serialize)]
pub struct Reading<T> {
    pub value: Option<T>,
    pub error: Option<String>,
}
impl<T> From<io::Result<T>> for Reading<T> {
    fn from(result: io::Result<T>) -> Self {
        match result {
            Ok(value) => Self { value: Some(value), error: None },
            Err(error) => Self { value: None, error: Some(error.to_string().chars().take(120)
                .map(|c| if c.is_control() { '?' } else { c }).collect()) },
        }
    }
}
