//! Raspberry Pi 5 boot framebuffer backend.
//!
//! Bring-up splits in two because the Pi image negotiates the framebuffer
//! through the VideoCore mailbox *before* page tables exist, while the kernel
//! display object registers only after kernel bring-up:
//!
//! 1. [`negotiate`] — runs pre-MMU against the DTB-discovered mailbox:
//!    property-channel transactions covering the firmware-revision transport
//!    probe, physical+virtual geometry, depth, virtual offset, buffer
//!    allocation, and scanout pitch — each envelope validated per-tag by the
//!    mailbox driver. The resulting [`FramebufferInfo`] feeds both the
//!    BootInfo handoff slot and the later MMIO region mapping list.
//! 2. [`initialize_backend`] — runs post-MMU: wraps the negotiated buffer in
//!    the same [`DisplayBackend`] contract shape the x86 QEMU-VirtIO platform
//!    exposes (whole-frame + row-damage presents into scanout memory).
//!
//! Any mailbox failure (missing DTB node, rejected tags, malformed responses —
//! e.g. boards/emulators without VideoCore) degrades every entry to [`None`],
//! leaving the platform on its serial-first bootstrap unchanged. NO PATH HERE
//! HAS RUN ON REAL PI 5 HARDWARE: QEMU cannot emulate the BCM2712 VideoCore
//! firmware pair, so this whole flow implements the spec UNTESTED WITHOUT
//! HARDWARE.

use core::{
    ptr,
    sync::atomic::{AtomicBool, Ordering},
};

use alloc::sync::Arc;
use serviceos_abi::{
    DisplayOutputBackend, DisplayOutputInfo, DisplayOutputState, DisplayPixelFormat,
};
use serviceos_kernel_core::{
    bootstrap::{FramebufferInfo, FramebufferPixelFormat},
    display::{DisplayBackend, DisplayOutputError},
};

use crate::mailbox::{
    self, MailboxError, PropertyRequestWriter, TAG_ALLOCATE_BUFFER, TAG_GET_FIRMWARE_REVISION,
    TAG_GET_PITCH, TAG_SET_DEPTH, TAG_SET_PHYSICAL_WIDTH_HEIGHT, TAG_SET_VIRTUAL_OFFSET,
    TAG_SET_VIRTUAL_WIDTH_HEIGHT,
};

/// Requested physical geometry (standard HDMI class); 32bpp XRGB8888 fixed.
pub const DEFAULT_WIDTH: usize = 1920;
pub const DEFAULT_HEIGHT: usize = 1080;
pub const DEFAULT_DEPTH_BITS: usize = 32;
const BYTES_PER_PIXEL: usize = DEFAULT_DEPTH_BITS / 8;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FramebufferStatus {
    pub implemented: bool,
    pub initialized: bool,
}

static NEGOTIATED: AtomicBool = AtomicBool::new(false);
static mut NEGOTIATED_FRAMEBUFFER: Option<FramebufferInfo> = None;

/// Storage backing an in-flight property-channel envelope. Mailbox protocol
/// requires 16-byte alignment; a dedicated static keeps the address stable so
/// the descriptor handed to VideoCore always matches the exchanged buffer.
#[repr(C, align(16))]
struct EnvelopeArena([u32; mailbox::MAX_PROPERTY_WORDS]);

static mut ENVELOPE_ARENA: EnvelopeArena = EnvelopeArena([0; mailbox::MAX_PROPERTY_WORDS]);

fn envelope_arena() -> &'static mut [u32] {
    // SAFETY: single-threaded early-boot context (interrupts off, only hart 0
    // runs this path before SMP bring-up), zero-initialized static storage.
    // Raw-pointer hop satisfies the 2024 static-mut-references lint without
    // weakening the invariants this comment pins down.
    unsafe {
        core::slice::from_raw_parts_mut(
            core::ptr::addr_of_mut!(ENVELOPE_ARENA.0).cast::<u32>(),
            mailbox::MAX_PROPERTY_WORDS,
        )
    }
}

pub fn status() -> FramebufferStatus {
    FramebufferStatus {
        implemented: true,
        initialized: NEGOTIATED.load(Ordering::Acquire),
    }
}

fn record_negotiated(info: FramebufferInfo) {
    // SAFETY: same single-threaded preemption-free window as envelope_arena.
    unsafe {
        NEGOTIATED_FRAMEBUFFER = Some(info);
    }
    NEGOTIATED.store(true, Ordering::Release);
}

