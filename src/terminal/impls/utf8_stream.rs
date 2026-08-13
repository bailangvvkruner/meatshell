/// Incrementally decodes a UTF-8 byte stream without treating read or SSH
/// packet boundaries as encoding errors.
#[derive(Default)]
pub(crate) struct Utf8StreamDecoder {
    pending: Vec<u8>,
}

impl Utf8StreamDecoder {
    pub(crate) fn decode(&mut self, input: &[u8]) -> String {
        let mut bytes = std::mem::take(&mut self.pending);
        bytes.extend_from_slice(input);
        let mut rest = bytes.as_slice();
        let mut output = String::new();

        while !rest.is_empty() {
            match std::str::from_utf8(rest) {
                Ok(valid) => {
                    output.push_str(valid);
                    break;
                }
                Err(error) => {
                    let valid_len = error.valid_up_to();
                    if valid_len > 0 {
                        output.push_str(std::str::from_utf8(&rest[..valid_len]).unwrap());
                    }
                    match error.error_len() {
                        Some(invalid_len) => {
                            output.push('\u{fffd}');
                            rest = &rest[valid_len + invalid_len..];
                        }
                        None => {
                            self.pending.extend_from_slice(&rest[valid_len..]);
                            debug_assert!(self.pending.len() <= 3);
                            break;
                        }
                    }
                }
            }
        }

        output
    }

    pub(crate) fn finish(&mut self) -> String {
        let pending = std::mem::take(&mut self.pending);
        String::from_utf8_lossy(&pending).into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::Utf8StreamDecoder;

    #[test]
    fn preserves_multibyte_text_split_at_every_byte() {
        let expected = "ASCII: btop 中文 ─│┌┐ 😀 done";
        let mut decoder = Utf8StreamDecoder::default();
        let mut actual = String::new();
        for byte in expected.as_bytes() {
            actual.push_str(&decoder.decode(&[*byte]));
            assert!(decoder.pending.len() <= 3);
        }
        actual.push_str(&decoder.finish());
        assert_eq!(actual, expected);
    }

    #[test]
    fn replaces_invalid_sequences_and_flushes_incomplete_tails() {
        let mut decoder = Utf8StreamDecoder::default();
        assert_eq!(decoder.decode(b"ok\xff!"), "ok\u{fffd}!");
        assert_eq!(decoder.decode(&[0xe2]), "");
        assert_eq!(decoder.decode(b"A"), "\u{fffd}A");
        assert_eq!(decoder.decode(&[0xf0, 0x9f, 0x92]), "");
        assert_eq!(decoder.finish(), "\u{fffd}");
    }
}
