// SPDX-License-Identifier: BSD-3-Clause
use std::{
    fs::{File, OpenOptions},
    io::{self, Write},
    os::{
        fd::{AsFd, BorrowedFd},
        unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt},
    },
    path::Path,
    time::Duration,
};

use drm::{
    buffer::Buffer,
    control::{
        connector, crtc, dumbbuffer::DumbBuffer, framebuffer, Device as ControlDevice, Mode,
        ModeTypeFlags, PageFlipFlags,
    },
    Device, DriverCapability,
};

use super::{
    format::Format,
    flip::{wait_for_flip, DoubleBuffer, FlipIo},
    output::{Connector, Crtc, DisplayMode, Encoder, Output, Request, Timing, Topology},
    Backend,
};
use crate::surface::{Surface, HEIGHT, WIDTH};

#[cfg(feature = "drm-diagnostics")]
#[path = "linux_diagnostic.rs"]
mod diagnostic;

struct Card(File);
impl AsFd for Card {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.0.as_fd()
    }
}
impl Device for Card {}
impl ControlDevice for Card {}

impl DisplayMode for Mode {
    fn timing(self) -> Timing {
        Timing {
            size: self.size(),
            clock: self.clock(),
            horizontal: self.hsync(),
            vertical: self.vsync(),
            hskew: self.hskew(),
            vscan: self.vscan(),
            flags: self.flags().bits(),
        }
    }
    fn preferred(self) -> bool {
        self.mode_type().contains(ModeTypeFlags::PREFERRED)
    }
}

struct Selected {
    topology: Topology<Mode>,
    output: Output<Mode>,
}

fn crtc_state(info: crtc::Info) -> Crtc<Mode> {
    Crtc {
        id: info.handle().into(),
        framebuffer: info.framebuffer().map(u32::from),
        position: info.position(),
        mode: info.mode(),
    }
}

fn handle<T: From<std::num::NonZeroU32>>(id: u32) -> io::Result<T> {
    std::num::NonZeroU32::new(id)
        .map(T::from)
        .ok_or_else(|| io::Error::other("zero DRM resource handle"))
}

pub struct LinuxDrm {
    card: Card,
    connector: Option<u32>,
    output: Option<Selected>,
    slots: [Slot; 2],
    enabled: bool,
    flips: DoubleBuffer,
}

impl LinuxDrm {
    fn topology(&self, force_probe: bool) -> io::Result<Topology<Mode>> {
        let resources = self.card.resource_handles()?;
        for count in [
            resources.connectors().len(),
            resources.encoders().len(),
            resources.crtcs().len(),
        ] {
            if !(1..=32).contains(&count) {
                return Err(io::Error::other("unexpected DRM resource counts"));
            }
        }
        let mut topology = Topology {
            connectors: Vec::new(),
            encoders: Vec::new(),
            crtcs: Vec::new(),
        };
        for id in resources.connectors() {
            let info = self.card.get_connector(*id, force_probe)?;
            if info.modes().len() > 128 || info.encoders().len() > 32 {
                return Err(io::Error::other("unexpected connector resource counts"));
            }
            topology.connectors.push(Connector {
                id: (*id).into(),
                connected: info.state() == connector::State::Connected,
                hdmi: info.interface() == connector::Interface::HDMIA,
                current_encoder: info.current_encoder().map(u32::from),
                encoders: info.encoders().iter().copied().map(u32::from).collect(),
                modes: info.modes().to_vec(),
            });
        }
        for id in resources.encoders() {
            let info = self.card.get_encoder(*id)?;
            topology.encoders.push(Encoder {
                id: (*id).into(),
                current_crtc: info.crtc().map(u32::from),
                possible_crtcs: resources
                    .filter_crtcs(info.possible_crtcs())
                    .into_iter()
                    .map(u32::from)
                    .collect(),
            });
        }
        for id in resources.crtcs() {
            topology.crtcs.push(crtc_state(self.card.get_crtc(*id)?));
        }
        Ok(topology)
    }

    fn apply(&self, request: Request<Mode>) -> io::Result<()> {
        let connectors: Vec<connector::Handle> = request
            .connectors
            .into_iter()
            .map(handle)
            .collect::<io::Result<_>>()?;
        self.card.set_crtc(
            handle(request.crtc)?,
            request.framebuffer.map(handle).transpose()?,
            request.position,
            &connectors,
            request.mode,
        )
    }

