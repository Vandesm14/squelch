use bincode::{Decode, Encode};

pub const TX_BUFFER_SIZE: usize = 256;
pub const MAX_PACKET_SIZE: usize = 4 * TX_BUFFER_SIZE + 8;

#[derive(Debug, Clone, Encode, Decode)]
#[allow(clippy::large_enum_variant)]
pub enum Packet {
  Ping,
  Audio([f32; TX_BUFFER_SIZE]),
}
