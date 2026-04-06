use serviceos_userspace_runtime as rt;

pub struct FirstPresentSurface {
    surface_handle: rt::Handle,
    shown: bool,
}

impl FirstPresentSurface {
    pub const fn new(surface_handle: rt::Handle) -> Self {
        Self {
            surface_handle,
            shown: false,
        }
    }

    pub fn present(&mut self, buffer_slot: u32, width: u32, height: u32) -> rt::Result<()> {
        rt::surface_present_buffer_slot(self.surface_handle, buffer_slot, 0, 0, width, height)?;
        if !self.shown {
            rt::surface_set_visibility(self.surface_handle, true)?;
            self.shown = true;
        }
        Ok(())
    }
}

pub struct DeferredStartup {
    pending: bool,
}

impl DeferredStartup {
    pub const fn new() -> Self {
        Self { pending: true }
    }

    pub fn run<F>(&mut self, task: F) -> rt::Result<bool>
    where
        F: FnOnce() -> rt::Result<bool>,
    {
        if !self.pending {
            return Ok(false);
        }
        self.pending = false;
        task()
    }
}
