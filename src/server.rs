use std::{
  collections::{HashMap, VecDeque},
  net::{SocketAddr, UdpSocket},
  sync::mpsc::channel,
  time::{Duration, Instant},
};

use bincode::config::{Configuration, standard};
use squelch::{MAX_PACKET_SIZE, Packet, TX_BUFFER_SIZE};

fn main() -> std::io::Result<()> {
  let wait_duration =
    Duration::from_secs_f32(1.0 / (44100.0 / TX_BUFFER_SIZE as f32));

  let socket = UdpSocket::bind("0.0.0.0:1837")?;
  socket
    .set_broadcast(true)
    .expect("set_broadcast to true should succeed");

  let (audio_tx, audio_rx) = channel::<(SocketAddr, [f32; TX_BUFFER_SIZE])>();
  let (ping_tx, ping_rx) = channel::<SocketAddr>();

  let cloned_socket = socket.try_clone().unwrap();
  std::thread::spawn(move || {
    let mut last_sent = Instant::now();
    let mut client_chunks: HashMap<
      SocketAddr,
      VecDeque<[f32; TX_BUFFER_SIZE]>,
    > = HashMap::new();

    loop {
      while let Ok(src) = ping_rx.try_recv() {
        client_chunks.entry(src).or_default();
        println!("Now {} clients", client_chunks.len());
      }

      while let Ok((src, bytes)) = audio_rx.try_recv() {
        client_chunks
          .entry(src)
          .and_modify(|e| {
            // REMOVE THIS.
            e.push_back(bytes);
          })
          .or_insert_with(|| {
            let mut v = VecDeque::new();
            v.push_back(bytes);
            v
          });

        println!("receive from {} as {:?}", src, Instant::now());
      }

      if last_sent.elapsed() > wait_duration {
        let mut buf = [0f32; TX_BUFFER_SIZE];
        for (_, chunks) in client_chunks.iter_mut() {
          if let Some(samples) = chunks.pop_front() {
            for (b, s) in buf.iter_mut().zip(samples.iter()) {
              *b += s;
              *b = b.clamp(-1.0, 1.0);
            }
          }
        }

        if buf.iter().any(|a| *a != 0.0) {
          for (client, _) in client_chunks.iter() {
            cloned_socket
              .send_to(
                &bincode::encode_to_vec(Packet::Audio(buf), standard())
                  .unwrap(),
                client,
              )
              .unwrap();
          }
        }

        last_sent = Instant::now();
      }
    }
  });

  let mut buf = [0; MAX_PACKET_SIZE];
  loop {
    let (_, src) = socket.recv_from(&mut buf)?;
    match bincode::decode_from_slice::<Packet, Configuration>(
      &buf,
      bincode::config::standard(),
    ) {
      Ok((packet, _)) => match packet {
        Packet::Ping => {
          ping_tx.send(src).unwrap();
        }
        Packet::Audio(bytes) => {
          audio_tx.send((src, bytes)).unwrap();
        }
      },
      Err(err) => eprintln!("Error decoding packet: {err:?}"),
    }
  }
}
