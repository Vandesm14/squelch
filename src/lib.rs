pub mod fx;
pub mod jitter;

use std::{sync::LazyLock, time::Duration};

use bincode::{Decode, Encode};

pub const TX_BUFFER_SIZE: usize = 256;
pub const MAX_PACKET_SIZE: usize = 4 * TX_BUFFER_SIZE + 8;

pub type TxBuffer = [f32; TX_BUFFER_SIZE];

#[derive(Debug, Clone, Encode, Decode)]
#[allow(clippy::large_enum_variant)]
pub enum Packet {
  Ping,
  Audio(TxBuffer),
}

pub fn map_would_block<T>(result: std::io::Result<T>) -> std::io::Result<()> {
  match result {
    Ok(_) => std::io::Result::Ok(()),
    Err(e) => match e.kind() {
      std::io::ErrorKind::WouldBlock => Ok(()),
      _ => Err(e),
    },
  }
}

pub static WAIT_DURATION: LazyLock<Duration> = LazyLock::new(|| {
  Duration::from_secs_f32(1.0 / (44100.0 / TX_BUFFER_SIZE as f32))
});
