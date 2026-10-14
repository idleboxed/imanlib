// SPDX-License-Identifier: BSD-3-Clause
//! Read-only evdev acquisition. Never grabs devices or changes their keymaps.
use crate::{controls::Conflict, gamepad::profile::Profile};
use {
    crate::evdev::DeviceInfo,
    crate::evdev::Event,
};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{self, Read},
    os::{
        fd::AsRawFd,
        unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
};

pub const EVENT_BYTES: usize = 2 * std::mem::size_of::<libc::c_ulong>() + 8;

pub trait Pad {
    /// Borrow the checked metadata of this open input device.
    fn info(&self) -> &DeviceInfo;
    /// Read one nonblocking evdev event; None means no event is currently available.
    fn read(&mut self) -> io::Result<Option<Event>>;
    /// Read the currently held kernel key codes to establish or restore a baseline.
    fn keys_down(&mut self) -> io::Result<Vec<u16>>;
    /// Read current absolute-axis values, or report that snapshots are unsupported.
    fn axes_state(&mut self) -> io::Result<BTreeMap<u8, i32>> {
        Err(io::Error::other("axis snapshots unavailable"))
    }
}
pub trait Source {
    /// List permitted candidate device paths without acquiring menu ownership.
    fn discover(&mut self) -> io::Result<Vec<PathBuf>>;
    /// Open and validate one candidate read-only; the returned object owns its device resources.
    fn open(&mut self, path: &Path) -> io::Result<Box<dyn Pad>>;
}

pub struct ProfileSource {
    profiles: Vec<Profile>,
}
impl ProfileSource {
    /// Create a read-only source with profiles; devices are opened only by explicit source operations.
    pub fn new(profiles: Vec<Profile>) -> Self {
        Self { profiles }
    }
}
impl Source for ProfileSource {
    fn discover(&mut self) -> io::Result<Vec<PathBuf>> {
        discover(&self.profiles, false, &[])
    }
    fn open(&mut self, path: &Path) -> io::Result<Box<dyn Pad>> {
        open(path, &self.profiles, false, &[])
    }
}

/// Broader acquisition is confined to the visible manager-owned input test.
/// Normal navigation and the standalone probe still require an exact profile.
pub struct DiagnosticSource {
    profiles: Vec<Profile>,
    conflicts: Vec<Conflict>,
}
impl DiagnosticSource {
    /// Create a diagnostic source with known profiles and conflicts; construction does not open devices.
    pub fn new(profiles: Vec<Profile>, conflicts: Vec<Conflict>) -> Self { Self { profiles, conflicts } }
}
impl Source for DiagnosticSource {
    fn discover(&mut self) -> io::Result<Vec<PathBuf>> { discover(&self.profiles, true, &self.conflicts) }
    fn open(&mut self, path: &Path) -> io::Result<Box<dyn Pad>> { open(path, &self.profiles, true, &self.conflicts) }
}
struct Device {
    file: File,
    info: DeviceInfo,
}

