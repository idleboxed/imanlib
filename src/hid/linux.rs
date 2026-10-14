// SPDX-License-Identifier: BSD-3-Clause
//! Read-only hidraw acquisition, pinned to an opened FD and sysfs generation.
use super::{
    invalid,
    receiver::{Device, Info, Source},
    Profile, MAX_DESCRIPTOR_BYTES,
};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read},
    os::{
        fd::AsRawFd,
        unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
};

pub struct LinuxSource {
    profiles: Vec<Profile>,
}
impl LinuxSource {
    /// Create a read-only HID source with profiles; construction does not open devices.
    pub fn new(profiles: Vec<Profile>) -> Self {
        Self { profiles }
    }
}
struct Raw {
    file: File,
    info: Info,
}

fn text(path: &Path) -> io::Result<String> {
    let mut bytes = Vec::new();
    File::open(path)?.take(4097).read_to_end(&mut bytes)?;
    if bytes.len() > 4096 {
        return Err(invalid("HID identity exceeds limit"));
    }
    String::from_utf8(bytes).map_err(|_| invalid("invalid HID identity text"))
}
fn identity(sys: &Path) -> io::Result<(u32, u16, u16, String)> {
    let event = text(&sys.join("device/uevent"))?;
    let get = |prefix: &str| {
        event
            .lines()
            .find_map(|line| line.strip_prefix(prefix))
            .ok_or_else(|| invalid("incomplete HID identity"))
    };
    let parts: Vec<_> = get("HID_ID=")?.split(':').collect();
    if parts.len() != 3 {
        return Err(invalid("invalid HID ID"));
    }
    let number = |text_item| u32::from_str_radix(text_item, 16).map_err(|_| invalid("invalid HID ID"));
    let narrow = |text_item| u16::try_from(number(text_item)?).map_err(|_| invalid("invalid HID VID/PID"));
    Ok((
        number(parts[0])?,
        narrow(parts[1])?,
        narrow(parts[2])?,
        get("HID_NAME=")?.to_owned(),
    ))
}
fn eligible(sys: &Path, profiles: &[Profile]) -> bool {
    identity(sys)
        .is_ok_and(|(bus, vendor, product, name)| profiles.iter().filter(|profile| profile.matches(bus, vendor, product, &name)).count() == 1)
}
fn node_name(name: &str) -> bool {
    name.strip_prefix("hidraw")
        .is_some_and(|text_item| !text_item.is_empty() && text_item.len() <= 3 && text_item.bytes().all(|second_metadata| second_metadata.is_ascii_digit()))
}
// Linux _IOR('H', nr, T). Only read-only metadata ioctls are provided here.
fn query<T>(file: &File, nr: u8, value: &mut T) -> io::Result<()> {
    let request = 0x80000000u32
        | ((std::mem::size_of::<T>() as u32) << 16)
        | ((b'H' as u32) << 8)
        | u32::from(nr);
    // SAFETY: callers use initialized repr(C) integer storage of exactly T's size.
    if unsafe { libc::ioctl(file.as_raw_fd(), request as _, value as *mut T) } < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
#[repr(C)]
#[derive(Default)]
struct RawInfo {
    bus: u32,
    vendor: u16,
    product: u16,
}
#[repr(C)]
struct RawDescriptor {
    size: u32,
    value: [u8; MAX_DESCRIPTOR_BYTES],
}
impl Source for LinuxSource {
    fn discover(&mut self) -> io::Result<Vec<PathBuf>> {
        let entries = match fs::read_dir("/sys/class/hidraw") {
            Ok(error) => error,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        let mut paths = Vec::new();
        for (index, entry) in entries.enumerate() {
            if index >= 256 {
                return Err(invalid("hidraw directory exceeds limit"));
            }
            let entry = entry?;
            if entry.file_name().to_str().is_some_and(node_name)
                && eligible(&entry.path(), &self.profiles)
            {
                paths.push(Path::new("/dev").join(entry.file_name()));
            }
        }
        paths.sort();
        Ok(paths)
    }
    fn open(&mut self, path: &Path) -> io::Result<Box<dyn Device>> {
        if path.parent() != Some(Path::new("/dev")) {
            return Err(invalid("hidraw path outside /dev"));
        }
        let name = path
            .file_name()
            .and_then(|name_item| name_item.to_str())
            .filter(|name_item| node_name(name_item))
            .ok_or_else(|| invalid("invalid hidraw node name"))?;
        let before = fs::symlink_metadata(path)?;
        if !before.file_type().is_char_device() {
            return Err(invalid("not a hidraw character device"));
        }
        let sys = Path::new("/sys/class/hidraw").join(name);
        let generation = sys.join("device").canonicalize()?;
        let (bus, vendor, product, name) = identity(&sys)?;
        if self.profiles.iter().filter(|profile| profile.matches(bus, vendor, product, &name)).count() != 1 {
            return Err(invalid("HID device does not match profile"));
        }
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(path)?;
        let opened = file.metadata()?;
        let same = |first_metadata: &fs::Metadata, second_metadata: &fs::Metadata| {
            first_metadata.dev() == second_metadata.dev() && first_metadata.ino() == second_metadata.ino() && first_metadata.rdev() == second_metadata.rdev()
        };
        if !opened.file_type().is_char_device() || !same(&before, &opened) {
            return Err(invalid("hidraw changed during open"));
        }
        let mut raw_info = RawInfo::default();
        query(&file, 0x03, &mut raw_info)?;
        if (raw_info.bus, raw_info.vendor, raw_info.product) != (bus, vendor, product) {
            return Err(invalid("hidraw FD identity mismatch"));
        }
        let mut raw_name = [0u8; 256];
        query(&file, 0x04, &mut raw_name)?;
        if raw_name.split(|second_metadata| *second_metadata == 0).next() != Some(name.as_bytes()) {
            return Err(invalid("hidraw FD name mismatch"));
        }
        let mut size = 0u32;
        query(&file, 0x01, &mut size)?;
        if size == 0 || size as usize > MAX_DESCRIPTOR_BYTES {
            return Err(invalid("invalid HID descriptor size"));
        }
        let mut raw = RawDescriptor {
            size,
            value: [0; MAX_DESCRIPTOR_BYTES],
        };
        query(&file, 0x02, &mut raw)?;
        if raw.size != size {
            return Err(invalid("HID descriptor size changed"));
        }
        let descriptor = raw.value[..size as usize].to_vec();
        let dev = format!(
            "{}:{}",
            libc::major(opened.rdev()),
            libc::minor(opened.rdev())
        );
        if text(&sys.join("dev"))?.trim() != dev
            || sys.join("device").canonicalize()? != generation
            || !same(&opened, &fs::symlink_metadata(path)?)
        {
            return Err(invalid("hidraw generation changed during acquisition"));
        }
        Ok(Box::new(Raw {
            file,
            info: Info {
                generation,
                bus,
                vendor,
                product,
                name,
                descriptor,
            },
        }))
    }
}
impl Device for Raw {
    fn info(&self) -> &Info {
        &self.info
    }
    fn read(&mut self, packet: &mut [u8]) -> io::Result<Option<usize>> {
        match self.file.read(packet) {
            Ok(0) => Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "hidraw disconnected",
            )),
            Ok(name_item) => Ok(Some(name_item)),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(None),
            Err(error) => Err(error),
        }
    }
}
