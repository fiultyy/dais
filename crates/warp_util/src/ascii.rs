//! P-ARC R8: 原 app/src/util 的三个零依赖 ASCII 工具提升至此,
//! app 侧 `crate::util` re-export 保持旧路径兼容。

/// Parses a sequence of ASCII digit bytes into a u32. Returns None if the
/// sequence is empty, contains a non-digit, or overflows.
pub fn parse_ascii_u32(bytes: &[u8]) -> Option<u32> {
    if bytes.is_empty() {
        return None;
    }

    let mut result: u32 = 0;
    for &byte in bytes {
        if !byte.is_ascii_digit() {
            return None;
        }
        result = result.checked_mul(10)?.checked_add((byte - b'0') as u32)?;
    }
    Some(result)
}

/// AsciiDebug is intended to make it easy to inspect the contents of byte slices that are mostly ASCII
/// characters (but may not be valid unicode). It changes the output of the wrapped byte slice to
/// a human readable string with non-ASCII characters written as hex escapes.
///
/// E.g. `log::info!("{:?}", &AsciiDebug(some_byte_slice));`
pub struct AsciiDebug<'a>(pub &'a [u8]);

impl std::fmt::Debug for AsciiDebug<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "\"")?;
        for &byte in self.0 {
            // Check if the byte is a standard printable character.
            if (32..126).contains(&byte) {
                write!(f, "{}", byte as char)?;
            } else {
                write!(f, "\\{{{byte:02X}}}")?;
            }
        }
        write!(f, "\"")?;
        Ok(())
    }
}
