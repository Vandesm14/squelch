use std::{
  fs::File,
  io::BufWriter,
  net::{SocketAddr, UdpSocket},
  sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc,
  },
  time::{SystemTime, UNIX_EPOCH},
};

use bincode::config::{Configuration, standard};
use clap::Parser;
use hound::{WavSpec, WavWriter};

use squelch::{MAX_PACKET_SIZE, Packet};

/// Record sound from ham radio server to WAV file
#[derive(Debug, Clone, Parser)]
#[command(version, about, long_about = None)]
pub struct Cli {
  /// The socket address of the ham radio server to record from
  #[arg(short, long)]
  pub address: SocketAddr,

  /// Output WAV file path (optional - will generate timestamped filename if not provided)
  #[arg(short, long)]
  pub output: Option<String>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
  let args = Cli::parse();

  // Generate output filename if not provided
  let output_path = match args.output {
    Some(path) => path,
    None => {
      let timestamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
      format!("recording_{}.wav", timestamp)
    }
  };

  println!("Recording to: {}", output_path);
  println!("Server address: {}", args.address);
  println!("Press Ctrl+C to stop recording and save the file...\n");

  // Set up the WAV file writer
  let spec = WavSpec {
    channels: 1,
    sample_rate: 44100,
    bits_per_sample: 32, // Using 32-bit float samples
    sample_format: hound::SampleFormat::Float,
  };

  let file = File::create(&output_path)?;
  let wav_writer =
    Arc::new(Mutex::new(WavWriter::new(BufWriter::new(file), spec)?));

  // Set up UDP socket to receive audio from server
  let socket = UdpSocket::bind("0.0.0.0:0")?;
  socket.set_nonblocking(true)?;

  // Send initial ping to server to start receiving audio
  let ping_packet = bincode::encode_to_vec(Packet::Ping, standard())?;
  socket.send_to(&ping_packet, args.address)?;
  println!("Sent ping to server at {}", args.address);

  println!("Listening for audio packets from server...");

  // Channel for sending audio data from the UDP thread to the main thread
  let (audio_tx, audio_rx) = mpsc::channel::<Vec<f32>>();

  // Set up Ctrl+C handler
  let running = Arc::new(AtomicBool::new(true));
  let running_clone = running.clone();

  ctrlc::set_handler(move || {
    println!("\nReceived Ctrl+C, stopping recording...");
    running_clone.store(false, Ordering::SeqCst);
  })?;

  // Spawn UDP receiving thread
  let running_udp = running.clone();
  let socket_clone = socket.try_clone()?;
  std::thread::spawn(move || {
    let mut buf = [0; MAX_PACKET_SIZE];

    while running_udp.load(Ordering::SeqCst) {
      match socket_clone.recv_from(&mut buf) {
        Ok((size, _)) => {
          // Decode the packet
          match bincode::decode_from_slice::<Packet, Configuration>(
            &buf[..size],
            standard(),
          ) {
            Ok((packet, _)) => match packet {
              Packet::Ping => {
                // Ignore ping packets
              }
              Packet::Audio(samples) => {
                // Send audio samples to main thread
                if let Err(e) = audio_tx.send(samples.to_vec()) {
                  eprintln!("Failed to send audio data: {}", e);
                  break;
                }
              }
            },
            Err(err) => {
              eprintln!("Failed to decode packet: {:?}", err);
            }
          }
        }
        Err(e) => {
          if e.kind() != std::io::ErrorKind::WouldBlock {
            eprintln!("UDP receive error: {}", e);
          }
        }
      }

      // Small delay to prevent busy waiting
      std::thread::sleep(std::time::Duration::from_millis(1));
    }
  });

  println!("Recording started!");

  let mut total_samples = 0u64;
  let mut last_update = std::time::Instant::now();

  // Main loop - process audio data and write to WAV file
  while running.load(Ordering::SeqCst) {
    // Process any pending audio data
    while let Ok(audio_data) = audio_rx.try_recv() {
      let mut writer = wav_writer.lock().unwrap();
      for &sample in &audio_data {
        if let Err(e) = writer.write_sample(sample) {
          eprintln!("Failed to write audio sample: {}", e);
          running.store(false, Ordering::SeqCst);
          break;
        }
      }
      total_samples += audio_data.len() as u64;

      // Print progress every second
      if last_update.elapsed().as_secs() >= 1 {
        let duration_secs = total_samples as f64 / 44100.0;
        print!(
          "\rRecording: {:.1}s ({} samples)",
          duration_secs, total_samples
        );
        std::io::Write::flush(&mut std::io::stdout()).unwrap();
        last_update = std::time::Instant::now();
      }
    }

    // Small delay to prevent busy waiting
    std::thread::sleep(std::time::Duration::from_millis(10));
  }

  // Signal UDP thread to stop (it will exit when running becomes false)

  // Process any remaining audio data
  while let Ok(audio_data) = audio_rx.try_recv() {
    let mut writer = wav_writer.lock().unwrap();
    for &sample in &audio_data {
      if let Err(e) = writer.write_sample(sample) {
        eprintln!("Failed to write final audio sample: {}", e);
        break;
      }
    }
    total_samples += audio_data.len() as u64;
  }

  // Finalize the WAV file
  let writer = Arc::try_unwrap(wav_writer)
    .map_err(|_| "Failed to unwrap Arc")?
    .into_inner()
    .map_err(|_| "Failed to unwrap Mutex")?;
  writer.finalize()?;

  let final_duration = total_samples as f64 / 44100.0;
  println!("\n\nRecording completed!");
  println!("Total samples: {}", total_samples);
  println!("Duration: {:.2} seconds", final_duration);
  println!("File saved: {}", output_path);

  Ok(())
}
