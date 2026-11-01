#[derive(Default)]
pub struct Writer {
    bytes: Vec<u8>,
}

impl Writer {
    pub fn new() -> Writer {
        Writer::default()
    }

    pub fn finish(self) -> Vec<u8> {
        self.bytes
    }

    pub fn varint(&mut self, mut n: u64) -> &mut Writer {
        while n >= 0x80 {
            self.bytes.push((n as u8 & 0x7f) | 0x80);
            n >>= 7;
        }
        self.bytes.push(n as u8);
        self
    }

    pub fn int(&mut self, n: i64) -> &mut Writer {
        self.varint(((n << 1) ^ (n >> 63)) as u64)
    }

    pub fn count(&mut self, n: u64) -> &mut Writer {
        self.int(i64::try_from(n).unwrap_or(i64::MAX))
    }

    pub fn tag(&mut self, t: u64) -> &mut Writer {
        self.varint(t)
    }

    pub fn bool(&mut self, b: bool) -> &mut Writer {
        self.bytes.push(u8::from(b));
        self
    }

    pub fn bytes(&mut self, b: &[u8]) -> &mut Writer {
        self.varint(b.len() as u64);
        self.bytes.extend_from_slice(b);
        self
    }

    pub fn str(&mut self, s: &str) -> &mut Writer {
        self.bytes(s.as_bytes())
    }

    pub fn written(&mut self, b: &[u8]) -> &mut Writer {
        self.bytes.extend_from_slice(b);
        self
    }

    pub fn option<T>(&mut self, v: Option<T>, write: impl FnOnce(&mut Writer, T)) -> &mut Writer {
        match v {
            None => {
                self.tag(0);
            }
            Some(v) => {
                self.tag(1);
                write(self, v);
            }
        }
        self
    }

    pub fn list<T>(
        &mut self,
        items: impl ExactSizeIterator<Item = T>,
        mut write: impl FnMut(&mut Writer, T),
    ) -> &mut Writer {
        self.varint(items.len() as u64);
        for item in items {
            write(self, item);
        }
        self
    }
}

pub struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    pub fn new(bytes: &'a [u8]) -> Reader<'a> {
        Reader { bytes, at: 0 }
    }

    pub fn end(&self) -> Result<(), String> {
        if self.at == self.bytes.len() {
            Ok(())
        } else {
            Err(format!("bytes follow the value at {}", self.at))
        }
    }

    pub fn varint(&mut self) -> Result<u64, String> {
        let mut n = 0u64;
        for shift in (0..64).step_by(7) {
            let b = *self
                .bytes
                .get(self.at)
                .ok_or_else(|| format!("the bytes end inside a varint at {}", self.at))?;
            self.at += 1;
            n |= u64::from(b & 0x7f) << shift;
            if b < 0x80 {
                return Ok(n);
            }
        }
        Err(format!("a varint wider than 64 bits at {}", self.at))
    }

    pub fn int(&mut self) -> Result<i64, String> {
        let z = self.varint()?;
        Ok(((z >> 1) as i64) ^ -((z & 1) as i64))
    }

    pub fn count(&mut self) -> Result<u64, String> {
        let at = self.at;
        u64::try_from(self.int()?).map_err(|_| format!("a negative count at {at}"))
    }

    pub fn tag(&mut self, count: u64) -> Result<u64, String> {
        let at = self.at;
        let t = self.varint()?;
        if t < count {
            Ok(t)
        } else {
            Err(format!("tag {t} of {count} at {at}"))
        }
    }

    pub fn bool(&mut self) -> Result<bool, String> {
        match self.bytes.get(self.at) {
            Some(b @ (0 | 1)) => {
                self.at += 1;
                Ok(*b == 1)
            }
            _ => Err(format!("no boolean at {}", self.at)),
        }
    }

    fn length(&mut self) -> Result<usize, String> {
        let at = self.at;
        let n = self.varint()?;
        usize::try_from(n)
            .ok()
            .filter(|n| *n <= self.bytes.len() - self.at)
            .ok_or_else(|| format!("a length of {n} past the bytes left at {at}"))
    }

    pub fn bytes(&mut self) -> Result<&'a [u8], String> {
        let n = self.length()?;
        let read = &self.bytes[self.at..self.at + n];
        self.at += n;
        Ok(read)
    }

    pub fn str(&mut self) -> Result<String, String> {
        let at = self.at;
        String::from_utf8(self.bytes()?.to_vec())
            .map_err(|_| format!("text that is not UTF-8 at {at}"))
    }

    pub fn option<T>(
        &mut self,
        read: impl FnOnce(&mut Reader<'a>) -> Result<T, String>,
    ) -> Result<Option<T>, String> {
        match self.tag(2)? {
            0 => Ok(None),
            _ => read(self).map(Some),
        }
    }

    pub fn list<T>(
        &mut self,
        mut read: impl FnMut(&mut Reader<'a>) -> Result<T, String>,
    ) -> Result<Vec<T>, String> {
        let n = self.length()?;
        (0..n).map(|_| read(self)).collect()
    }
}
