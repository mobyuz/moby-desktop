//! The one audio format moby-api speaks: RIFF/WAVE, PCM, mono, 16 kHz, 16-bit. The API checks the
//! header byte for byte (`moby-api/src/voice/wave.rs`), so the header here is the same 44 bytes.
//! Everything is in memory; nothing here touches a file.

/// 30 s is the API's own limit for one clip.
pub const MAX_PCM_BYTES: usize = 16_000 * 2 * 30;
/// The API allows 4 KiB of header and metadata around the audio.
pub const MAX_WAV_BYTES: usize = MAX_PCM_BYTES + 4096;

// PCM, one channel, 16 kHz, 32 kB/s, two bytes per frame, 16 bits.
const PCM_FORMAT: [u8; 16] = [1, 0, 1, 0, 128, 62, 0, 0, 0, 125, 0, 0, 2, 0, 16, 0];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WavError {
    /// Not a WAV this app takes.
    Invalid,
    TooLarge,
}

/// Wraps raw 16 kHz mono 16-bit PCM in the header the API wants. `None` if it is empty, odd or
/// longer than the API takes.
pub fn encode(pcm: &[u8]) -> Option<Vec<u8>> {
    if pcm.is_empty() || !pcm.len().is_multiple_of(2) || pcm.len() > MAX_PCM_BYTES {
        return None;
    }
    let size = u32::try_from(pcm.len()).ok()?;
    let mut wave = Vec::with_capacity(44 + pcm.len());
    wave.extend_from_slice(b"RIFF");
    wave.extend_from_slice(&(36 + size).to_le_bytes());
    wave.extend_from_slice(b"WAVEfmt ");
    wave.extend_from_slice(&16_u32.to_le_bytes());
    wave.extend_from_slice(&PCM_FORMAT);
    wave.extend_from_slice(b"data");
    wave.extend_from_slice(&size.to_le_bytes());
    wave.extend_from_slice(pcm);
    Some(wave)
}

fn le32(bytes: &[u8]) -> Option<usize> {
    let array: [u8; 4] = bytes.get(..4)?.try_into().ok()?;
    usize::try_from(u32::from_le_bytes(array)).ok()
}

/// The samples of a WAV from the API. Anything that is not exactly the agreed format is refused,
/// so a bad answer is never played.
pub fn decode(wave: &[u8]) -> Result<Vec<i16>, WavError> {
    if wave.len() > MAX_WAV_BYTES {
        return Err(WavError::TooLarge);
    }
    let header = wave.get(..12).ok_or(WavError::Invalid)?;
    if &header[..4] != b"RIFF"
        || &header[8..12] != b"WAVE"
        || le32(&header[4..8]) != wave.len().checked_sub(8)
    {
        return Err(WavError::Invalid);
    }
    let mut rest = &wave[12..];
    let mut format_seen = false;
    let mut audio: Option<&[u8]> = None;
    while !rest.is_empty() {
        let head = rest.get(..8).ok_or(WavError::Invalid)?;
        let size = le32(&head[4..8]).ok_or(WavError::Invalid)?;
        let body = rest.get(8..).ok_or(WavError::Invalid)?;
        let contents = body.get(..size).ok_or(WavError::Invalid)?;
        rest = body.get(size..).ok_or(WavError::Invalid)?;
        if size % 2 == 1 {
            rest = rest.get(1..).ok_or(WavError::Invalid)?;
        }
        match &head[..4] {
            b"fmt " if !format_seen => {
                if contents.get(..PCM_FORMAT.len()) != Some(PCM_FORMAT.as_slice())
                    || !matches!(contents.get(PCM_FORMAT.len()..), Some([] | [0, 0]))
                {
                    return Err(WavError::Invalid);
                }
                format_seen = true;
            }
            b"data" if format_seen && audio.is_none() => audio = Some(contents),
            b"fmt " | b"data" => return Err(WavError::Invalid),
            _ => {}
        }
    }
    let audio = audio.ok_or(WavError::Invalid)?;
    if audio.is_empty() || audio.len() % 2 != 0 || audio.len() > MAX_PCM_BYTES {
        return Err(WavError::Invalid);
    }
    Ok(audio
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| i16::from_le_bytes(*pair))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_then_decode_returns_the_same_samples() {
        let pcm: Vec<u8> = (0..400_u16)
            .flat_map(|n| (n as i16 - 200).to_le_bytes())
            .collect();
        let wave = encode(&pcm).unwrap();
        assert_eq!(wave.len(), 44 + pcm.len());
        let samples = decode(&wave).unwrap();
        assert_eq!(samples.len(), 400);
        assert_eq!(samples[0], -200);
        assert_eq!(samples[399], 199);
    }

    #[test]
    fn bad_input_is_refused_in_both_directions() {
        assert!(encode(&[]).is_none());
        assert!(encode(&[1]).is_none());
        assert!(encode(&vec![0; MAX_PCM_BYTES + 2]).is_none());
        assert!(encode(&vec![0; MAX_PCM_BYTES]).is_some());
        let good = encode(&[0, 1, 2, 3]).unwrap();
        assert_eq!(decode(&good[..40]), Err(WavError::Invalid));
        let mut stereo = good.clone();
        stereo[22] = 2;
        assert_eq!(decode(&stereo), Err(WavError::Invalid));
        let mut rate = good.clone();
        rate[24] = 0x44;
        assert_eq!(decode(&rate), Err(WavError::Invalid));
        let mut size = good.clone();
        size[4] = 0;
        assert_eq!(decode(&size), Err(WavError::Invalid));
        assert_eq!(decode(&vec![0; MAX_WAV_BYTES + 1]), Err(WavError::TooLarge));
        assert_eq!(decode(b"not a wave file at all"), Err(WavError::Invalid));
    }
}