fn negotiated_framebuffer() -> Option<FramebufferInfo> {
    if !NEGOTIATED.load(Ordering::Acquire) {
        return None;
    }
    // SAFETY: written exactly once by record_negotiated before the flag flips.
    unsafe { NEGOTIATED_FRAMEBUFFER }
}

/// Information captured by [`negotiate`] for bootstrap logging and wiring.
#[derive(Clone, Copy, Debug)]
pub struct NegotiatedDisplay {
    pub info: FramebufferInfo,
    pub pitch_bytes: usize,
    pub firmware_revision: u32,
}

/// One staged property exchange: rebuild the authoritative request view,
/// submit over the DTB-discovered mailbox, then revalidate the in-place
/// response against that view. Returns the validated response slice.
///
/// UNTESTED WITHOUT HARDWARE (transport exercised only on a real mailbox).
fn exchange(
    build: impl FnOnce(&mut PropertyRequestWriter<'_>) -> Result<(), MailboxError>,
) -> Result<&'static [u32], MailboxError> {
    let base = mailbox::base().ok_or(MailboxError::InvalidRequest)?;
    let arena = envelope_arena();
    arena.fill(0);
    let mut writer = PropertyRequestWriter::new(arena)?;
    build(&mut writer)?;
    let request_words = writer.finish().len();

    let mut request_view = [0u32; mailbox::MAX_PROPERTY_WORDS];
    request_view[..request_words].copy_from_slice(&arena[..request_words]);

    mailbox::call_property_channel(
        base,
        &mut arena[..request_words],
        crate::timer::counter_frequency_hz(),
        crate::timer::counter_value,
    )?;

    let response = &arena[..request_words];
    mailbox::validate_exchange(&request_view, response)?;
    Ok(response)
}

/// Pre-MMU bring-up: negotiate an XRGB8888 framebuffer through three
/// serialized property-channel transactions. Returns `None` when no mailbox
/// was discovered or any step fails; callers continue serial-first bring-up
/// unchanged.
///
/// UNTESTED WITHOUT HARDWARE.
pub fn negotiate(firmware_revision_probe: bool) -> Option<NegotiatedDisplay> {
    if mailbox::base().is_none() {
        return None;
    }

    // Transaction A: identity probe validates transport end-to-end.
    let firmware_revision = if firmware_revision_probe {
        let response =
            exchange(|writer| writer.push_tag(TAG_GET_FIRMWARE_REVISION, &[], 4)).ok()?;
        match mailbox::tag_values(response, TAG_GET_FIRMWARE_REVISION) {
            Some(values) => values.values.first().copied().unwrap_or(0),
            None => return None,
        }
    } else {
        0
    };

    // Transaction B: geometry + depth + offset + allocation share one envelope
    // so the VC-side working queue stays consistent across dependent steps.
    let response = exchange(|writer| {
        writer.push_tag(
            TAG_SET_PHYSICAL_WIDTH_HEIGHT,
            &[DEFAULT_WIDTH as u32, DEFAULT_HEIGHT as u32],
            8,
        )?;
        writer.push_tag(
            TAG_SET_VIRTUAL_WIDTH_HEIGHT,
            &[DEFAULT_WIDTH as u32, DEFAULT_HEIGHT as u32],
            8,
        )?;
        writer.push_tag(TAG_SET_DEPTH, &[DEFAULT_DEPTH_BITS as u32], 4)?;
        writer.push_tag(TAG_SET_VIRTUAL_OFFSET, &[0, 0], 8)?;
        writer.push_tag(
            TAG_ALLOCATE_BUFFER,
            &[mailbox::BUFFER_MIN_ALIGN_BYTES as u32],
            8,
        )
    })
    .ok()?;

    let allocation = mailbox::tag_values(response, TAG_ALLOCATE_BUFFER)?;
    if allocation.values.len() < 2 {
        return None;
    }

    // Transaction C: scanout pitch, only meaningful once allocation completed.
    let response = exchange(|writer| writer.push_tag(TAG_GET_PITCH, &[], 4)).ok()?;
    let pitch = mailbox::tag_values(response, TAG_GET_PITCH)?;
    let pitch_bytes = pitch.values.first().copied().unwrap_or(0) as usize;

    let geometry = mailbox::assemble_geometry(
        allocation.values[0] as u64,
        allocation.values[1],
        DEFAULT_WIDTH,
        DEFAULT_HEIGHT,
        pitch_bytes,
        BYTES_PER_PIXEL,
    )?;
    let info = to_framebuffer_info(geometry);
    record_negotiated(info);
    Some(NegotiatedDisplay {
        info,
        pitch_bytes,
        firmware_revision,
    })
}

