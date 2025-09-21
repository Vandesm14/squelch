use std::{
  net::UdpSocket,
  sync::mpsc::{self, Sender},
};

use bincode::config::{Configuration, standard};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use eframe::egui::{self, Button, Sense};
use squelch::{MAX_PACKET_SIZE, Packet, TX_BUFFER_SIZE};
use turborand::{TurboRand, rng::Rng};

fn main() {
  let err_fn = move |err| {
    eprintln!("an error occurred on stream: {}", err);
  };

  let (mic_tx, mic_rx) = mpsc::channel::<Vec<f32>>();
  let (spk_tx, spk_rx) = mpsc::channel::<Vec<f32>>();
  let (ptt_tx, ptt_rx) = mpsc::channel::<bool>();

  let host = cpal::default_host();

  let mic_device = host.default_input_device().unwrap();
  let mic_config = mic_device.default_input_config().unwrap();
  println!("Sample rate: {}", mic_config.sample_rate().0);

  let mic_stream = mic_device
    .build_input_stream(
      &mic_config.clone().into(),
      move |data: &[f32], _: &_| {
        mic_tx.send(data.to_vec()).unwrap();
      },
      err_fn,
      None,
    )
    .unwrap();

  mic_stream.play().unwrap();

  let spk_device = host.default_output_device().unwrap();
  let spk_config = spk_device.default_output_config().unwrap();
  let mut buf = Vec::with_capacity(TX_BUFFER_SIZE);
  let spk_stream = spk_device
    .build_output_stream(
      &spk_config.into(),
      move |data: &mut [f32], _: &_| {
        spk_rx.try_iter().for_each(|samples| {
          buf.extend(samples);
        });
        if !buf.is_empty() {
          let take = data.len().min(buf.len());
          buf
            .iter()
            .enumerate()
            .take(take)
            .for_each(|(i, s)| data[i] = *s);
          buf.drain(0..take);
        } else {
          for item in data.iter_mut() {
            *item = 0.0;
          }
        }
      },
      err_fn,
      None,
    )
    .unwrap();

  spk_stream.play().unwrap();

  std::thread::spawn(move || {
    let mut buf = [0; MAX_PACKET_SIZE];
    let socket = UdpSocket::bind("0.0.0.0:0").unwrap();
    socket.set_nonblocking(true).unwrap();
    socket
      .send_to(
        &bincode::encode_to_vec(Packet::Ping, standard()).unwrap(),
        "0.0.0.0:1837",
      )
      .unwrap();

    let mut rng = Rng::new();

    let mut ptt = false;
    loop {
      if let Ok(new_ptt) = ptt_rx.try_recv() {
        ptt = new_ptt;
      }

      if ptt {
        match mic_rx.try_recv() {
          Ok(new_samples) => {
            for chunk in new_samples.chunks_exact(TX_BUFFER_SIZE) {
              let mut buf = [0f32; TX_BUFFER_SIZE];
              buf.copy_from_slice(chunk);

              socket
                .send_to(
                  &bincode::encode_to_vec(Packet::Audio(buf), standard())
                    .unwrap(),
                  "0.0.0.0:1837",
                )
                .unwrap();
            }
          }
          Err(err) => match err {
            mpsc::TryRecvError::Empty => {}
            mpsc::TryRecvError::Disconnected => {
              panic!("Speaker sender disconnected, exiting thread.")
            }
          },
        }
      } else if socket.recv_from(&mut buf).is_ok() {
        match bincode::decode_from_slice::<Packet, Configuration>(
          &buf,
          bincode::config::standard(),
        ) {
          Ok((packet, _)) => match packet {
            Packet::Ping => todo!(),
            Packet::Pong => todo!(),
            Packet::Audio(mut samples) => {
              let cutoff: f32 = 400.0;
              let mut noise = [0f32; TX_BUFFER_SIZE];
              for s in noise.iter_mut() {
                *s = rng.f32() * 0.06;
              }
              // lowpass_filter::lowpass_filter(&mut noise, 44100.0, cutoff);
              let atten = 0.001;
              for (s, n) in samples.iter_mut().zip(noise.iter()) {
                *s *= 2.0;
                *s = s.clamp(-atten, atten) * (0.1 / atten);
                *s += n;
                *s = s.clamp(-1.0, 1.0);
              }
              lowpass_filter::lowpass_filter(&mut samples, 44100.0, cutoff);
              spk_tx.send(samples.to_vec()).unwrap();
            }
          },
          Err(err) => {
            eprintln!("Failed to decode packet: {err:?}")
          }
        }
      }
    }
  });

  let native_options = eframe::NativeOptions::default();
  eframe::run_native(
    "Squelch",
    native_options,
    Box::new(|cc| Ok(Box::new(MyEguiApp::new(cc, ptt_tx)))),
  )
  .unwrap();
}

struct MyEguiApp {
  ptt_sender: Sender<bool>,
  ptt: bool,
}

impl MyEguiApp {
  fn new(_: &eframe::CreationContext<'_>, ptt_sender: Sender<bool>) -> Self {
    MyEguiApp {
      ptt_sender,
      ptt: false,
    }
  }
}

impl eframe::App for MyEguiApp {
  fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
    egui::CentralPanel::default().show(ctx, |ui| {
      ui.heading("Hello World!");
      ui.label(format!("PTT: {}", self.ptt));

      let response = ui.add(Button::new("PTT").sense(Sense::drag()));
      if response.drag_started() {
        self.ptt = true;
        self.ptt_sender.send(self.ptt).unwrap();
      } else if response.drag_stopped() {
        self.ptt = false;
        self.ptt_sender.send(self.ptt).unwrap();
      }
    });
  }
}
