// Copyright 2018-2024 the Deno authors. MIT license.

use std::borrow::Cow;
use std::path::Path;

pub trait DenoPkgJsonFs {
  fn read_to_string_lossy(
    &self,
    path: &Path,
  ) -> Result<String, std::io::Error> {
    // allowed here for the real fs
    #[allow(clippy::disallowed_methods)]
    let bytes = std::fs::read(path)?;
    Ok(string_from_utf8_lossy(bytes))
  }
}

impl<'a> Default for &'a dyn DenoPkgJsonFs {
  fn default() -> Self {
    &RealDenoPkgJsonFs
  }
}

#[derive(Debug, Clone, Copy)]
pub struct RealDenoPkgJsonFs;

impl DenoPkgJsonFs for RealDenoPkgJsonFs {
  fn read_to_string_lossy(
    &self,
    path: &Path,
  ) -> Result<String, std::io::Error> {
    // allowed here for the real fs
    #[allow(clippy::disallowed_methods)]
    let bytes = std::fs::read(path)?;
    Ok(string_from_utf8_lossy(bytes))
  }
}

// Like String::from_utf8_lossy but operates on owned values
#[inline(always)]
fn string_from_utf8_lossy(buf: Vec<u8>) -> String {
  match String::from_utf8_lossy(&buf) {
    // buf contained non-utf8 chars than have been patched
    Cow::Owned(s) => s,
    // SAFETY: if Borrowed then the buf only contains utf8 chars,
    // we do this instead of .into_owned() to avoid copying the input buf
    Cow::Borrowed(_) => unsafe { String::from_utf8_unchecked(buf) },
  }
}
