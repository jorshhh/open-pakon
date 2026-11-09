//! The scanner as the command layer sees it: frames out, replies in, image
//! bytes in, control transfers. [`pakon_usb::Device`] is the real one; tests
//! use a simulated scanner.

use std::time::Duration;

use pakon_usb::device::{Control, EP_CMD_IN, EP_CMD_OUT, EP_IMAGE_IN};

use crate::ScanError;

pub trait Link {
    /// Write one command frame on EP1 OUT; returns the bytes sent.
    fn send_cmd(&mut self, wire: &[u8], timeout: Duration) -> Result<usize, ScanError>;
    /// Read one reply from EP1 IN.
    fn recv_cmd(&mut self, buf: &mut [u8], timeout: Duration) -> Result<usize, ScanError>;
    /// Read image bytes from EP 0x86.
    fn recv_image(&mut self, buf: &mut [u8], timeout: Duration) -> Result<usize, ScanError>;
    /// An EP0 control transfer; IN returns up to `length` bytes.
    fn control(&mut self, x: &Control, timeout: Duration) -> Result<Vec<u8>, ScanError>;
}

impl Link for pakon_usb::Device {
    fn send_cmd(&mut self, wire: &[u8], timeout: Duration) -> Result<usize, ScanError> {
        Ok(self.send(EP_CMD_OUT, wire, timeout)?)
    }
    fn recv_cmd(&mut self, buf: &mut [u8], timeout: Duration) -> Result<usize, ScanError> {
        Ok(self.recv(EP_CMD_IN, buf, timeout)?)
    }
    fn recv_image(&mut self, buf: &mut [u8], timeout: Duration) -> Result<usize, ScanError> {
        Ok(self.recv(EP_IMAGE_IN, buf, timeout)?)
    }
    fn control(&mut self, x: &Control, timeout: Duration) -> Result<Vec<u8>, ScanError> {
        Ok(pakon_usb::Device::control(self, x, timeout)?)
    }
}
