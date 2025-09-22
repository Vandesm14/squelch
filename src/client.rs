use std::{
  collections::VecDeque,
  net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket},
  str::FromStr,
  sync::mpsc::{self, Sender},
};

use bincode::config::{Configuration, standard};
use biquad::{
  Biquad, Coefficients, DirectForm1, Q_BUTTERWORTH_F32, ToHertz, Type,
};
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

  /// Threshold of distortion effect.
  #[arg(short, long, default_value_t = 0.005)]
  pub distortion: f32,

  /// Gain multiplier for incoming signal.
  #[arg(short, long, default_value_t = 1.0)]
  pub gain: f32,

  /// Gain multiplier for mic signal.
  #[arg(short, long, default_value_t = 1.0)]
  pub mic_gain: f32,
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

  let spk_config = cpal::SupportedStreamConfig::new(
    1,
    cpal::SampleRate(44100),
    cpal::SupportedBufferSize::Range { min: 1, max: 8192 },
    cpal::SampleFormat::F32,
  );

  let mic_config = cpal::SupportedStreamConfig::new(
    1,
    cpal::SampleRate(44100),
    cpal::SupportedBufferSize::Range { min: 1, max: 8192 },
    cpal::SampleFormat::F32,
  );

  let mic_device = host.default_input_device().unwrap();
  println!("mic config: {mic_config:?}");

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
  println!("spk config: {spk_config:?}");
  let mut buf = VecDeque::with_capacity(TX_BUFFER_SIZE);
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
    let noiser: Fbm<noise::Simplex> = noise::Fbm::new(0);

    let f0 = 8000.hz();
    let fs = 44100.hz();
    let coeffs = Coefficients::<f32>::from_params(
      Type::LowPass,
      fs,
      f0,
      Q_BUTTERWORTH_F32,
    )
    .unwrap();
    let mut lowpass = DirectForm1::<f32>::new(coeffs);

    let f0 = 400.hz();
    let coeffs = Coefficients::<f32>::from_params(
      Type::HighPass,
      fs,
      f0,
      Q_BUTTERWORTH_F32,
    )
    .unwrap();
    let mut highpass = DirectForm1::<f32>::new(coeffs);

    let mut mic_buf: Vec<f32> = Vec::with_capacity(TX_BUFFER_SIZE);

    let mut ptt = false;
    loop {
      if let Ok(new_ptt) = ptt_rx.try_recv() {
        ptt = new_ptt;
      }

      if ptt {
        match mic_rx.try_recv() {
          Ok(new_samples) => {
            mic_buf.extend(new_samples);

            let mut count = 0;
            for chunk in mic_buf.chunks_exact(TX_BUFFER_SIZE) {
              let mut buf = [0f32; TX_BUFFER_SIZE];
              buf.copy_from_slice(chunk);

              for s in buf.iter_mut() {
                *s *= args.mic_gain;
                *s = s.clamp(-1.0, 1.0);
              }

              map_would_block(
                socket.send_to(
                  &bincode::encode_to_vec(Packet::Audio(buf), standard())
                    .unwrap(),
                  address,
                ),
              )
              .unwrap();

              count += 1;
            }
            mic_buf.drain(0..count * TX_BUFFER_SIZE);
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

                for (s, n) in samples.iter_mut().zip(noise.iter()) {
                  *s = s.clamp(-args.distortion, args.distortion)
                    * (0.4 / args.distortion);
                  *s *= args.gain;
                  *s += n * 0.3;
                  *s = s.clamp(-1.0, 1.0);
                }

                for s in samples.iter_mut() {
                  *s = lowpass.run(*s);
                  *s = highpass.run(*s);
                }
              } else {
                for s in samples.iter_mut() {
                  *s *= args.gain;
                  *s = s.clamp(-1.0, 1.0);
                }
              }

              spk_tx.send(samples).unwrap();
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
