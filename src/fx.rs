use biquad::{
  Biquad, Coefficients, DirectForm1, Q_BUTTERWORTH_F32, ToHertz, Type,
};
use noise::{Fbm, NoiseFn, Simplex};

use crate::{TX_BUFFER_SIZE, TxBuffer};

#[derive(Debug, Clone)]
pub struct FxUnit {
  disabled: bool,

  noiser: Fbm<Simplex>,
  noise_idx: f64,

  lowpass: DirectForm1<f32>,
  highpass: DirectForm1<f32>,

  signal_gain: f32,
  distortion: f32,
}

impl FxUnit {
  pub fn new(disabled: bool, signal_gain: f32, distortion: f32) -> Self {
    let noise_idx = 0.0f64;
    let noiser: Fbm<noise::Simplex> = noise::Fbm::new(0);

    let fs = 44100.hz();

    let f0 = 8000.hz();
    let coeffs = Coefficients::<f32>::from_params(
      Type::LowPass,
      fs,
      f0,
      Q_BUTTERWORTH_F32,
    )
    .unwrap();
    let lowpass = DirectForm1::<f32>::new(coeffs);

    let f0 = 400.hz();
    let coeffs = Coefficients::<f32>::from_params(
      Type::HighPass,
      fs,
      f0,
      Q_BUTTERWORTH_F32,
    )
    .unwrap();
    let highpass = DirectForm1::<f32>::new(coeffs);

    Self {
      disabled,
      noiser,
      noise_idx,
      lowpass,
      highpass,
      signal_gain,
      distortion,
    }
  }

  pub fn run(&mut self, samples: &mut TxBuffer) {
    if !self.disabled {
      let mut noise = [0f32; TX_BUFFER_SIZE];
      for s in noise.iter_mut() {
        *s = self.noiser.get([self.noise_idx, self.noise_idx]) as f32;
        self.noise_idx += 0.005;
      }

      for (s, n) in samples.iter_mut().zip(noise.iter()) {
        *s =
          s.clamp(-self.distortion, self.distortion) * (0.4 / self.distortion);
        *s *= self.signal_gain;
        *s += n * 0.3;
        *s = s.clamp(-1.0, 1.0);
      }

      for s in samples.iter_mut() {
        *s = self.lowpass.run(*s);
        *s = self.highpass.run(*s);
      }
    } else {
      for s in samples.iter_mut() {
        *s *= self.signal_gain;
        *s = s.clamp(-1.0, 1.0);
      }
    }
  }

  pub fn squelch(&mut self) -> Vec<TxBuffer> {
    let length = 8;
    let mut chunks = Vec::with_capacity(length);
    if !self.disabled {
      for _ in 0..length {
        let mut noise_buf = [0f32; TX_BUFFER_SIZE];
        for sample in noise_buf.iter_mut() {
          *sample =
            self.noiser.get([self.noise_idx, self.noise_idx]) as f32 * 0.1;
          self.noise_idx += 0.03;
        }

        self.run(&mut noise_buf);
        chunks.push(noise_buf);
      }
    }

    chunks
  }
}
