//! Protocol-conformance harness for the VideoCore mailbox driver.
//!
//! Lives outside the lib test harness because `serviceos-platform-raspi5`
//! links `serviceos-kernel-core`, whose kernel-image `#[global_allocator]`
//! would otherwise be selected for a host binary that never initializes the
//! kernel heap (the reason every firmware crate in this workspace sets
//! `[lib] test = false`). This target instead includes the production
//! `mailbox.rs` by path — it is dependency-free by design — so the real wire
//! builder/parser/validation/geometry code runs on the host under std's
//! default allocator.

#[path = "../src/mailbox.rs"]
mod mailbox;

#[cfg(test)]
mod geometry {
    use super::mailbox::{assemble_geometry, bus_to_physical, physical_to_bus, VC_BUS_SDRAM_ALIAS};

    const SAMPLE_BUS: u64 = VC_BUS_SDRAM_ALIAS | 0x00F3_0000;
    // pitch(7680) * height(1080): exactly what a linear XRGB8888 surface needs.
    const FRAME_BYTES: u32 = 7680 * 1080;

    #[test]
    fn assemble_accepts_canonical_allocation_and_pitch() {
        let info =
            assemble_geometry(SAMPLE_BUS, FRAME_BYTES, 1920, 1080, 1920 * 4, 4)
                .expect("canonical allocation");
        assert_eq!(info.physical_base, 0x00F3_0000);
        assert_eq!(info.width, 1920);
        assert_eq!(info.height, 1080);
        assert_eq!(info.stride_bytes, 7680);
        assert_eq!(info.byte_len, FRAME_BYTES as usize);
        assert_eq!(info.bytes_per_pixel, 4);
    }

    #[test]
    fn assemble_rejects_zero_geometry() {
        assert!(assemble_geometry(SAMPLE_BUS, FRAME_BYTES, 0, 1080, 7680, 4).is_none());
        assert!(assemble_geometry(SAMPLE_BUS, FRAME_BYTES, 1920, 0, 7680, 4).is_none());
        assert!(assemble_geometry(SAMPLE_BUS, FRAME_BYTES, 1920, 1080, 7680, 0).is_none());
    }

    #[test]
    fn assemble_rejects_stride_narrower_than_visible_row() {
        assert!(
            assemble_geometry(SAMPLE_BUS, FRAME_BYTES, 1920, 1080, 1916, 4).is_none()
        );
    }

    #[test]
    fn assemble_rejects_allocation_smaller_than_scanout() {
        assert!(assemble_geometry(SAMPLE_BUS, 1024, 1920, 1080, 7680, 4).is_none());
    }

    #[test]
    fn assemble_rejects_non_sdram_bus_addresses() {
        assert!(
            assemble_geometry(0x0001_3880, FRAME_BYTES, 1920, 1080, 7680, 4).is_none()
        );
    }

    #[test]
    fn bus_translation_roundtrip_and_window_rejection() {
        assert_eq!(physical_to_bus(0x00F3_0000), Some(0x40F3_0000));
        assert_eq!(bus_to_physical(0xC000_0000 | 0x00F3_0000), Some(0x00F3_0000));
        assert_eq!(bus_to_physical(0x40F3_0000), Some(0x00F3_0000));
        // Peripheral windows do not map into SDRAM.
        assert_eq!(bus_to_physical(0x0001_3880), None);
        assert_eq!(bus_to_physical(0x8000_0000 | 0x1000), None);
        // Above the 32-bit legacy window nothing translates.
        assert_eq!(physical_to_bus(0x1_0000_0000), None);
    }
}
