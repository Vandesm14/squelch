use std::{
  collections::VecDeque,
  net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket},
  str::FromStr,
  sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
    mpsc::{self},
  },
  time::Instant,
};

use clap::Parser;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use eframe::egui::{self, Button, Sense};
use global_hotkey::{
  GlobalHotKeyEvent, GlobalHotKeyManager,
  hotkey::{Code, HotKey},
};

use squelch::{
  MAX_PACKET_SIZE, Packet, TX_BUFFER_SIZE, TxBuffer, WAIT_DURATION, fx::FxUnit,
  map_would_block,
};

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
  #[arg(short, long, default_value_t = 0.05)]
  pub distortion: f32,

  /// Gain multiplier for incoming signal.
  #[arg(short, long, default_value_t = 1.0)]
  pub gain: f32,

  /// Gain multiplier for mic signal.
  #[arg(short, long, default_value_t = 1.0)]
  pub mic_gain: f32,

  /// Playback jitter-buffer depth in milliseconds. Audio is pre-buffered
  /// to roughly this much before playback (re)starts, to absorb network and
  /// scheduling jitter. Higher = fewer pops but more latency.
  #[arg(long, default_value_t = 20)]
  pub jitter_ms: u64,

  /// Audio device period size in frames (0 = backend default). The default
  /// backend period on this machine is large (~32 ms), which sets a latency
  /// floor. Request a small fixed period (e.g. 441 ≈ 10 ms) to enable
  /// low-latency playback like Mumble's low-delay mode. Requires the audio
  /// backend to honor small periods.
  #[arg(long, default_value_t = 0)]
  pub frames: u32,
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
  let (spk_tx, spk_rx) = mpsc::channel::<TxBuffer>();
  let ptt = Arc::new(AtomicBool::new(false));

  let host = cpal::default_host();
  let mic_device = host.default_input_device().unwrap();
  let spk_device = host.default_output_device().unwrap();

  // Request an explicit (optionally small) device period. The backend
  // default period on this machine is ~32 ms, which caps how low playback
  // latency can go; a small fixed period lets us run closer to Mumble.
  //
  // Not every backend honors an arbitrary fixed period (e.g. ALSA via
  // PipeWire rejects many sizes with EINVAL), so probe the requested size
  // on both devices and fall back to the backend default if it's rejected
  // rather than panicking.
  let buffer_size = resolve_buffer_size(&mic_device, &spk_device, args.frames);

  let spk_config = cpal::StreamConfig {
    channels: 1,
    sample_rate: cpal::SampleRate(44100),
    buffer_size,
  };

  let mic_config = cpal::StreamConfig {
    channels: 1,
    sample_rate: cpal::SampleRate(44100),
    buffer_size,
  };

  println!("mic config: {mic_config:?}");

  let ptt_ref = ptt.clone();
  let mic_stream = mic_device
    .build_input_stream(
      &mic_config,
      move |data: &[f32], _: &_| {
        if ptt_ref.load(Ordering::SeqCst) {
          mic_tx.send(data.to_vec()).unwrap();
        }
      },
      err_fn,
      None,
    )
    .unwrap();
  mic_stream.play().unwrap();

  println!("spk config: {spk_config:?}");

  // Diagnostics: count how often the speaker callback runs short of data.
  // `underruns`       -> callback had an empty queue (whole block silenced).
  // `partial_fills`   -> callback had some, but not enough, samples.
  // `missing_samples` -> total samples we had to zero-fill across all blocks.
  let underruns = Arc::new(AtomicU64::new(0));
  let partial_fills = Arc::new(AtomicU64::new(0));
  let missing_samples = Arc::new(AtomicU64::new(0));
  let callbacks = Arc::new(AtomicU64::new(0));
  let queue_len = Arc::new(AtomicU64::new(0));

  let (underruns_cb, partial_cb, missing_cb, callbacks_cb, queue_cb) = (
    underruns.clone(),
    partial_fills.clone(),
    missing_samples.clone(),
    callbacks.clone(),
    queue_len.clone(),
  );

  // Jitter buffer: pre-buffer ~jitter_ms of audio before (re)starting
  // playback so the consumer block (which is much larger than a single
  // network chunk) never skates on an empty queue.
  let target_samples = (args.jitter_ms as usize * 44100) / 1000;
  // Bound added latency if the sender clock runs slightly fast (drift).
  let max_samples = target_samples * 4;

  let mut buf = VecDeque::with_capacity(target_samples.max(TX_BUFFER_SIZE));
  // Start in the "refilling" state so we wait for a healthy backlog.
  let mut filling = true;
  let spk_stream = spk_device
    .build_output_stream(
      &spk_config,
      move |data: &mut [f32], _: &_| {
        spk_rx.try_iter().for_each(|samples| {
          buf.extend(samples);
        });

        callbacks_cb.fetch_add(1, Ordering::Relaxed);

        // Drop oldest samples if drift made the backlog grow unbounded.
        if buf.len() > max_samples {
          let drop = buf.len() - target_samples;
          buf.drain(0..drop);
        }

        // While (re)filling, emit silence until the backlog is healthy.
        // This is what stops the per-block zero-fills (faint pops): we
        // wait for a cushion instead of dribbling out partial blocks.
        if filling {
          if buf.len() >= target_samples {
            filling = false;
          } else {
            for item in data.iter_mut() {
              *item = 0.0;
            }
            queue_cb.store(buf.len() as u64, Ordering::Relaxed);
            return;
          }
        }

        let take = data.len().min(buf.len());

        buf
          .iter()
          .enumerate()
          .take(take)
          .for_each(|(i, s)| data[i] = *s);
        buf.drain(0..take);

        // Couldn't fully satisfy the block: zero the tail, record it, and
        // drop back into refilling so we rebuild a cushion before resuming
        // rather than emitting a string of partially-filled blocks.
        if take < data.len() {
          if take == 0 {
            underruns_cb.fetch_add(1, Ordering::Relaxed);
          } else {
            partial_cb.fetch_add(1, Ordering::Relaxed);
          }
          missing_cb.fetch_add((data.len() - take) as u64, Ordering::Relaxed);
          for item in data[take..].iter_mut() {
            *item = 0.0;
          }
          filling = true;
        }

        queue_cb.store(buf.len() as u64, Ordering::Relaxed);
      },
      err_fn,
      None,
    )
    .unwrap();
  spk_stream.play().unwrap();

  let ptt_ref = ptt.clone();
  std::thread::spawn(move || {
    let mut buf = [0; MAX_PACKET_SIZE];
    let mut fx_unit = FxUnit::new(args.no_fx, args.gain, args.distortion);

    let socket = UdpSocket::bind("0.0.0.0:0").unwrap();
    socket.set_nonblocking(true).unwrap();
    map_would_block(
      socket.send_to(&postcard::to_allocvec(&Packet::Ping).unwrap(), address),
    )
    .unwrap();

    let mut last_ptt = false;
    let mut do_squelch = false;
    let mut last_packet = Instant::now();
    let mut mic_buf: Vec<f32> = Vec::with_capacity(TX_BUFFER_SIZE);
    loop {
      // If PTT was just released, send white noise.
      let new_ptt = ptt_ref.load(Ordering::SeqCst);
      if !new_ptt && last_ptt {
        for chunk in fx_unit.squelch() {
          spk_tx.send(chunk).unwrap();
        }
      }
      last_ptt = new_ptt;

      if ptt_ref.load(Ordering::SeqCst) {
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

              map_would_block(socket.send_to(
                &postcard::to_allocvec(&Packet::Audio(buf)).unwrap(),
                address,
              ))
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
        match postcard::from_bytes::<Packet>(&buf) {
          Ok(packet) => match packet {
            Packet::Ping => todo!(),
            Packet::Audio(mut samples) => {
              last_packet = Instant::now();
              do_squelch = true;

              fx_unit.run(&mut samples);
              spk_tx.send(samples).unwrap();
            }
          },
          Err(err) => {
            eprintln!("Failed to decode packet: {err:?}")
          }
        }
      } else if do_squelch
        && last_packet.elapsed() >= WAIT_DURATION.mul_f32(7.0)
      {
        do_squelch = false;

        for chunk in fx_unit.squelch() {
          spk_tx.send(chunk).unwrap();
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

    loop {
      if let Ok(event) = GlobalHotKeyEvent::receiver().try_recv()
        && event.id == code as u32
      {
        match event.state {
          global_hotkey::HotKeyState::Pressed => {
            ptt.store(true, Ordering::SeqCst);
          }
          global_hotkey::HotKeyState::Released => {
            ptt.store(false, Ordering::SeqCst);
          }
        }
      }
    }
  }

  let ptt_ref = ptt.clone();
  let native_options = eframe::NativeOptions::default();
  eframe::run_native(
    "Squelch",
    native_options,
    Box::new(|cc| Ok(Box::new(MyEguiApp::new(cc, ptt_ref)))),
  )
  .unwrap();
}

/// Probe whether the requested fixed device period (`frames`) is accepted by
/// both the input and output devices. Returns `Fixed(frames)` only if both
/// accept it; otherwise warns and returns `Default` so we never panic on a
/// backend that rejects the size (e.g. ALSA via PipeWire returning EINVAL).
fn resolve_buffer_size(
  mic_device: &cpal::Device,
  spk_device: &cpal::Device,
  frames: u32,
) -> cpal::BufferSize {
  if frames == 0 {
    return cpal::BufferSize::Default;
  }

  let cfg = cpal::StreamConfig {
    channels: 1,
    sample_rate: cpal::SampleRate(44100),
    buffer_size: cpal::BufferSize::Fixed(frames),
  };
  let noop_err = |_err| {};

  let out_probe = spk_device
    .build_output_stream(&cfg, |_: &mut [f32], _: &_| {}, noop_err, None)
    .map(drop);
  let in_probe = mic_device
    .build_input_stream(&cfg, |_: &[f32], _: &_| {}, noop_err, None)
    .map(drop);

  if out_probe.is_ok() && in_probe.is_ok() {
    println!("Using fixed device period of {frames} frames.");
    return cpal::BufferSize::Fixed(frames);
  }

  if let Err(e) = out_probe {
    eprintln!(
      "warning: output device rejected --frames {frames} ({e}); \
       falling back to backend default period"
    );
  }
  if let Err(e) = in_probe {
    eprintln!(
      "warning: input device rejected --frames {frames} ({e}); \
       falling back to backend default period"
    );
  }
  cpal::BufferSize::Default
}

struct MyEguiApp {
  ptt: Arc<AtomicBool>,
}

impl MyEguiApp {
  fn new(_: &eframe::CreationContext<'_>, ptt: Arc<AtomicBool>) -> Self {
    MyEguiApp { ptt }
  }
}

impl eframe::App for MyEguiApp {
  fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
    egui::CentralPanel::default().show(ctx, |ui| {
      ui.heading("Hello World!");
      ui.label(format!("PTT: {}", self.ptt.load(Ordering::SeqCst)));

      let response = ui.add(Button::new("PTT").sense(Sense::drag()));
      if response.drag_started() {
        self.ptt.store(true, Ordering::SeqCst);
      } else if response.drag_stopped() {
        self.ptt.store(false, Ordering::SeqCst);
      }
    });
  }
}
