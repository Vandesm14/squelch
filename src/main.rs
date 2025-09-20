use std::sync::mpsc::{self, Receiver, Sender};

use cpal::{
  traits::{DeviceTrait, HostTrait, StreamTrait},
  Stream,
};
use eframe::egui;

fn main() {
  let native_options = eframe::NativeOptions::default();
  eframe::run_native(
    "My egui App",
    native_options,
    Box::new(|cc| Ok(Box::new(MyEguiApp::new(cc)))),
  )
  .unwrap();
}

struct MyEguiApp {
  /// Samples from the input device
  samples: Vec<f32>,

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

impl MyEguiApp {
  fn new(cc: &eframe::CreationContext<'_>) -> Self {
    let err_fn = move |err| {
      eprintln!("an error occurred on stream: {}", err);
    };

    let (tx, rx) = mpsc::channel();
    let (out_tx, out_rx) = mpsc::channel::<f32>();

    let host = cpal::default_host();

    let device = host.default_input_device().unwrap();
    let config = device.default_input_config().unwrap();
    let spec = wav_spec_from_config(&config);
    println!("Sample rate: {}", spec.sample_rate);

    let stream = device
      .build_input_stream(
        &config.clone().into(),
        move |data: &[f32], _: &_| {
          let data_vec = data.iter().map(|s| s * 10.0).collect::<Vec<_>>();

          tx.send(data_vec).expect("Failed to send samples");
        },
        err_fn,
        None,
      )
      .unwrap();

    stream.play().unwrap();

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

    out_stream.play().unwrap();

    MyEguiApp {
      samples: Vec::new(),
      samples_receiver: rx,
      samples_sender: out_tx,
      stream,
      output_stream: out_stream,
    }
  }
}

impl eframe::App for MyEguiApp {
  fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
    egui::CentralPanel::default().show(ctx, |ui| {
      ui.heading("Hello World!");
    });

    while let Ok(new_samples) = self.samples_receiver.try_recv() {
      self.samples.extend(&new_samples);

      new_samples
        .iter()
        .for_each(|sample| self.samples_sender.send(*sample).unwrap());
    }
  }
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
