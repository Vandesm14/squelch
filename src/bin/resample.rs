use std::error::Error;
use std::path::PathBuf;

use audioadapter_buffers::direct::SequentialSliceOfVecs;
use clap::Parser;
use hound::{SampleFormat, WavReader, WavSpec, WavWriter};
use rubato::{Fft, FixedSync, Resampler};

#[derive(Parser, Debug)]
#[command(
  about = "Resample a WAV file to a target sample rate using rubato (FFT resampler)."
)]
struct Args {
  /// Input WAV path
  input: PathBuf,

  /// Output WAV path
  output: PathBuf,

  /// Target sample rate (Hz)
  #[arg(short = 'r', long)]
  rate: u32,

  /// Rubato chunk size (frames per channel)
  #[arg(long, default_value_t = 1024)]
  chunk_size: usize,

  /// Rubato FFT sub-chunks (trade-off CPU/latency)
  #[arg(long, default_value_t = 2)]
  sub_chunks: usize,

  /// Hard-clip output samples to [-1.0, 1.0]
  #[arg(long, default_value_t = true)]
  clip: bool,
}

fn main() -> Result<(), Box<dyn Error>> {
  let args = Args::parse();

  let mut reader = WavReader::open(&args.input)?;
  let spec = reader.spec();

  let src_rate = spec.sample_rate;
  let channels = spec.channels as usize;
  if channels == 0 {
    return Err("input WAV has 0 channels".into());
  }
  if args.rate == 0 {
    return Err("target sample rate must be > 0".into());
  }
  if args.chunk_size == 0 {
    return Err("chunk_size must be > 0".into());
  }
  if args.sub_chunks == 0 {
    return Err("sub_chunks must be > 0".into());
  }

  eprintln!(
    "Reading {:?} ({} Hz, {} ch, {:?} {}-bit)",
    args.input, src_rate, channels, spec.sample_format, spec.bits_per_sample
  );

  // Read interleaved samples and convert to planar f32 (one Vec per channel).
  let mut input_by_channel: Vec<Vec<f32>> = vec![Vec::new(); channels];
  match spec.sample_format {
    SampleFormat::Float => match spec.bits_per_sample {
      32 => {
        for (i, s) in reader.samples::<f32>().enumerate() {
          let s = s?;
          input_by_channel[i % channels].push(s);
        }
      }
      bps => {
        return Err(
          format!("unsupported float WAV bits_per_sample: {bps} (expected 32)")
            .into(),
        );
      }
    },
    SampleFormat::Int => {
      if spec.bits_per_sample == 0 || spec.bits_per_sample > 32 {
        return Err(
          format!(
            "unsupported int WAV bits_per_sample: {} (expected 1..=32)",
            spec.bits_per_sample
          )
          .into(),
        );
      }
      let scale = (1i64 << (spec.bits_per_sample - 1)) as f32;
      for (i, s) in reader.samples::<i32>().enumerate() {
        let s = s?;
        input_by_channel[i % channels].push((s as f32) / scale);
      }
    }
  }

  let input_len = input_by_channel[0].len();
  if input_len == 0 {
    return Err("input WAV contained 0 frames".into());
  }
  if input_by_channel.iter().any(|ch| ch.len() != input_len) {
    return Err(
      "input WAV did not contain a consistent number of samples per channel"
        .into(),
    );
  }

  eprintln!(
    "Resampling {} frames/channel: {} Hz -> {} Hz",
    input_len, src_rate, args.rate
  );

  let src_rate_usize = usize::try_from(src_rate)?;
  let dst_rate_usize = usize::try_from(args.rate)?;

  let mut resampler = Fft::<f32>::new(
    src_rate_usize,
    dst_rate_usize,
    args.chunk_size,
    channels,
    args.sub_chunks,
    FixedSync::Both,
  )?;

  let input =
    SequentialSliceOfVecs::new(&input_by_channel, channels, input_len)?;

  let output_capacity = resampler.process_all_needed_output_len(input_len);
  let mut output_by_channel: Vec<Vec<f32>> =
    vec![vec![0.0; output_capacity]; channels];
  let mut output = SequentialSliceOfVecs::new_mut(
    &mut output_by_channel,
    channels,
    output_capacity,
  )?;

  let (_nbr_in, nbr_out) =
    resampler.process_all_into_buffer(&input, &mut output, input_len, None)?;

  eprintln!(
    "Writing {:?} ({} Hz, {} ch, {} frames/channel)",
    args.output, args.rate, channels, nbr_out
  );

  let out_spec = WavSpec {
    channels: spec.channels,
    sample_rate: args.rate,
    bits_per_sample: 32,
    sample_format: SampleFormat::Float,
  };
  let mut writer = WavWriter::create(&args.output, out_spec)?;

  for frame in 0..nbr_out {
    for ch in 0..channels {
      let mut s = output_by_channel[ch][frame];
      if args.clip {
        s = s.clamp(-1.0, 1.0);
      }
      writer.write_sample::<f32>(s)?;
    }
  }
  writer.finalize()?;

  Ok(())
}
