/* Userspace flat-image link script.
 *
 * Every output section is scripted explicitly AND pinned to an explicit
 * program header (`:phdr` bindings below), because lld otherwise derives
 * LOAD-segment boundaries from input-section attributes: a mergeable `.data`
 * (SHF_MERGE|SHF_STRINGS) can be folded into the preceding read-only
 * segment, leaving static mutable state mapped read-only, and unplaced
 * orphans have previously overlapped .got onto .text for larger binaries.
 *
 * The three segments stay disjoint and monotonic from the fixed image base
 * the kernel loader expects (see kernel/core/src/user/loader.rs); their
 * permissions are what the flat-image wrapper copies into the bundle header
 * (executable_limit / writable_offset, see userspace/catalog/build_support).
 */
ENTRY(_start)

PHDRS
{
    text   PT_LOAD FLAGS(5); /* R+X */
    rodata PT_LOAD FLAGS(4); /* R */
    rw     PT_LOAD FLAGS(6); /* R+W */
}

SECTIONS
{
    . = 0x400000000000;

    /* Under -Ccode-model=large rustc places local functions into
     * .ltext.<symbol> sections (cf. .ldata/.lbss); match that family here,
     * otherwise lld treats them as orphans and glues executable bytes onto
     * whichever read-only page comes next. */
    .text : ALIGN(4K) {
        *(.text.start)
        *(.text .text.*)
        *(.ltext .ltext.*)
    } :text

    .rodata : ALIGN(4K) {
        *(.rodata .rodata.*)
        *(.eh_frame_hdr)
        KEEP(*(.eh_frame))
    } :rodata

    /* Mutable statics are explicitly not mergeable in the output image:
     * they share the writable segment instead of being folded into a
     * read-only page by attribute-based orphan placement. */
    .data : ALIGN(4K) {
        *(.data .data.*)
        *(.sdata .sdata.*)
        *(.lrodata .lrodata.*)
        *(.ldata .ldata.*)
    } :rw

    /* GOT entries are runtime-mutated relocations and belong in the
     * writable segment as well. */
    .got : ALIGN(4K) {
        *(.got .got.*)
        *(.got.plt)
    } :rw

    .bss : ALIGN(4K) {
        *(.bss .bss.*)
        *(.sbss .sbss.*)
        *(.lbss .lbss.*)
        *(COMMON)
    } :rw
}
