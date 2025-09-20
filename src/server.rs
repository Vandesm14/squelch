use std::net::{SocketAddr, UdpSocket};

use bincode::config::Configuration;
use ham_radio_rs::Packet;

fn main() -> std::io::Result<()> {
  let socket = UdpSocket::bind("0.0.0.0:1837")?;
  socket
    .set_broadcast(true)
    .expect("set_broadcast to true should succeed");

  let mut clients: Vec<SocketAddr> = Vec::new();

  let mut buf = [0u8; 1024]; // Allocate a buffer to receive data
  loop {
    let (amt, src) = socket.recv_from(&mut buf)?;
    let received_data = &buf[..amt];
    match bincode::decode_from_slice::<Packet, Configuration>(
      received_data,
      bincode::config::standard(),
    ) {
      Ok(packet) => {
        println!("packet: {packet:?}");
        clients.push(src);
      }
      Err(_) => todo!(),
    }

    for client in clients.iter() {
      socket.send_to(
        &bincode::encode_to_vec(Packet::Pong, bincode::config::standard())
          .unwrap(),
        client,
      )?;
    }
  }
}
