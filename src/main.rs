use std::{
  fs::File,
  io::BufWriter,
  sync::{Arc, Mutex},
};

use cpal::{
  traits::{DeviceTrait, HostTrait, StreamTrait},
  FromSample, Sample,
};

fn main() {
  let host = cpal::default_host();

  // Set up the input device and stream with the default input config.
  let device = host.default_input_device().unwrap();
  println!("Input device: {}", device.name().unwrap());

  let config = device.default_input_config().unwrap();
  println!("Default input config: {:?}", config);

  // The WAV file we're recording to.
  const PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/recorded.wav");
  let spec = wav_spec_from_config(&config);
  let writer = hound::WavWriter::create(PATH, spec).unwrap();
  let writer = Arc::new(Mutex::new(Some(writer)));

  // A flag to indicate that recording is in progress.
  println!("Begin recording...");

  // Run the input stream on a separate thread.
  let writer_2 = writer.clone();

  let err_fn = move |err| {
    eprintln!("an error occurred on stream: {}", err);
  };

  let stream = match config.sample_format() {
    cpal::SampleFormat::I8 => device
      .build_input_stream(
        &config.into(),
        move |data, _: &_| write_input_data::<i8, i8>(data, &writer_2),
        err_fn,
        None,
      )
      .unwrap(),
    cpal::SampleFormat::I16 => device
      .build_input_stream(
        &config.into(),
        move |data, _: &_| write_input_data::<i16, i16>(data, &writer_2),
        err_fn,
        None,
      )
      .unwrap(),
    cpal::SampleFormat::I32 => device
      .build_input_stream(
        &config.into(),
        move |data, _: &_| write_input_data::<i32, i32>(data, &writer_2),
        err_fn,
        None,
      )
      .unwrap(),
    cpal::SampleFormat::F32 => device
      .build_input_stream(
        &config.into(),
        move |data, _: &_| write_input_data::<f32, f32>(data, &writer_2),
        err_fn,
        None,
      )
      .unwrap(),
    sample_format => panic!("Unsupported sample format '{sample_format}'"),
  };

  stream.play().unwrap();

  // Let recording go for roughly three seconds.
  std::thread::sleep(std::time::Duration::from_secs(3));
  drop(stream);

  writer.lock().unwrap().take().unwrap().finalize().unwrap();
  println!("Recording {} complete!", PATH);
}

fn sample_format(format: cpal::SampleFormat) -> hound::SampleFormat {
  if format.is_float() {
    hound::SampleFormat::Float
  } else {
    hound::SampleFormat::Int
  }
}

fn wav_spec_from_config(
  config: &cpal::SupportedStreamConfig,
) -> hound::WavSpec {
  hound::WavSpec {
    channels: config.channels() as _,
    sample_rate: config.sample_rate().0 as _,
    bits_per_sample: (config.sample_format().sample_size() * 8) as _,
    sample_format: sample_format(config.sample_format()),
  }
}

type WavWriterHandle = Arc<Mutex<Option<hound::WavWriter<BufWriter<File>>>>>;

fn write_input_data<T, U>(input: &[T], writer: &WavWriterHandle)
where
  T: Sample,
  U: Sample + hound::Sample + FromSample<T>,
{
  if let Ok(mut guard) = writer.try_lock() {
    if let Some(writer) = guard.as_mut() {
      for &sample in input.iter() {
        let sample: U = U::from_sample(sample);
        writer.write_sample(sample).ok();
      }
    }
  }
}