fn query<T>(file: &File, nr: u8, value: &mut T) -> io::Result<()> {
    let request = 0x80000000u32
        | ((std::mem::size_of::<T>() as u32) << 16)
        | ((b'E' as u32) << 8)
        | u32::from(nr);
    // SAFETY: read-only Linux ioctl, correct initialized scalar/array storage.
    if unsafe { libc::ioctl(file.as_raw_fd(), request as _, value as *mut T) } < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
fn text_field(path: &Path) -> io::Result<String> {
    let mut bytes = Vec::new();
    File::open(path)?.take(513).read_to_end(&mut bytes)?;
    if bytes.len() > 512 {
        return Err(io::Error::other("identity too long"));
    }
    Ok(String::from_utf8_lossy(&bytes)
        .trim()
        .chars()
        .map(|character| if character.is_control() { '?' } else { character })
        .collect())
}
/// Read bounded sanitized diagnostic text, retaining an explicit error when unavailable.
pub(crate) fn small_text(path: &Path) -> String {
    text_field(path).unwrap_or_else(|error_item| format!("ERROR: {error_item}"))
}
fn event_name(name: &str) -> bool {
    name.strip_prefix("event")
        .is_some_and(|text_item| !text_item.is_empty() && text_item.len() <= 3 && text_item.bytes().all(|byte| byte.is_ascii_digit()))
}
struct InputIdentity {
    bus: u16,
    vendor: u16,
    product: u16,
    name: String,
}
impl InputIdentity {
    fn read(sys: &Path) -> io::Result<Self> {
        let device = sys.join("device");
        let number = |field| -> io::Result<u16> {
            let text = text_field(&device.join(field))?;
            u16::from_str_radix(&text, 16)
                .map_err(|_| io::Error::other(format!("invalid sysfs {field}")))
        };
        Ok(Self {
            bus: number("id/bustype")?, vendor: number("id/vendor")?,
            product: number("id/product")?, name: text_field(&device.join("name"))?,
        })
    }

    fn matches(&self, profiles: &[Profile]) -> usize {
        profiles.iter().filter(|profile_item| profile_item.matches(u32::from(self.bus), self.vendor, self.product, &self.name)).count()
    }
}

fn gamepad_capabilities(keys: &[u16], axes: &[u16]) -> bool {
    // Both joystick and gamepad collections occur in USB HID replicas.
    keys.iter().filter(|&&code| (288..=318).contains(&code)).count() >= 4
        && ((axes.contains(&0) && axes.contains(&1)) || (axes.contains(&16) && axes.contains(&17)))
}

/// Repeat the diagnostic capability predicate on ioctl metadata, not just sysfs.
pub fn diagnostic_gamepad(info: &DeviceInfo) -> bool {
    info.validate().is_ok() && info.identity.get("id/bustype").is_some_and(|bus|
        u16::from_str_radix(bus, 16) == Ok(3))
        && gamepad_capabilities(&info.key_codes, &info.axis_codes.iter().map(|&value_item| u16::from(value_item)).collect::<Vec<_>>())
}

// sysfs prints most-significant kernel unsigned-long first, unlike EVIOCGBIT.
fn capability_codes(path: &Path, limit: usize) -> io::Result<Vec<u16>> {
    let value = text_field(path)?;
    let words: Vec<_> = value.split_whitespace().collect();
    let width = usize::BITS as usize;
    if words.is_empty() || words.len() > limit.div_ceil(width) {
        return Err(io::Error::other("invalid sysfs capability bitmap length"));
    }
    let mut codes = Vec::new();
    for (index, word) in words.into_iter().rev().enumerate() {
        if word.len() > width / 4 || !word.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(io::Error::other("invalid sysfs capability bitmap word"));
        }
        let bits = usize::from_str_radix(word, 16)
            .map_err(|_| io::Error::other("invalid sysfs capability bitmap"))?;
        for bit in 0..width {
            if bits & (1usize << bit) != 0 {
                let code = index * width + bit;
                if code >= limit { return Err(io::Error::other("sysfs capability code exceeds limit")); }
                codes.push(code as u16);
            }
        }
    }
    Ok(codes)
}

fn eligibility(sys: &Path, identity: &InputIdentity, profiles: &[Profile], diagnostic: bool, conflicts: &[Conflict]) -> io::Result<&'static str> {
    if conflicts.iter().any(|conflict| conflict.backend == "evdev" && identity.bus == 3
        && (identity.vendor, identity.product, &identity.name) == (conflict.vendor, conflict.product, &conflict.name)) {
        return Ok("ambiguous_profiles");
    }
    match identity.matches(profiles) {
        1 => return Ok("profile_match"),
        2.. => return Ok("ambiguous_profiles"),
        _ => {}
    }
    if !diagnostic { return Ok("no_matching_profile"); }
    if identity.bus != 3 { return Ok("non_usb"); }
    let root = sys.join("device/capabilities");
    let ev = capability_codes(&root.join("ev"), 32)?;
    if !ev.contains(&1) || !ev.contains(&3) { return Ok("not_gamepad"); }
    let keys = capability_codes(&root.join("key"), 768)?;
    let axes = capability_codes(&root.join("abs"), 64)?;
    Ok(if gamepad_capabilities(&keys, &axes) { "gamepad_capabilities" } else { "not_gamepad" })
}

