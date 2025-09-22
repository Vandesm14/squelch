use std::{
  fs::File,
  io::BufReader,
  net::{SocketAddr, UdpSocket},
  path::Path,
  time::Duration,
};

use bincode::config::standard;
use clap::Parser;
use hound::WavReader;
use minimp3::{Decoder, Frame};

use squelch::{Packet, TX_BUFFER_SIZE};

/// Play audio file to ham radio server
#[derive(Debug, Clone, Parser)]
#[command(version, about, long_about = None)]
pub struct Cli {
  /// The socket address to connect to
  #[arg(short, long)]
  pub address: SocketAddr,

  /// Path to the audio file (WAV or MP3)
  #[arg(value_name = "FILE")]
  pub file: String,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
  let args = Cli::parse();

  let file_path = Path::new(&args.file);
  let extension = file_path
    .extension()
    .and_then(|ext| ext.to_str())
    .ok_or("Unable to determine file extension")?
    .to_lowercase();

  println!("Playing file: {}", args.file);
  println!("Connecting to server: {}", args.address);

  let socket = UdpSocket::bind("0.0.0.0:0")?;

  // Send initial ping
  let ping_packet = bincode::encode_to_vec(Packet::Ping, standard())?;
  socket.send_to(&ping_packet, args.address)?;
  println!("Sent ping to server");

  let samples = match extension.as_str() {
    "wav" => read_wav_file(&args.file)?,
    "mp3" => read_mp3_file(&args.file)?,
    _ => return Err(format!("Unsupported file format: {}", extension).into()),
  };

  println!("Loaded {} samples", samples.len());

  // Stream audio data in chunks
  let mut buffer = [0f32; TX_BUFFER_SIZE];
  for chunk in samples.chunks_exact(TX_BUFFER_SIZE) {
    // Copy chunk to buffer, padding with zeros if necessary
    for (i, &sample) in chunk.iter().enumerate() {
      buffer[i] = sample;
    }

    let audio_packet =
      bincode::encode_to_vec(Packet::Audio(buffer), standard())?;
    socket.send_to(&audio_packet, args.address)?;

    std::thread::sleep(Duration::from_secs_f32(0.0057));
  }

  Ok(())
}

fn read_wav_file(
  file_path: &str,
) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
  let mut reader = WavReader::open(file_path)?;
  let spec = reader.spec();

  println!("WAV file info:");
  println!("  Sample rate: {} Hz", spec.sample_rate);
  println!("  Channels: {}", spec.channels);
  println!("  Bits per sample: {}", spec.bits_per_sample);

  let mut samples = Vec::new();

  match spec.sample_format {
    hound::SampleFormat::Float => {
      for sample_result in reader.samples::<f32>() {
        let sample = sample_result?;
        samples.push(sample);
      }
    }
    hound::SampleFormat::Int => {
      match dbg!(spec.bits_per_sample) {
        16 => {
          for sample_result in reader.samples::<i16>() {
            let sample = sample_result?;
            // Convert i16 to f32 in range [-1.0, 1.0]
            samples.push(sample as f32 / i16::MAX as f32);
          }
        }
        32 => {
          for sample_result in reader.samples::<i32>() {
            let sample = sample_result?;
            // Convert i32 to f32 in range [-1.0, 1.0]
            samples.push(sample as f32 / i32::MAX as f32);
          }
        }
        _ => {
          return Err(
            format!("Unsupported bit depth: {}", spec.bits_per_sample).into(),
          );
        }
      }
    }
  }

  // If stereo, convert to mono by averaging channels
  if spec.channels == 2 {
    let mono_samples: Vec<f32> = samples
      .chunks_exact(2)
      .map(|pair| (pair[0] + pair[1]) / 2.0)
      .collect();
    return Ok(mono_samples);
  }

  Ok(samples)
}

fn read_mp3_file(
  file_path: &str,
) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
  let file = File::open(file_path)?;
  let mut decoder = Decoder::new(BufReader::new(file));
  let mut samples = Vec::new();

  println!("MP3 file info:");

  loop {
    match decoder.next_frame() {
      Ok(Frame {
        data,
        sample_rate,
        channels,
        ..
      }) => {
        if samples.is_empty() {
          println!("  Sample rate: {} Hz", sample_rate);
          println!("  Channels: {}", channels);
        }

        // Convert i16 samples to f32 in range [-1.0, 1.0]
        let frame_samples: Vec<f32> = data
          .iter()
          .map(|&sample| sample as f32 / i16::MAX as f32)
          .collect();

        // If stereo, convert to mono by averaging channels
        if channels == 2 {
          let mono_samples: Vec<f32> = frame_samples
            .chunks_exact(2)
            .map(|pair| (pair[0] + pair[1]) / 2.0)
            .collect();
          samples.extend(mono_samples);
        } else {
          samples.extend(frame_samples);
        }
      }
      Err(minimp3::Error::Eof) => break,
      Err(e) => return Err(e.into()),
    }
  }

  Ok(samples)
}
