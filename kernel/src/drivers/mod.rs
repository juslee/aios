//! Device drivers.

pub mod virtio_blk;
pub mod virtio_common;
pub mod virtio_gpu;
// Allow dead_code until Step 5 wires input init into main.rs boot sequence.
#[allow(dead_code)]
pub mod virtio_input;