    /// Open only the explicitly selected Linux primary node; never probe cards.
    pub fn open(path: &Path, connector: Option<u32>) -> io::Result<Self> {
        Self::open_with_format(path, connector, Format::configured())
    }

    /// Explicit scanout format; no fallback, device probing or child format change.
    pub fn open_with_format(path: &Path, connector: Option<u32>, format: Format) -> io::Result<Self> {
        format.registration()?;
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.strip_prefix("card"));
        let minor = name
            .and_then(|n| n.parse::<u32>().ok())
            .filter(|n| *n < 64)
            .ok_or_else(|| io::Error::other("expected a DRM primary node named card0..card63"))?;
        if !path.is_absolute() {
            return Err(io::Error::other("DRM device path must be absolute"));
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)?;
        let meta = file.metadata()?;
        if !meta.file_type().is_char_device()
            || libc::major(meta.rdev()) != 226
            || libc::minor(meta.rdev()) != minor
        {
            return Err(io::Error::other("unexpected DRM primary device"));
        }
        let card = Card(file);
        if card.get_driver_capability(DriverCapability::DumbBuffer)? == 0 {
            return Err(io::Error::other("DRM dumb buffers are required"));
        }
        Ok(Self {
            card,
            connector,
            output: None,
            slots: std::array::from_fn(|_| Slot {
                format,
                ..Slot::default()
            }),
            enabled: false,
            flips: DoubleBuffer::default(),
        })
    }
}

impl Backend for LinuxDrm {
    fn acquire_master(&mut self) -> io::Result<()> {
        self.card.acquire_master_lock()
    }
    fn release_master(&mut self) -> io::Result<()> {
        self.card.release_master_lock()
    }

    fn select_output(&mut self) -> io::Result<()> {
        let topology = self.topology(true)?;
        // Bounded records survive an early refusal; no binary data or per-frame dumps.
        for c in &topology.connectors {
            eprintln!("IGUI DRM: connector={} connected={} hdmi={} current_encoder={:?} encoders={:?} modes={}",
                c.id, c.connected, c.hdmi, c.current_encoder, c.encoders, c.modes.len());
        }
        for e in &topology.encoders {
            eprintln!(
                "IGUI DRM: encoder={} current_crtc={:?} possible_crtcs={:?}",
                e.id, e.current_crtc, e.possible_crtcs
            );
        }
        for c in &topology.crtcs {
            eprintln!(
                "IGUI DRM: crtc={} fb={:?} position={:?} mode={:?}",
                c.id, c.framebuffer, c.position, c.mode
            );
        }
        let output = topology.select_connector(self.connector)?;
        eprintln!(
            "IGUI DRM: selected connector={} encoder={} crtc={} restore={} mode={:?}",
            output.connector,
            output.encoder,
            output.previous.id,
            if output.previous.mode.is_some() {
                "active"
            } else {
                "disabled"
            },
            output.mode
        );
        self.output = Some(Selected { topology, output });
        Ok(())
    }

    fn create_buffer(&mut self) -> io::Result<()> {
        self.slots[0].create(&self.card)
    }

    fn add_framebuffer(&mut self) -> io::Result<()> {
        self.slots[0].add(&self.card)
    }

    fn write_frame(&mut self, surface: &Surface) -> io::Result<()> {
        if !self.enabled {
            return self.slots[0].write(&self.card, surface);
        }
        let mut io = Scanout {
            card: &self.card,
            selected: self.output.as_ref().expect("output selected"),
            slots: &mut self.slots,
        };
        let first = self.flips.completed() == 0;
        let result = self.flips.present(&mut io, surface);
        if first && result.is_ok() {
            let _ = writeln!(io::stderr(), "IGUI DRM: first synchronized page flip completed and verified; double buffering active");
        }
        if let Err(error) = &result {
            let _ = writeln!(
                io::stderr(),
                "IGUI DRM: page flip failed; no single-buffer fallback: {error}"
            );
        }
        result
    }

    fn enable(&mut self) -> io::Result<()> {
        let selected = self.output.as_ref().expect("output selected");
        // Legacy SETCRTC has no atomic ownership check. Refuse known changes before
        // the syscall; afterwards the scanout is checked before displaying frames.
        selected.topology.verify_unchanged(&self.topology(false)?)?;
        let request = selected.output.enable_request(
            self.slots[0]
                .framebuffer
                .expect("framebuffer created")
                .into(),
        )?;
        self.enabled = true; // Even a failed SETCRTC must be checked before freeing.
        self.apply(request)
    }

