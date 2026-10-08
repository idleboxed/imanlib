// SPDX-License-Identifier: BSD-3-Clause
use super::*;
use crate::kms::{
    diagnostic::{EventStamp, Operation, Timings},
    flip::wait_for_flip_event,
};
use std::time::Instant;

impl LinuxDrm {
    /// Uses the ordinary two-buffer policy and owner checks, with no raw FB access.
    /// FlipOnly requires both slots to have been populated by the caller beforehand.
    pub fn diagnostic_step(
        &mut self,
        operation: Operation,
        frame: &Surface,
    ) -> io::Result<Timings> {
        if !self.enabled {
            return Err(io::Error::other("diagnostic scanout is not enabled"));
        }
        let mut timed = Timed {
            io: Scanout {
                card: &self.card,
                selected: self.output.as_ref().expect("output selected"),
                slots: &mut self.slots,
            },
            operation,
            timings: Timings::default(),
        };
        let result = match operation {
            Operation::UploadOnly => self.flips.write_hidden(&mut timed, frame),
            Operation::FlipOnly | Operation::UploadAndFlip => self.flips.present(&mut timed, frame),
        };
        if let Err(error) = &result {
            let _ = writeln!(
                io::stderr(),
                "REDRAW operation={operation:?} failed={error} partial={:?}",
                timed.timings
            );
        }
        result?;
        Ok(timed.timings)
    }
}

struct Timed<'a> {
    io: Scanout<'a>,
    operation: Operation,
    timings: Timings,
}

impl FlipIo for Timed<'_> {
    fn verify(&mut self, slot: usize) -> io::Result<()> {
        self.io.verify(slot)
    }

    fn write(&mut self, slot: usize, frame: &Surface) -> io::Result<()> {
        if self.operation == Operation::FlipOnly {
            if self.io.slots[slot].buffer.is_none() || self.io.slots[slot].framebuffer.is_none() {
                return Err(io::Error::other(
                    "flip-only requires a prepared hidden buffer",
                ));
            }
            return Ok(());
        }
        if (frame.width(), frame.height()) != (WIDTH, HEIGHT) {
            return Err(io::Error::other("unexpected GUI dimensions"));
        }
        self.io.ensure_slot(slot)?;
        let format = self.io.slots[slot].format;
        let start = Instant::now();
        let (mut mapping, pitch) = self.io.slots[slot].map(self.io.card)?;
        self.timings.map_us = Some(start.elapsed().as_micros() as u64);
        let start = Instant::now();
        let result = format.copy_frame(frame, mapping.as_mut(), pitch);
        self.timings.copy_us = Some(start.elapsed().as_micros() as u64);
        let start = Instant::now();
        drop(mapping);
        self.timings.unmap_us = Some(start.elapsed().as_micros() as u64);
        result
    }

    fn queue(&mut self, slot: usize) -> io::Result<()> {
        let start = Instant::now();
        let result = self.io.queue(slot);
        self.timings.submit_us = Some(start.elapsed().as_micros() as u64);
        result
    }

    fn wait(&mut self) -> io::Result<()> {
        let start = Instant::now();
        let result = wait_for_flip_event(
            self.io.card,
            handle(self.io.selected.output.previous.id)?,
            Duration::from_secs(1),
        );
        self.timings.wait_us = Some(start.elapsed().as_micros() as u64);
        let event = result?;
        self.timings.event = Some(EventStamp {
            sequence: event.frame,
            timestamp_us: event.duration.as_micros() as u64,
        });
        Ok(())
    }

    fn restore(&mut self, slot: usize) -> io::Result<()> {
        self.io.restore(slot)
    }
}
