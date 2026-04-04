use core::array;

use serviceos_userspace_runtime as rt;

pub struct SurfaceBuffers<const N: usize> {
    handles: [rt::Handle; N],
    mapped: [Option<rt::MappedMemory>; N],
    front: usize,
}

impl<const N: usize> SurfaceBuffers<N> {
    pub fn new(
        surface_handle: rt::Handle,
        buffer_width: u32,
        buffer_height: u32,
        stride_pixels: u32,
        buffer_bytes: usize,
    ) -> rt::Result<Self> {
        let mut handles = [rt::INVALID_HANDLE; N];
        let mut mapped: [Option<rt::MappedMemory>; N] = array::from_fn(|_| None);

        for slot in 0..N {
            let buffer_handle = match rt::memory_create(buffer_bytes, true) {
                Ok(handle) => handle,
                Err(error) => {
                    close_handles(&handles);
                    return Err(error);
                }
            };
            if let Err(error) = rt::surface_attach_buffer_slot(
                surface_handle,
                slot as u32,
                buffer_handle,
                buffer_width,
                buffer_height,
                stride_pixels,
            ) {
                let _ = rt::handle_close(buffer_handle);
                close_handles(&handles);
                return Err(error);
            }
            let mapped_buffer = match rt::MappedMemory::map(buffer_handle, buffer_bytes, true) {
                Ok(buffer) => buffer,
                Err(error) => {
                    let _ = rt::handle_close(buffer_handle);
                    close_handles(&handles);
                    return Err(error);
                }
            };
            handles[slot] = buffer_handle;
            mapped[slot] = Some(mapped_buffer);
        }

        Ok(Self {
            handles,
            mapped,
            front: 0,
        })
    }

    pub fn current(&mut self) -> (u32, &mut rt::MappedMemory) {
        (
            self.front as u32,
            self.mapped[self.front].as_mut().expect("surface buffer slot mapped"),
        )
    }

    pub fn advance(&mut self) -> (u32, &mut rt::MappedMemory) {
        self.front = (self.front + 1) % N;
        self.current()
    }
}

impl<const N: usize> Drop for SurfaceBuffers<N> {
    fn drop(&mut self) {
        close_handles(&self.handles);
    }
}

fn close_handles<const N: usize>(handles: &[rt::Handle; N]) {
    for handle in handles.iter().copied() {
        if handle != rt::INVALID_HANDLE {
            let _ = rt::handle_close(handle);
        }
    }
}
