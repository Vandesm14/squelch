use std::{
  collections::HashSet,
  net::{SocketAddr, UdpSocket},
};

use bincode::config::{Configuration, standard};
use squelch::{MAX_PACKET_SIZE, Packet};

fn main() -> std::io::Result<()> {
  let socket = UdpSocket::bind("0.0.0.0:1837")?;
  socket
    .set_broadcast(true)
    .expect("set_broadcast to true should succeed");

  let mut clients: HashSet<SocketAddr> = HashSet::new();

  let mut buf = [0; MAX_PACKET_SIZE];
  loop {
    let (_, src) = socket.recv_from(&mut buf)?;
    match bincode::decode_from_slice::<Packet, Configuration>(
      &buf,
      bincode::config::standard(),
    ) {
      Ok((packet, _)) => match packet {
        Packet::Ping => {
          clients.insert(src);
          println!("Now {} clients", clients.len());
        }
        Packet::Audio(bytes) => {
          for client in clients.iter().filter(|c| **c != src) {
            socket
              .send_to(
                &bincode::encode_to_vec(Packet::Audio(bytes), standard())
                  .unwrap(),
                client,
              )
              .unwrap();
          }
        }
      },
      Err(err) => eprintln!("Error decoding packet: {err:?}"),
    }
  }
}
