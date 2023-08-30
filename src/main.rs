use cpal::Stream;
use nannou::prelude::*;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use spectrum_analyzer::scaling::divide_by_N_sqrt;
use spectrum_analyzer::windows::hann_window;
use spectrum_analyzer::{
  samples_fft_to_spectrum, FrequencyLimit, FrequencySpectrum,
};
use std::sync::mpsc::{self, Receiver};

struct Model {
  samples: Vec<f32>,
  spectrum: Option<FrequencySpectrum>,
  samples_receiver: Receiver<Vec<f32>>, // Receiver to get samples from the callback
  #[allow(dead_code)]
  stream: Stream,
}

fn main() {
  nannou::app(model).update(update).simple_window(view).run();
}

fn model(_: &App) -> Model {
  let host = cpal::default_host();

  // Set up the input device and stream with the default input config.
  let device = host.default_input_device().unwrap();
  println!("Input device: {}", device.name().unwrap());

  let config = device.default_input_config().unwrap();
  println!("Default input config: {:?}", config);

  let (tx, rx) = mpsc::channel();

  // A flag to indicate that recording is in progress.
  println!("Begin recording...");

  let err_fn = move |err| {
    eprintln!("an error occurred on stream: {}", err);
  };

  let stream = device
    .build_input_stream(
      &config.into(),
      move |data: &[f32], _: &_| {
        // Sending data to the main thread
        let data_vec = data.to_vec();
        tx.send(data_vec).expect("Failed to send samples");
      },
      err_fn,
      None,
    )
    .unwrap();

  stream.play().unwrap();

  // Set up your audio stream and provide the sender to the callback here...
  Model {
    samples: Vec::new(),
    spectrum: None,
    samples_receiver: rx,
    stream,
  }
}

fn update(_app: &App, model: &mut Model, _update: Update) {
  while let Ok(new_samples) = model.samples_receiver.try_recv() {
    model.samples.extend(new_samples);
  }

  if model.samples.len() > (44_100 / 60) {
    model.samples = model.samples.split_off(model.samples.len() - 2048);
  }

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
