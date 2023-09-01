use cpal::Stream;
use nannou::prelude::*;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use spectrum_analyzer::scaling::divide_by_N_sqrt;
use spectrum_analyzer::windows::hann_window;
use spectrum_analyzer::{
  samples_fft_to_spectrum, FrequencyLimit, FrequencySpectrum,
};
use std::sync::mpsc::{self, Receiver, Sender};

struct Model {
  /// Samples from the input device
  samples: Vec<f32>,

  /// Spectrum of the samples
  spectrum: Option<FrequencySpectrum>,

  /// Receiver to get samples from the input device
  samples_receiver: Receiver<Vec<f32>>,

  /// Sender to send samples to the output device
  samples_sender: Sender<f32>,

  /// Persist the stream so it doesn't get dropped
  #[allow(dead_code)]
  stream: Stream,

  /// Persist the output stream so it doesn't get dropped
  #[allow(dead_code)]
  output_stream: Stream,
}

fn main() {
  // Create the window.
  nannou::app(model).update(update).simple_window(view).run();
}

fn model(_: &App) -> Model {
  // Create a channel to get samples from the input device
  let (tx, rx) = mpsc::channel();

  // Create a channel to send samples to the output device
  let (out_tx, out_rx) = mpsc::channel::<f32>();

  let err_fn = move |err| {
    eprintln!("an error occurred on stream: {}", err);
  };

  let host = cpal::default_host();

  // Create the input stream
  let device = host.default_input_device().unwrap();
  let config = device.default_input_config().unwrap();
  let stream = device
    .build_input_stream(
      &config.clone().into(),
      move |data: &[f32], _: &_| {
        let data_vec = data.to_vec();
        tx.send(data_vec).expect("Failed to send samples");
      },
      err_fn,
      None,
    )
    .unwrap();

  // Start the stream
  stream.play().unwrap();

  // Create the output stream
  let out_device = host.default_output_device().unwrap();
  let out_config = out_device.default_output_config().unwrap();
  let out_stream = out_device
    .build_output_stream(
      &out_config.into(),
      move |data: &mut [f32], _: &_| {
        out_rx.try_iter().take(data.len()).enumerate().for_each(
          |(i, sample)| {
            data[i] = sample;
          },
        );
      },
      err_fn,
      None,
    )
    .unwrap();

  // Start the output stream
  out_stream.play().unwrap();

  // Create the spec from the input stream
  let spec = wav_spec_from_config(&config);
  println!("Sample rate: {}", spec.sample_rate);

  // Create the model
  Model {
    samples: Vec::new(),
    spectrum: None,
    samples_receiver: rx,
    samples_sender: out_tx,
    stream,
    output_stream: out_stream,
  }
}

fn update(_app: &App, model: &mut Model, _update: Update) {
  // If there are new samples, add them to the model
  while let Ok(new_samples) = model.samples_receiver.try_recv() {
    // Add the new samples to our model
    model.samples.extend(&new_samples);

    // Send the new samples to the output device
    new_samples
      .iter()
      .for_each(|sample| model.samples_sender.send(*sample).unwrap());
  }

  // Limit the number of samples to 1 second (at 44.1 kHz)
  if model.samples.len() > (44_100 / 60) {
    model.samples = model.samples.split_off(model.samples.len() - 2048);
  }

  // If there are samples, calculate the spectrum
  if !model.samples.is_empty() {
    let spectrum = apply_fft(&model.samples);
    model.spectrum = Some(spectrum);
  }
}

fn view(app: &App, model: &Model, frame: Frame) {
  let draw = app.draw();
  draw.background().color(BLACK);

  let width = app.window_rect().w();
  let height = app.window_rect().h();

  if let Some(spectrum) = &model.spectrum {
    let data = spectrum.data();
    let points = data.iter().enumerate().map(|(i, (_, ampl))| {
      (
        ((i as f32).log10() * data.len() as f32 - (width / 2.0))
          .clamp(-width / 2.0, width / 2.0),
        ampl.val() * ((height * 2.0) / (u8::MAX as f32).sqrt())
          - (height / 2.0),
      )
    });

    draw.path().stroke().color(WHITE).points(points).finish();
  }

  draw.to_frame(app, &frame).unwrap();
}

/// Apply the FFT to the samples
fn apply_fft(samples: &[f32]) -> FrequencySpectrum {
  // apply hann window for smoothing; length must be a power of 2 for the FFT
  // 2048 is a good starting point with 44100 kHz
  let window = hann_window(&samples[0..2048]);
  // calc spectrum
  samples_fft_to_spectrum(
    // (windowed) samples
    &window,
    // sampling rate
    44100,
    // optional frequency limit: e.g. only interested in frequencies 50 <= f <= 150?
    FrequencyLimit::Range(0.0, 10_000.0),
    // optional scale
    Some(&divide_by_N_sqrt),
  )
  .unwrap()
}

/// Convert a cpal sample format to a hound sample format
fn sample_format(format: cpal::SampleFormat) -> hound::SampleFormat {
  if format.is_float() {
    hound::SampleFormat::Float
  } else {
    hound::SampleFormat::Int
  }
}

/// Generate a hound spec from a cpal config
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