fn eligible(sys: &Path, profiles: &[Profile], diagnostic: bool, conflicts: &[Conflict]) -> bool {
    InputIdentity::read(sys).and_then(|identity| eligibility(sys, &identity, profiles, diagnostic, conflicts))
        .is_ok_and(|reason| matches!(reason, "profile_match" | "gamepad_capabilities"))
}

/// Passive discovery snapshot, not permission to open a device. Shares the exact
/// profile predicate with acquisition; never opens event nodes or reads serials.
/// Roots are explicit to permit synthetic sysfs fixtures without real hardware.
pub fn input_inventory(input: &Path, sysfs: &Path, profiles: &[Profile]) -> serde_json::Value {
    inventory(input, sysfs, profiles, false, &[])
}

/// Read a bounded device inventory with profile conflicts included, without acquiring input ownership.
pub fn diagnostic_input_inventory(input: &Path, sysfs: &Path, profiles: &[Profile], conflicts: &[Conflict]) -> serde_json::Value {
    inventory(input, sysfs, profiles, true, conflicts)
}

fn inventory(input: &Path, sysfs: &Path, profiles: &[Profile], diagnostic: bool, conflicts: &[Conflict]) -> serde_json::Value {
    use serde_json::json;
    let mut report = json!({"devices":[], "error":null, "entries_seen":0, "events_seen":0,
        "eligible_seen":0, "devices_omitted":0, "traversal_truncated":false, "profiles":profiles.len()});
    let mut entries = match fs::read_dir(input) {
        Ok(entries) => entries,
        Err(error) => {
            report["error"] = json!(error.to_string().chars().take(160).collect::<String>());
            return report;
        }
    };
    let mut names = Vec::new();
    for _ in 0..256 {
        let Some(entry) = entries.next() else { break; };
        match entry {
            Ok(entry) => {
                report["entries_seen"] = json!(report["entries_seen"].as_u64().unwrap() + 1);
                if entry.file_name().to_str().is_some_and(event_name) { names.push(entry.file_name()); }
            }
            Err(error) => {
                report["error"] = json!(error.to_string().chars().take(160).collect::<String>());
                break;
            }
        }
    }
    report["traversal_truncated"] = json!(entries.next().is_some());
    names.sort();
    report["events_seen"] = json!(names.len());
    for name in names {
        let sys = sysfs.join(&name);
        let mut item = json!({"node":name.to_string_lossy(), "reason":"identity_unavailable"});
        match InputIdentity::read(&sys) {
            Ok(identity) => {
                let matches = identity.matches(profiles);
                item["identity"] = json!({"bus":format!("{:04x}", identity.bus),
                    "vendor":format!("{:04x}", identity.vendor), "product":format!("{:04x}", identity.product),
                    "name":identity.name.chars().take(128).collect::<String>()});
                item["matching_profiles"] = json!(matches);
                match eligibility(&sys, &identity, profiles, diagnostic, conflicts) {
                    Ok(reason) => {
                        item["reason"] = json!(reason);
                        if matches!(reason, "profile_match" | "gamepad_capabilities") {
                            report["eligible_seen"] = json!(report["eligible_seen"].as_u64().unwrap() + 1);
                        }
                    }
                    Err(error) => {
                        item["reason"] = json!("capabilities_unavailable");
                        item["error"] = json!(error.to_string().chars().take(160).collect::<String>());
                    }
                }
            }
            Err(error) => item["error"] = json!(error.to_string().chars().take(160).collect::<String>()),
        }
        match fs::symlink_metadata(input.join(&name)) {
            Ok(meta) => item["node_type"] = json!(if meta.file_type().is_char_device() { "character" }
                else if meta.file_type().is_symlink() { "symlink" } else { "other" }),
            Err(error) => item["node_error"] = json!(error.to_string().chars().take(160).collect::<String>()),
        }
        let devices = report["devices"].as_array_mut().unwrap();
        if devices.len() < 16 { devices.push(item); }
        else { report["devices_omitted"] = json!(report["devices_omitted"].as_u64().unwrap() + 1); }
    }
    report
}