    fn verify(&mut self) -> io::Result<()> {
        self.flips.ready()?;
        Scanout {
            card: &self.card,
            selected: self.output.as_ref().expect("output selected"),
            slots: &mut self.slots,
        }
        .verify(self.flips.front())
    }

    fn restore(&mut self) -> io::Result<()> {
        let result = self.flips.finish(&mut Scanout {
            card: &self.card,
            selected: self.output.as_ref().expect("output selected"),
            slots: &mut self.slots,
        });
        let _ = writeln!(
            io::stderr(),
            "IGUI DRM: page flips completed={}; restore={result:?}",
            self.flips.completed()
        );
        result
    }

    fn remove_framebuffer(&mut self) -> io::Result<()> {
        self.safe_to_release()?;
        let mut errors = Vec::new();
        for slot in &mut self.slots {
            if let Some(framebuffer) = slot.framebuffer {
                match self.card.destroy_framebuffer(framebuffer) {
                    Ok(()) => slot.framebuffer = None,
                    Err(e) => errors.push(e.to_string()),
                }
            }
        }
        cleanup_result(errors)
    }
    fn destroy_buffer(&mut self) -> io::Result<()> {
        self.safe_to_release()?;
        let mut errors = Vec::new();
        for slot in &mut self.slots {
            if slot.framebuffer.is_some() {
                errors.push("framebuffer still registered; buffer retained until FD close".into());
            } else if let Some(buffer) = slot.buffer.take() {
                if let Err(e) = self.card.destroy_dumb_buffer(buffer) {
                    errors.push(e.to_string());
                }
            }
        }
        cleanup_result(errors)
    }
}

impl LinuxDrm {
    /// Called by a retained owner before lending the display, never by IGUI's
    /// rendering loop. A future present lazily allocates a hidden buffer again.
    pub fn discard_hidden(&mut self) -> io::Result<()> {
        if !self.enabled {
            return Err(io::Error::other("display scanout is not enabled"));
        }
        self.flips.discard_hidden(&mut Scanout {
            card: &self.card,
            selected: self.output.as_ref()
                .ok_or_else(|| io::Error::other("display output is not selected"))?,
            slots: &mut self.slots,
        })
    }

    fn safe_to_release(&self) -> io::Result<()> {
        if self.enabled && !self.flips.retired() {
            return Err(io::Error::other(
                "scanout not safely restored; buffers retained until DRM FD close",
            ));
        }
        Ok(())
    }

    /// Cached routing/counters only; never queries or opens another DRM descriptor.
    pub fn diagnostic_state(
        &self,
        master_acquired: bool,
    ) -> Option<crate::display_metrics::KmsState> {
        let output = &self.output.as_ref()?.output;
        Some(crate::display_metrics::KmsState {
            connector: output.connector,
            encoder: output.encoder,
            crtc: output.previous.id,
            width: output.mode.size().0,
            height: output.mode.size().1,
            refresh_hz: output.mode.vrefresh(),
            completed_flips: self.flips.completed(),
            master_acquired,
        })
    }

    /// Read-only diagnostic snapshot through this owner's already open FD.
    /// Opening a second primary FD just for observation could acquire master.
    pub fn snapshot(&self) -> io::Result<Topology<Mode>> {
        self.topology(false)
    }
}

fn cleanup_result(errors: Vec<String>) -> io::Result<()> {
    if errors.is_empty() {
        Ok(())
    } else {
        Err(io::Error::other(errors.join("; ")))
    }
}

#[derive(Default)]
struct Slot {
    format: Format,
    buffer: Option<DumbBuffer>,
    framebuffer: Option<framebuffer::Handle>,
}

impl Slot {
    fn create(&mut self, card: &Card) -> io::Result<()> {
        let registration = self.format.registration()?;
        let buffer = card.create_dumb_buffer(
            (WIDTH, HEIGHT),
            registration.fourcc,
            registration.bits_per_pixel,
        )?;
        self.buffer = Some(buffer); // Retain even invalid geometry for cleanup.
        if buffer.size() != (WIDTH, HEIGHT)
            || self.format
                .validate_pitch(WIDTH as usize, buffer.pitch() as usize)
                .is_err()
            || u64::from(buffer.pitch()) * u64::from(HEIGHT) > 32 * 1024 * 1024
        {
            return Err(io::Error::other("invalid DRM dumb buffer geometry"));
        }
        Ok(())
    }

