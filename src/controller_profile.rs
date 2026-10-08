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
    pub fn validate(&self, name: &str) -> io::Result<()> {
        let hex = |s: &str| {
            s.len() == 4
                && s.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
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
    pub fn matches(&self, bus: u32, vendor: u16, product: u16) -> bool {
        bus == 3
            && u16::from_str_radix(&self.vendor, 16) == Ok(vendor)
            && u16::from_str_radix(&self.product, 16) == Ok(product)
    }
}
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
pub(crate) fn unique_map<'de, D, K, V>(d: D) -> Result<BTreeMap<K, V>, D::Error>
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
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("unique input bindings")
        }
        fn visit_map<A: serde::de::MapAccess<'de>>(
            self,
            mut a: A,
        ) -> Result<Self::Value, A::Error> {
            let mut result = BTreeMap::new();
            while let Some((key, value)) = a.next_entry()? {
                if result.insert(key, value).is_some() {
                    return Err(A::Error::custom("duplicate input binding"));
                }
            }
            Ok(result)
        }
    }
    d.deserialize_map(Visitor(std::marker::PhantomData))
}