fn bits(bytes: &[u8]) -> Vec<u16> {
    (0..bytes.len() * 8)
        .filter(|&index_item| bytes[index_item / 8] & (1 << (index_item % 8)) != 0)
        .map(|index_item| index_item as u16)
        .collect()
}
fn discover(profiles: &[Profile], diagnostic: bool, conflicts: &[Conflict]) -> io::Result<Vec<PathBuf>> {
    if profiles.is_empty() && !diagnostic { return Ok(Vec::new()); }
    let entries = match fs::read_dir(crate::input::INPUT_DIR) {
        Ok(value_item) => value_item,
        Err(error_item) if error_item.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error_item) => return Err(error_item),
    };
    let mut paths = Vec::new();
    for (index_item, entry) in entries.enumerate() {
        if index_item >= 256 {
            return Err(io::Error::other("input directory exceeds diagnostic limit"));
        }
        let entry = entry?;
        if entry.file_name().to_str().is_some_and(event_name)
            && eligible(
                &Path::new("/sys/class/input").join(entry.file_name()),
                profiles,
                diagnostic,
                conflicts,
            )
        {
            paths.push(entry.path());
        }
    }
    paths.sort();
    Ok(paths)
}
fn open(path: &Path, profiles: &[Profile], diagnostic: bool, conflicts: &[Conflict]) -> io::Result<Box<dyn Pad>> {
    if !fs::symlink_metadata(path)?.file_type().is_char_device() {
        return Err(io::Error::other("not an evdev character device"));
    }
    let name = path
        .file_name()
        .and_then(|text_item| text_item.to_str())
        .filter(|text_item| event_name(text_item))
        .ok_or_else(|| io::Error::other("invalid event name"))?;
    let sys = Path::new("/sys/class/input").join(name);
    if !eligible(&sys, profiles, diagnostic, conflicts) {
        return Err(io::Error::other("not an eligible USB controller"));
    }
    let sys_identity = sys.join("device").canonicalize()?;
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)?;
    let before = file.metadata()?;
    if !before.file_type().is_char_device() {
        return Err(io::Error::other("not an evdev character device"));
    }
    if libc::major(before.rdev()) != 13 {
        return Err(io::Error::other("not a Linux input device"));
    }
    let mut id = [0u16; 4];
    query(&file, 0x02, &mut id)?;
    let mut name_buf = [0u8; 257];
    query(&file, 0x06, &mut name_buf)?;
    let name = String::from_utf8_lossy(name_buf.split(|&byte| byte == 0).next().unwrap_or_default())
        .into_owned();
    let matching = profiles
        .iter()
        .filter(|profile_item| profile_item.matches(u32::from(id[0]), id[1], id[2], &name)).count();
    if matching > 1 || (matching == 0 && !diagnostic) || name != InputIdentity::read(&sys)?.name {
        return Err(io::Error::other("evdev name mismatch"));
    }
    let mut ev = [0u8; 4];
    query(&file, 0x20, &mut ev)?;
    if ev[0] & 0x02 == 0 || (matching == 0 && ev[0] & 0x0a != 0x0a) {
        return Err(io::Error::other(
            "receiver has no EV_KEY/EV_ABS capabilities",
        ));
    }
    let mut keys = [0u8; 96];
    query(&file, 0x21, &mut keys)?;
    let mut axes = [0u8; 8];
    if ev[0] & 0x08 != 0 { query(&file, 0x23, &mut axes)?; }
    let axis_codes: Vec<u8> = bits(&axes).into_iter().map(|count_item| count_item as u8).collect();
    let mut abs_info = BTreeMap::new();
    for &code in &axis_codes {
        let mut info = [0i32; 6];
        query(&file, 0x40 + code, &mut info)?;
        abs_info.insert(code, info);
    }
    let mut identity = BTreeMap::new();
    for field in [
        "name",
        "phys",
        "uniq",
        "id/bustype",
        "id/vendor",
        "id/product",
        "id/version",
    ] {
        identity.insert(field.into(), small_text(&sys.join("device").join(field)));
    }
    identity.insert(
        "sysfs".into(),
        sys.join("device")
            .canonicalize()?
            .to_string_lossy()
            .into_owned(),
    );
    identity.insert("dev".into(), text_field(&sys.join("dev"))?);
    identity.insert(
        "ioctl_id".into(),
        format!("{:04x}:{:04x}:{:04x}:{:04x}", id[0], id[1], id[2], id[3]),
    );
    let dev = format!(
        "{}:{}",
        libc::major(before.rdev()),
        libc::minor(before.rdev())
    );
    if identity["dev"] != dev {
        return Err(io::Error::other("evdev/sysfs device number mismatch"));
    }
    if identity["name"] != name {
        return Err(io::Error::other("evdev/sysfs name mismatch"));
    }
    for (index_item, field) in ["id/bustype", "id/vendor", "id/product", "id/version"]
        .iter()
        .enumerate()
    {
        if identity[*field] != format!("{:04x}", id[index_item]) {
            return Err(io::Error::other("evdev/sysfs identity mismatch"));
        }
    }
    let after = fs::symlink_metadata(path)?;
    if before.rdev() != after.rdev()
        || before.ino() != after.ino()
        || before.dev() != after.dev()
        || sys.join("device").canonicalize()? != sys_identity
    {
        return Err(io::Error::other("evdev changed during identity read"));
    }
    let info = DeviceInfo {
        path: path.to_string_lossy().into_owned(),
        name,
        axis_codes,
        key_codes: bits(&keys),
        abs_info,
        identity,
    };
    info.validate().map_err(io::Error::other)?;
    if matching == 0 && !diagnostic_gamepad(&info) {
        return Err(io::Error::other("ioctl capabilities are not a USB gamepad"));
    }
    Ok(Box::new(Device { file, info }))
}
impl Pad for Device {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }
    fn read(&mut self) -> io::Result<Option<Event>> {
        let mut bytes = [0u8; EVENT_BYTES];
        match self.file.read(&mut bytes) {
            Ok(EVENT_BYTES) => Event::decode(&bytes)
                .map(Some)
                .ok_or_else(|| io::Error::other("invalid input_event")),
            Ok(_) => Err(io::Error::other("evdev disconnected or short event")),
            Err(error_item) if error_item.kind() == io::ErrorKind::WouldBlock => Ok(None),
            Err(error_item) => Err(error_item),
        }
    }
    fn keys_down(&mut self) -> io::Result<Vec<u16>> {
        let mut keys = [0u8; 96];
        query(&self.file, 0x18, &mut keys)?;
        Ok(bits(&keys))
    }
    fn axes_state(&mut self) -> io::Result<BTreeMap<u8, i32>> {
        let mut axes = BTreeMap::new();
        for &code in &self.info.axis_codes {
            let mut info = [0i32; 6];
            query(&self.file, 0x40 + code, &mut info)?;
            axes.insert(code, info[0]);
        }
        Ok(axes)
    }
}
