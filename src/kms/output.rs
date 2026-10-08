// SPDX-License-Identifier: BSD-3-Clause
//! Pure legacy-KMS routing and restoration policy; never opens a device.

use std::{fmt::Debug, io};

/// Timing, excluding names, preferred/driver flags and computed refresh metadata.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timing {
    pub size: (u16, u16),
    pub clock: u32,
    pub horizontal: (u16, u16, u16),
    pub vertical: (u16, u16, u16),
    pub hskew: u16,
    pub vscan: u16,
    pub flags: u32,
}

/// Keeps the adapter's exact mode for SETCRTC without manufacturing a TV timing.
pub trait DisplayMode: Copy + Eq + Debug {
    fn timing(self) -> Timing;
    fn preferred(self) -> bool;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Connector<M> {
    pub id: u32,
    pub connected: bool,
    pub hdmi: bool,
    pub current_encoder: Option<u32>,
    pub encoders: Vec<u32>,
    pub modes: Vec<M>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Encoder {
    pub id: u32,
    pub current_crtc: Option<u32>,
    pub possible_crtcs: Vec<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Crtc<M> {
    pub id: u32,
    pub framebuffer: Option<u32>,
    pub position: (u32, u32),
    pub mode: Option<M>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Topology<M> {
    pub connectors: Vec<Connector<M>>,
    pub encoders: Vec<Encoder>,
    pub crtcs: Vec<Crtc<M>>,
}

/// A SETCRTC request, including the empty connector list needed to disable a CRTC.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request<M> {
    pub crtc: u32,
    pub framebuffer: Option<u32>,
    pub position: (u32, u32),
    pub connectors: Vec<u32>,
    pub mode: Option<M>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Output<M> {
    pub connector: u32,
    pub encoder: u32,
    pub previous: Crtc<M>,
    pub mode: M,
}

fn require(condition: bool, message: &str) -> io::Result<()> {
    if condition {
        Ok(())
    } else {
        Err(io::Error::other(message))
    }
}

fn ids(values: impl IntoIterator<Item = u32>) -> io::Result<()> {
    let mut seen = Vec::new();
    for id in values {
        require(
            id != 0 && !seen.contains(&id),
            "invalid or duplicate DRM resource",
        )?;
        seen.push(id);
    }
    Ok(())
}

fn supported(mode: impl DisplayMode) -> bool {
    let t = mode.timing();
    let (hs, he, ht) = t.horizontal;
    let (vs, ve, vt) = t.vertical;
    // Linux UAPI INTERLACE (bit 4) and DBLSCAN (bit 5). No invented timings.
    t.size == (1280, 720)
        && t.clock > 0
        && t.flags & ((1 << 4) | (1 << 5)) == 0
        && t.vscan <= 1
        && hs >= 1280
        && he > hs
        && ht > he
        && vs >= 720
        && ve > vs
        && vt > ve
}

impl<M: DisplayMode> Topology<M> {
    /// Called again immediately before SETCRTC, after buffer allocation.
    pub fn verify_unchanged(&self, current: &Self) -> io::Result<()> {
        require(self == current, "DRM topology changed before modeset")
    }

    fn validate(&self) -> io::Result<()> {
        for count in [self.connectors.len(), self.encoders.len(), self.crtcs.len()] {
            require((1..=32).contains(&count), "unexpected DRM resource counts")?;
        }
        ids(self.connectors.iter().map(|v| v.id))?;
        ids(self.encoders.iter().map(|v| v.id))?;
        ids(self.crtcs.iter().map(|v| v.id))?;
        for connector in &self.connectors {
            require(
                connector.encoders.len() <= 32 && connector.modes.len() <= 128,
                "unexpected display connector or modes",
            )?;
            ids(connector.encoders.iter().copied())?;
            for encoder in connector
                .encoders
                .iter()
                .copied()
                .chain(connector.current_encoder)
            {
                require(
                    self.encoders.iter().any(|e| e.id == encoder),
                    "unknown connector encoder",
                )?;
            }
        }
        for encoder in &self.encoders {
            require(
                encoder.possible_crtcs.len() <= 32,
                "unexpected encoder CRTC count",
            )?;
            ids(encoder.possible_crtcs.iter().copied())?;
            for crtc in encoder
                .possible_crtcs
                .iter()
                .copied()
                .chain(encoder.current_crtc)
            {
                require(
                    self.crtcs.iter().any(|c| c.id == crtc),
                    "unknown encoder CRTC",
                )?;
            }
        }
        for crtc in &self.crtcs {
            require(crtc.framebuffer != Some(0), "invalid DRM framebuffer")?;
        }
        Ok(())
    }

    fn exclusive(&self, connector: u32, encoder: u32, crtc: u32) -> bool {
        !self
            .encoders
            .iter()
            .any(|e| e.id != encoder && e.current_crtc == Some(crtc))
            && !self
                .connectors
                .iter()
                .any(|c| c.id != connector && c.current_encoder == Some(encoder))
    }

    /// Prefer the current route. Only a fully disabled, unshared CRTC is a fallback.
    pub fn select(&self) -> io::Result<Output<M>> {
        self.select_connector(None)
    }

    /// Selection does not filter the topology: other outputs still participate
    /// in ownership checks, including disconnected outputs with active routes.
    pub fn select_connector(&self, id: Option<u32>) -> io::Result<Output<M>> {
        self.validate()?;
        let connected: Vec<_> = self
            .connectors
            .iter()
            .filter(|c| c.connected && id.is_none_or(|id| id == c.id))
            .collect();
        let [connector] = connected.as_slice() else {
            return Err(io::Error::other(
                "exactly one connected selected output is required",
            ));
        };
        require(
            !connector.encoders.is_empty(),
            "connector with advertised encoders required",
        )?;
        if let Some(id) = connector.current_encoder {
            require(
                connector.encoders.contains(&id),
                "current encoder is not advertised by display",
            )?;
            let encoder = self.encoders.iter().find(|e| e.id == id).unwrap();
            if let Some(id) = encoder.current_crtc {
                require(
                    encoder.possible_crtcs.contains(&id),
                    "incompatible display encoder/CRTC",
                )?;
                let previous = *self.crtcs.iter().find(|c| c.id == id).unwrap();
                require(
                    self.exclusive(connector.id, encoder.id, id),
                    "display CRTC is shared with another output",
                )?;
                match (previous.mode, previous.framebuffer) {
                    (Some(mode), Some(_)) => {
                        require(
                            supported(mode),
                            "active display mode must be progressive 1280x720",
                        )?;
                        return Ok(Output {
                            connector: connector.id,
                            encoder: encoder.id,
                            previous,
                            mode,
                        });
                    }
                    (None, None) => {}
                    _ => return Err(io::Error::other("inconsistent active display CRTC state")),
                }
            }
        }
        let mode = connector
            .modes
            .iter()
            .copied()
            .filter(|m| supported(*m))
            .find(|m| m.preferred())
            .or_else(|| connector.modes.iter().copied().find(|m| supported(*m)))
            .ok_or_else(|| io::Error::other("display does not advertise progressive 1280x720"))?;
        // Try the current encoder first, then the driver's advertised order, as BOX1 does.
        let order = connector.current_encoder.into_iter().chain(
            connector
                .encoders
                .iter()
                .copied()
                .filter(|id| Some(*id) != connector.current_encoder),
        );
        for id in order {
            let encoder = self.encoders.iter().find(|e| e.id == id).unwrap();
            for previous in &self.crtcs {
                if encoder.possible_crtcs.contains(&previous.id)
                    && encoder.current_crtc.is_none_or(|id| id == previous.id)
                    && previous.mode.is_none()
                    && previous.framebuffer.is_none()
                    && self.exclusive(connector.id, encoder.id, previous.id)
                {
                    return Ok(Output {
                        connector: connector.id,
                        encoder: encoder.id,
                        previous: *previous,
                        mode,
                    });
                }
            }
        }
        Err(io::Error::other(
            "no unused compatible display encoder/CRTC",
        ))
    }
}

fn same_state<M: DisplayMode>(left: Crtc<M>, right: Crtc<M>) -> bool {
    left.id == right.id && left.framebuffer == right.framebuffer
        && left.mode.map(DisplayMode::timing) == right.mode.map(DisplayMode::timing)
        // Disabled CRTCs may retain stale x/y; these have no scanout meaning.
        && (left.mode.is_none() && left.framebuffer.is_none() || left.position == right.position)
}

impl<M: DisplayMode> Output<M> {
    pub fn enable_request(&self, framebuffer: u32) -> io::Result<Request<M>> {
        require(framebuffer != 0, "invalid DRM framebuffer")?;
        Ok(Request {
            crtc: self.previous.id,
            framebuffer: Some(framebuffer),
            position: (0, 0),
            connectors: vec![self.connector],
            mode: Some(self.mode),
        })
    }

    pub fn verify(&self, current: Crtc<M>, framebuffer: u32) -> io::Result<()> {
        require(
            framebuffer != 0
                && same_state(
                    current,
                    Crtc {
                        id: self.previous.id,
                        framebuffer: Some(framebuffer),
                        position: (0, 0),
                        mode: Some(self.mode),
                    },
                ),
            "DRM scanout readback mismatch or another owner",
        )
    }

    /// None means SETCRTC did not alter state. Never overwrite another owner's FB.
    pub fn restore_request(
        &self,
        current: Crtc<M>,
        framebuffer: u32,
    ) -> io::Result<Option<Request<M>>> {
        require(
            current.id == self.previous.id && framebuffer != 0,
            "wrong DRM restore identity",
        )?;
        if same_state(current, self.previous) {
            return Ok(None);
        }
        require(
            current.framebuffer == Some(framebuffer),
            "CRTC changed by another owner; refusing restore",
        )?;
        let active = self.previous.mode.is_some();
        Ok(Some(Request {
            crtc: self.previous.id,
            framebuffer: self.previous.framebuffer,
            position: if active {
                self.previous.position
            } else {
                (0, 0)
            },
            connectors: if active { vec![self.connector] } else { vec![] },
            mode: self.previous.mode,
        }))
    }

    pub fn verify_restored(&self, current: Crtc<M>) -> io::Result<()> {
        require(
            same_state(current, self.previous),
            "previous CRTC restoration was not confirmed",
        )
    }
}
