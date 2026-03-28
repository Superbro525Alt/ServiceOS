use smoltcp::{
    phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken},
    time::Instant,
};

use serviceos_userspace_runtime as rt;
use rt::PacketInterfaceInfo;

use crate::consts::MAX_FRAME_BYTES;

pub(crate) struct KernelPacketDevice {
    pub(crate) handle: rt::Handle,
    pub(crate) info: PacketInterfaceInfo,
    rx_buffer: [u8; MAX_FRAME_BYTES],
    tx_buffer: [u8; MAX_FRAME_BYTES],
}

impl KernelPacketDevice {
    pub(crate) fn new(handle: rt::Handle, info: PacketInterfaceInfo) -> Self {
        Self {
            handle,
            info,
            rx_buffer: [0; MAX_FRAME_BYTES],
            tx_buffer: [0; MAX_FRAME_BYTES],
        }
    }
}

pub(crate) struct KernelRxToken<'a> {
    buffer: &'a mut [u8],
}

pub(crate) struct KernelTxToken<'a> {
    handle: rt::Handle,
    buffer: &'a mut [u8],
}

impl Device for KernelPacketDevice {
    type RxToken<'a>
        = KernelRxToken<'a>
    where
        Self: 'a;
    type TxToken<'a>
        = KernelTxToken<'a>
    where
        Self: 'a;

    fn receive(&mut self, _timestamp: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        match rt::packet_interface_receive_nonblocking(self.handle, &mut self.rx_buffer) {
            Ok(length) => Some((
                KernelRxToken {
                    buffer: &mut self.rx_buffer[..length],
                },
                KernelTxToken {
                    handle: self.handle,
                    buffer: &mut self.tx_buffer,
                },
            )),
            Err(rt::Error::QueueEmpty) => None,
            Err(_) => None,
        }
    }

    fn transmit(&mut self, _timestamp: Instant) -> Option<Self::TxToken<'_>> {
        Some(KernelTxToken {
            handle: self.handle,
            buffer: &mut self.tx_buffer,
        })
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.medium = Medium::Ethernet;
        caps.max_transmission_unit = self.info.mtu as usize;
        caps.max_burst_size = Some(1);
        caps
    }
}

impl RxToken for KernelRxToken<'_> {
    fn consume<R, F>(self, f: F) -> R
    where
        F: FnOnce(&[u8]) -> R,
    {
        f(self.buffer)
    }
}

impl TxToken for KernelTxToken<'_> {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        let result = f(&mut self.buffer[..len]);
        let _ = rt::packet_interface_transmit(self.handle, &self.buffer[..len]);
        result
    }
}
