// SPDX-License-Identifier: BSD-3-Clause
//! Shared declarative input-profile boundaries, independent of device access.
use serde::{de::Error, Deserialize, Deserializer};
use std::{
    collections::BTreeMap,
    fs::OpenOptions,
    io::{self, Read},
    path::Path,
};

pub(crate) const MAX_BYTES: usize = 32 * 1024;
/// Construct an invalid-profile error without accessing any device.
pub(crate) fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}
#[derive(Clone, Deserialize)]
pub(crate) struct Identity {
    pub bus: String,
    pub vendor: String,
    pub product: String,
}
impl Identity {
    /// Require supported USB identity spelling and a bounded device name.
    pub fn validate(&self, name: &str) -> io::Result<()> {
        let hex = |text: &str| {
            text.len() == 4
                && text.bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        };
        if self.bus != "usb"
            || !hex(&self.vendor)
            || !hex(&self.product)
            || name.is_empty()
            || name.len() > 256
            || name.chars().any(char::is_control)
        {
            return Err(invalid("invalid USB profile identity/name"));
        }
        Ok(())
    }
    /// Report whether the USB bus, vendor and product match this identity.
    pub fn matches(&self, bus: u32, vendor: u16, product: u16) -> bool {
        bus == 3
            && u16::from_str_radix(&self.vendor, 16) == Ok(vendor)
            && u16::from_str_radix(&self.product, 16) == Ok(product)
    }
}
/// Read at most one bounded regular profile file without following its final symlink.
pub(crate) fn read(path: &Path) -> io::Result<Vec<u8>> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC);
    }
    let file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(invalid("input profile must be a regular file"));
    }
    let mut bytes = Vec::new();
    file.take((MAX_BYTES + 1) as u64).read_to_end(&mut bytes)?;
    if bytes.len() > MAX_BYTES {
        return Err(invalid("input profile exceeds 32 KiB"));
    }
    Ok(bytes)
}
// Unlike BTreeMap's default deserializer, reject duplicate JSON keys.
/// Decode profile bindings while rejecting duplicate keys rather than silently replacing them.
pub(crate) fn unique_map<'de, D, K, V>(deserializer: D) -> Result<BTreeMap<K, V>, D::Error>
where
    D: Deserializer<'de>,
    K: Deserialize<'de> + Ord,
    V: Deserialize<'de>,
{
    struct Visitor<K, V>(std::marker::PhantomData<(K, V)>);
    impl<'de, K: Deserialize<'de> + Ord, V: Deserialize<'de>> serde::de::Visitor<'de>
        for Visitor<K, V>
    {
        type Value = BTreeMap<K, V>;
        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("unique input bindings")
        }
        fn visit_map<A: serde::de::MapAccess<'de>>(
            self,
            mut access: A,
        ) -> Result<Self::Value, A::Error> {
            let mut result = BTreeMap::new();
            while let Some((key, value)) = access.next_entry()? {
                if result.insert(key, value).is_some() {
                    return Err(A::Error::custom("duplicate input binding"));
                }
            }
            Ok(result)
        }
    }
    deserializer.deserialize_map(Visitor(std::marker::PhantomData))
}
