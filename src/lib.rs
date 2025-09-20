use bincode::{Decode, Encode};

#[derive(Debug, Clone, Encode, Decode)]
pub enum Packet {
  Ping,
  Pong,
  Audio([f32; 16]),
}
