use std::{
  net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket},
  str::FromStr,
  sync::mpsc::{self, Sender},
};

use bincode::config::{Configuration, standard};
use clap::Parser;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use eframe::egui::{self, Button, Sense};
use global_hotkey::{
  GlobalHotKeyEvent, GlobalHotKeyManager,
  hotkey::{Code, HotKey},
};
use noise::{Fbm, NoiseFn};

use squelch::{MAX_PACKET_SIZE, Packet, TX_BUFFER_SIZE};

/// Squelch
#[derive(Debug, Clone, Parser)]
#[command(version, about, long_about = None)]
pub struct Cli {
  /// The socket IPv4 address to bind the WebSocket server to.
  #[arg(short, long, default_value = None)]
  pub address: Option<SocketAddr>,

  /// Registers a PTT key via key string (see https://docs.rs/global-hotkey/latest/global_hotkey/hotkey/enum.Code.html).
  #[arg(long)]
  pub hotkey: Option<String>,

  /// Disables effects.
  #[arg(long)]
  pub no_fx: bool,
}

pub fn map_would_block<T>(result: std::io::Result<T>) -> std::io::Result<()> {
  match result {
    Ok(_) => std::io::Result::Ok(()),
    Err(e) => match e.kind() {
      std::io::ErrorKind::WouldBlock => Ok(()),
      _ => Err(e),
    },
  }
}

fn main() {
  let args = Cli::parse();

  let address = args.address.unwrap_or_else(|| {
    SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(0, 0, 0, 0), 1837))
  });

  let err_fn = move |err| {
    eprintln!("an error occurred on stream: {}", err);
  };

  let (mic_tx, mic_rx) = mpsc::channel::<Vec<f32>>();
  let (spk_tx, spk_rx) = mpsc::channel::<[f32; TX_BUFFER_SIZE]>();
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
          println!("buf: {}", buf.len());
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

  let thread_args = args.clone();
  std::thread::spawn(move || {
    let mut buf = [0; MAX_PACKET_SIZE];
    let socket = UdpSocket::bind("0.0.0.0:0").unwrap();
    socket.set_nonblocking(true).unwrap();
    map_would_block(socket.send_to(
      &bincode::encode_to_vec(Packet::Ping, standard()).unwrap(),
      address,
    ))
    .unwrap();

    let mut noise_idx = 0.0f64;
    let noiser: Fbm<noise::Worley> = noise::Fbm::new(0);

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

              map_would_block(
                socket.send_to(
                  &bincode::encode_to_vec(Packet::Audio(buf), standard())
                    .unwrap(),
                  address,
                ),
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
            Packet::Audio(mut samples) => {
              if !thread_args.no_fx {
                let mut noise = [0f32; TX_BUFFER_SIZE];
                for s in noise.iter_mut() {
                  *s = noiser.get([noise_idx, noise_idx]) as f32;
                  noise_idx += 0.005;
                }
                let atten = 0.01;
                for (s, n) in samples.iter_mut().zip(noise.iter()) {
                  *s *= 4.0;
                  *s = s.clamp(-atten, atten) * (0.4 / atten);
                  *s += n * 0.35;
                  *s = s.clamp(-1.0, 1.0);
                }
                lowpass_filter::lowpass_filter(&mut samples, 44100.0, 700.0);
              } else {
                for s in samples.iter_mut() {
                  *s *= 4.0;
                  *s = s.clamp(-1.0, 1.0);
                }
              }

              // if let Some(chunks) = jitter_buffer.push_and_drain(samples) {
              // for chunk in chunks {
              spk_tx.send(samples).unwrap();
              // }
              // }
            }
          },
          Err(err) => {
            eprintln!("Failed to decode packet: {err:?}")
          }
        }
      }
    }
  });

  if let Some(key) = args.hotkey {
    println!("Using hotkey.");

    let code = Code::from_str(&key).unwrap();

    let manager = GlobalHotKeyManager::new().unwrap();
    let hotkey = HotKey::new(None, code);
    manager.register(hotkey).unwrap();

    let hotkey_ptt_tx = ptt_tx.clone();
    loop {
      if let Ok(event) = GlobalHotKeyEvent::receiver().try_recv()
        && event.id == code as u32
      {
        match event.state {
          global_hotkey::HotKeyState::Pressed => {
            hotkey_ptt_tx.send(true).unwrap()
          }
          global_hotkey::HotKeyState::Released => {
            hotkey_ptt_tx.send(false).unwrap()
          }
        }
      }
    }
  }

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