/// Map raw [`FrameGeometry`] facts onto the kernel's BootInfo frame type.
fn to_framebuffer_info(geometry: mailbox::FrameGeometry) -> FramebufferInfo {
    FramebufferInfo {
        physical_base: serviceos_kernel_core::memory::PhysicalAddress::new(
            geometry.physical_base,
        ),
        byte_len: geometry.byte_len,
        width: geometry.width,
        height: geometry.height,
        stride: geometry.stride_bytes,
        bytes_per_pixel: geometry.bytes_per_pixel,
        pixel_format: FramebufferPixelFormat::Xrgb8888,
    }
}

/// Post-MMU registration: turn the negotiated boot framebuffer into the
/// display backend consumed by the kernel display object registry. Returns
/// `None` (headless fallback) until [`negotiate`] has succeeded exactly once.
pub fn initialize_backend() -> Option<Arc<dyn DisplayBackend>> {
    Some(Arc::new(PiBootFramebufferBackend {
        framebuffer: negotiated_framebuffer()?,
    }))
}

/// Direct-to-scanout backend over the VC-allocated buffer. Present copies
/// whole frames; present_damage copies whole rows inside the damaged rect
/// (row granularity matches how VideoCore pitches the surface, keeping copy
/// math trivially correct under device-memory attributes).
struct PiBootFramebufferBackend {
    framebuffer: FramebufferInfo,
}

impl DisplayBackend for PiBootFramebufferBackend {
    fn info(&self) -> DisplayOutputInfo {
        DisplayOutputInfo {
            backend: DisplayOutputBackend::BootFramebuffer as u32,
            state: if NEGOTIATED.load(Ordering::Acquire) {
                DisplayOutputState::Connected as u32
            } else {
                DisplayOutputState::Disconnected as u32
            },
            pixel_format: match self.framebuffer.pixel_format {
                FramebufferPixelFormat::Xrgb8888 => DisplayPixelFormat::Xrgb8888 as u32,
                FramebufferPixelFormat::Bgrx8888 => DisplayPixelFormat::Bgrx8888 as u32,
            },
            reserved: 0,
            width: self.framebuffer.width as u32,
            height: self.framebuffer.height as u32,
            stride: self.framebuffer.stride as u32,
            bytes_per_pixel: self.framebuffer.bytes_per_pixel as u32,
            byte_len: self.framebuffer.byte_len as u64,
            present_count: 0,
        }
    }

    fn present(&self, frame: &[u8]) -> Result<(), DisplayOutputError> {
        if frame.len() != self.framebuffer.byte_len {
            return Err(DisplayOutputError::BufferTooSmall);
        }

        let target = self.framebuffer.physical_base.as_u64() as *mut u8;
        // SAFETY: physical_base is mapped device memory via the platform MMU
        // region list (see the image main flow) and outlives this object.
        unsafe {
            ptr::copy_nonoverlapping(frame.as_ptr(), target, frame.len());
        }
        Ok(())
    }

    fn present_damage(
        &self,
        frame: &[u8],
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    ) -> Result<(), DisplayOutputError> {
        if frame.len() != self.framebuffer.byte_len {
            return Err(DisplayOutputError::BufferTooSmall);
        }

        let start_x = x.max(0) as usize;
        let start_y = y.max(0) as usize;
        let end_x = ((x + width as i32).max(0) as usize).min(self.framebuffer.width);
        let end_y = ((y + height as i32).max(0) as usize).min(self.framebuffer.height);
        if start_x >= end_x || start_y >= end_y {
            return Ok(());
        }

        let row_bytes = self.framebuffer.stride * self.framebuffer.bytes_per_pixel;
        let bytes_per_pixel = self.framebuffer.bytes_per_pixel;
        let target = self.framebuffer.physical_base.as_u64() as *mut u8;
        let copy_start = start_x * bytes_per_pixel;
        let copy_end = end_x * bytes_per_pixel;

        for row in start_y..end_y {
            let row_offset = row * row_bytes;
            // SAFETY: see present(); offsets stay inside the mapped scanout.
            unsafe {
                ptr::copy_nonoverlapping(
                    frame.as_ptr().add(row_offset + copy_start),
                    target.add(row_offset + copy_start),
                    copy_end - copy_start,
                );
            }
        }
        Ok(())
    }
}
