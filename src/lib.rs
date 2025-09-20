use bincode::{Decode, Encode};

pub const TX_BUFFER_SIZE: usize = 16;

#[derive(Debug, Clone, Encode, Decode)]
pub enum Packet {
  Ping,
  Pong,
  Audio([f32; TX_BUFFER_SIZE]),
}
