pub struct JitterBuffer<T> {
  buffer: Vec<T>,
  capacity: usize,
}

impl<T> JitterBuffer<T> {
  pub fn new(capacity: usize) -> Self {
    Self {
      buffer: Vec::with_capacity(capacity),
      capacity,
    }
  }

  pub fn push_and_drain(&mut self, value: T) -> Option<Vec<T>> {
    if self.buffer.len() >= self.capacity {
      let items: Vec<_> = self.buffer.drain(..).collect();
      self.buffer.push(value);
      Some(items)
    } else {
      self.buffer.push(value);
      None
    }
  }
}
