//! Bounded I/O bridge. The renderer never receives ambient filesystem access.
use alloc::{format, string::String, vec::Vec};
pub type Error = String;
pub type Result<T> = core::result::Result<T, Error>;
pub trait Read {
    fn read(&mut self, out: &mut [u8]) -> Result<usize>;
}
pub trait Write {
    fn write_all(&mut self, bytes: &[u8]) -> Result<()>;
    fn flush(&mut self) -> Result<()> {
        Ok(())
    }
    fn write_fmt(&mut self, args: core::fmt::Arguments<'_>) -> Result<()> {
        struct Adapter<'a, T: Write + ?Sized>(&'a mut T, Option<String>);
        impl<T: Write + ?Sized> core::fmt::Write for Adapter<'_, T> {
            fn write_str(&mut self, s: &str) -> core::fmt::Result {
                self.0.write_all(s.as_bytes()).map_err(|e| {
                    self.1 = Some(e);
                    core::fmt::Error
                })
            }
        }
        let mut a = Adapter(self, None);
        core::fmt::write(&mut a, args).map_err(|_| a.1.unwrap_or_else(|| format!("format failed")))
    }
}
pub struct BufReader<R> {
    inner: R,
    bytes: Vec<u8>,
    pos: usize,
}
impl<R: Read> BufReader<R> {
    pub fn new(inner: R) -> Self {
        Self {
            inner,
            bytes: alloc::vec![0;8192],
            pos: 8192,
        }
    }
}
impl<R: Read> Read for BufReader<R> {
    fn read(&mut self, out: &mut [u8]) -> Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        if self.pos == self.bytes.len() {
            self.bytes.resize(8192, 0);
            let n = self.inner.read(&mut self.bytes)?;
            self.bytes.truncate(n);
            self.pos = 0;
            if n == 0 {
                return Ok(0);
            }
        }
        let n = out.len().min(self.bytes.len() - self.pos);
        out[..n].copy_from_slice(&self.bytes[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}
