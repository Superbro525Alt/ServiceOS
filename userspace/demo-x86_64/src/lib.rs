#![no_std]

const MAGIC: [u8; 8] = *b"SOSUIMG\0";
const ABI_VERSION: u32 = 1;
const HEADER_LEN: u32 = 48;
const IMAGE_BASE: u64 = 0x0000_4000_0000_0000;
const ENTRY_OFFSET: u64 = 0;
const CODE_SIZE: u64 = 29;
const USER_STACK_TOP: u64 = 0x0000_7fff_ffff_0000;

// x86_64 userspace program:
//   mov eax, 0          ; abi_version
//   int 0x80
//   mov ebx, eax
//   mov eax, 1          ; monotonic_now
//   int 0x80
//   add eax, ebx
//   mov edi, eax        ; exit code
//   mov eax, 2          ; exit
//   int 0x80
//   ud2
const CODE: [u8; CODE_SIZE as usize] = [
    0xb8, 0x00, 0x00, 0x00, 0x00, 0xcd, 0x80, 0x89, 0xc3, 0xb8, 0x01, 0x00, 0x00, 0x00, 0xcd, 0x80,
    0x01, 0xd8, 0x89, 0xc7, 0xb8, 0x02, 0x00, 0x00, 0x00, 0xcd, 0x80, 0x0f, 0x0b,
];

pub const FLAT_IMAGE: [u8; HEADER_LEN as usize + CODE_SIZE as usize] = {
    let mut image = [0u8; HEADER_LEN as usize + CODE_SIZE as usize];
    image[0] = MAGIC[0];
    image[1] = MAGIC[1];
    image[2] = MAGIC[2];
    image[3] = MAGIC[3];
    image[4] = MAGIC[4];
    image[5] = MAGIC[5];
    image[6] = MAGIC[6];
    image[7] = MAGIC[7];

    write_u32_le(&mut image, 8, ABI_VERSION);
    write_u32_le(&mut image, 12, HEADER_LEN);
    write_u64_le(&mut image, 16, IMAGE_BASE);
    write_u64_le(&mut image, 24, ENTRY_OFFSET);
    write_u64_le(&mut image, 32, CODE_SIZE);
    write_u64_le(&mut image, 40, USER_STACK_TOP);

    let mut index = HEADER_LEN as usize;
    while index < image.len() {
        image[index] = CODE[index - HEADER_LEN as usize];
        index += 1;
    }

    image
};

pub const fn image() -> &'static [u8] {
    &FLAT_IMAGE
}

pub const fn expected_exit_low32() -> u32 {
    0x0002_0000
}

const fn write_u32_le(buffer: &mut [u8], offset: usize, value: u32) {
    let bytes = value.to_le_bytes();
    buffer[offset] = bytes[0];
    buffer[offset + 1] = bytes[1];
    buffer[offset + 2] = bytes[2];
    buffer[offset + 3] = bytes[3];
}

const fn write_u64_le(buffer: &mut [u8], offset: usize, value: u64) {
    let bytes = value.to_le_bytes();
    buffer[offset] = bytes[0];
    buffer[offset + 1] = bytes[1];
    buffer[offset + 2] = bytes[2];
    buffer[offset + 3] = bytes[3];
    buffer[offset + 4] = bytes[4];
    buffer[offset + 5] = bytes[5];
    buffer[offset + 6] = bytes[6];
    buffer[offset + 7] = bytes[7];
}