    fn add(&mut self, card: &Card) -> io::Result<()> {
        let registration = self.format.registration()?;
        self.framebuffer = Some(card.add_framebuffer(
            self.buffer.as_ref().expect("buffer created"),
            registration.depth,
            registration.bits_per_pixel,
        )?);
        Ok(())
    }

    fn write(&mut self, card: &Card, surface: &Surface) -> io::Result<()> {
        if (surface.width(), surface.height()) != (WIDTH, HEIGHT) {
            return Err(io::Error::other("unexpected GUI dimensions"));
        }
        let format = self.format;
        let (mut mapping, pitch) = self.map(card)?;
        format.copy_frame(surface, mapping.as_mut(), pitch)
    }

    fn map(
        &mut self,
        card: &Card,
    ) -> io::Result<(drm::control::dumbbuffer::DumbMapping<'_>, usize)> {
        let buffer = self.buffer.as_mut().expect("buffer created");
        let pitch = buffer.pitch() as usize;
        let mapping = card.map_dumb_buffer(buffer)?;
        if mapping.len() > 32 * 1024 * 1024 {
            return Err(io::Error::other("unexpected DRM mapping size"));
        }
        Ok((mapping, pitch))
    }
}

struct Scanout<'a> {
    card: &'a Card,
    selected: &'a Selected,
    slots: &'a mut [Slot; 2],
}

impl Scanout<'_> {
    fn ensure_slot(&mut self, slot: usize) -> io::Result<()> {
        if self.slots[slot].buffer.is_none() {
            self.slots[slot].create(self.card)?;
        }
        if self.slots[slot].framebuffer.is_none() {
            self.slots[slot].add(self.card)?;
        }
        Ok(())
    }
}

impl FlipIo for Scanout<'_> {
    fn discard(&mut self, slot: usize) -> io::Result<()> {
        let slot = &mut self.slots[slot];
        if let Some(framebuffer) = slot.framebuffer {
            self.card.destroy_framebuffer(framebuffer)?;
            slot.framebuffer = None;
        }
        if let Some(buffer) = slot.buffer.take() {
            self.card.destroy_dumb_buffer(buffer)?;
        }
        Ok(())
    }

    fn verify(&mut self, slot: usize) -> io::Result<()> {
        let output = &self.selected.output;
        let current = crtc_state(self.card.get_crtc(handle(output.previous.id)?)?);
        output.verify(
            current,
            self.slots[slot]
                .framebuffer
                .expect("framebuffer created")
                .into(),
        )
    }

    fn write(&mut self, slot: usize, frame: &Surface) -> io::Result<()> {
        // Retained owners may discard the hidden slot between redraws. Allocate
        // only when needed, before queuing any flip; failures stop the session.
        self.ensure_slot(slot)?;
        self.slots[slot].write(self.card, frame)
    }

    fn queue(&mut self, slot: usize) -> io::Result<()> {
        self.card.page_flip(
            handle(self.selected.output.previous.id)?,
            self.slots[slot].framebuffer.expect("framebuffer created"),
            PageFlipFlags::EVENT,
            None,
        )
    }

    fn wait(&mut self) -> io::Result<()> {
        wait_for_flip(
            self.card,
            handle(self.selected.output.previous.id)?,
            Duration::from_secs(1),
        )
    }

    fn restore(&mut self, slot: usize) -> io::Result<()> {
        let output = &self.selected.output;
        let current = crtc_state(self.card.get_crtc(handle(output.previous.id)?)?);
        if let Some(request) = output.restore_request(
            current,
            self.slots[slot]
                .framebuffer
                .expect("framebuffer created")
                .into(),
        )? {
            let connectors = request
                .connectors
                .into_iter()
                .map(handle)
                .collect::<io::Result<Vec<_>>>()?;
            self.card.set_crtc(
                handle(request.crtc)?,
                request.framebuffer.map(handle).transpose()?,
                request.position,
                &connectors,
                request.mode,
            )?;
            let restored = crtc_state(self.card.get_crtc(handle(output.previous.id)?)?);
            output.verify_restored(restored)?;
        }
        Ok(())
    }
}
