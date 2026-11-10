//! Chromaprint fingerprints: decoding and comparing them.
//!
//! `fpcalc -json` prints a *compressed* fingerprint ("AQAD..."). Two encodes
//! of the same recording (a YouTube Opus download and a FLAC rip, say) never
//! produce the same string, so hashing it cannot find duplicates. Instead we
//! [`decode`] the string back into Chromaprint's raw 32-bit sub-fingerprints
//! (one every ~0.124 s) and measure how many bits two of them share with
//! [`similarity`].

use base64::Engine as _;
use base64::alphabet;
use base64::engine::general_purpose::{GeneralPurpose, NO_PAD_INDIFFERENT};

/// Chromaprint writes URL-safe base64 without padding. Be lenient about
/// padding and unused trailing bits: neither carries data.
const BASE64: GeneralPurpose = GeneralPurpose::new(
    &alphabet::URL_SAFE,
    NO_PAD_INDIFFERENT.with_decode_allow_trailing_bits(true),
);

/// A 3-bit "normal" value of 7 means "7 plus the next 5-bit exceptional value".
const MAX_NORMAL_VALUE: u8 = 7;

/// How far (in sub-fingerprints, ~0.124 s each) one fingerprint may be shifted
/// against the other: about ±15 s, enough for a different intro or silence.
const MAX_OFFSET: isize = 120;

/// The fewest sub-fingerprints (~12 s) that must overlap at an offset for it
/// to count. Shorter overlaps match unrelated audio too easily.
const MIN_OVERLAP: usize = 100;

/// The [`similarity`] at or above which two fingerprints are taken to be the
/// same recording.
///
/// Measured with `fpcalc` 1.6.1 (default 120 s) on two synthetic 60 s
/// songs (melody, vibrato lead, bass line, pulsed pink noise) and a real
/// 2:41 pop song, each re-encoded from a WAV master:
///
/// - the same recording as WAV, FLAC, MP3 128k/64k, AAC 128k, Opus 64k/32k:
///   0.980–1.000 (FLAC vs WAV: 1.000; MP3 128k vs WAV: 0.996–1.000;
///   the weakest pairs are AAC vs Opus and Opus 32k vs MP3 64k: 0.980)
/// - with 3 s of leading silence vs without: 0.951–0.971
/// - "YouTube-like" (1.5 s trimmed, loudness-normalised, mono, 48 kHz Opus
///   48k) vs the master: 0.975
/// - heavy EQ plus compression ("another master"): 0.87–0.88
/// - the same song played 2 % faster: 0.63 (Chromaprint is not tempo-proof)
/// - different songs: 0.45–0.57
/// - the same real song from 1:50 on vs from the start: 0.69 (repeated
///   choruses and loops line up; the worst "different audio" case seen)
///
/// 0.85 sits 0.16 above the worst different-audio score and 0.10 below the
/// weakest plain re-encode. It leans towards the "different" side because
/// merging two different songs is worse than missing a duplicate.
pub const SAME_RECORDING: f64 = 0.85;

/// Decodes a compressed Chromaprint fingerprint (as printed by `fpcalc`
/// without `-raw`) into its raw 32-bit sub-fingerprints.
///
/// Returns `None` when the string is not a well-formed fingerprint.
pub fn decode(compressed: &str) -> Option<Vec<u32>> {
    let bytes = BASE64.decode(compressed.trim()).ok()?;
    if bytes.len() < 4 {
        return None;
    }
    // bytes[0] is the algorithm id; it does not change the encoding.
    let num_values =
        (usize::from(bytes[1]) << 16) | (usize::from(bytes[2]) << 8) | usize::from(bytes[3]);
    if num_values == 0 {
        return Some(Vec::new());
    }
    let body = &bytes[4..];

    // The 3-bit stream: read until `num_values` zeros (value terminators).
    let mut bits = Vec::new();
    let mut found_values = 0;
    let mut num_exceptional = 0usize;
    for index in 0.. {
        let value = read_packed(body, index, 3)?;
        bits.push(value);
        if value == 0 {
            found_values += 1;
            if found_values == num_values {
                break;
            }
        } else if value == MAX_NORMAL_VALUE {
            num_exceptional += 1;
        }
    }

    // The 5-bit exceptional stream starts at the next whole byte.
    let exceptional = body.get((bits.len() * 3).div_ceil(8)..)?;
    if exceptional.len() < (num_exceptional * 5).div_ceil(8) {
        return None;
    }
    let escaped = bits.iter_mut().filter(|bit| **bit == MAX_NORMAL_VALUE);
    for (index, bit) in escaped.enumerate() {
        *bit += read_packed(exceptional, index, 5)?;
    }

    // Each value is a list of set bit positions as deltas, ended by a 0; the
    // values themselves are XOR deltas of their predecessor.
    let mut output = Vec::with_capacity(num_values);
    let mut value = 0u32;
    let mut last_bit = 0u32;
    for bit in bits {
        if bit == 0 {
            let previous = output.last().copied().unwrap_or(0);
            output.push(value ^ previous);
            value = 0;
            last_bit = 0;
            continue;
        }
        last_bit += u32::from(bit);
        if last_bit > 32 {
            return None;
        }
        value |= 1 << (last_bit - 1);
    }
    Some(output)
}

/// Reads the `index`-th `width`-bit value of an LSB-first packed bit stream,
/// or `None` past its end.
fn read_packed(bytes: &[u8], index: usize, width: usize) -> Option<u8> {
    let start = index * width;
    let byte = start / 8;
    let shift = start % 8;
    let low = u16::from(*bytes.get(byte)?);
    let word = if shift + width > 8 {
        low | (u16::from(*bytes.get(byte + 1)?) << 8)
    } else {
        low
    };
    Some(((word >> shift) & ((1 << width) - 1)) as u8)
}

/// How alike two raw fingerprints are, from 0.0 to 1.0.
///
/// Tries every time offset of up to ±120 sub-fingerprints (~±15 s) that
/// leaves at least 100 (~12 s) overlapping, scores each as the share of
/// equal bits in the overlap, and returns the best. 0.0 when no offset
/// overlaps enough (one of them is shorter than ~12 s).
///
/// What the numbers mean:
/// - ~1.0: the same audio (a lossless copy, or a re-tagged file)
/// - ≥ ~0.95: the same recording in another encoding (MP3, Opus, AAC, FLAC)
///   or with a few seconds more or less of silence at the start
/// - ~0.85–0.9: the same recording, mastered differently
/// - ~0.45–0.6: unrelated music (random bits agree half of the time);
///   different parts of one song can reach ~0.7
///
/// See [`SAME_RECORDING`] for the threshold and the numbers behind it.
/// A fingerprint that can tell songs apart. Silence, a steady tone or one
/// held chord give the same value over and over (0% distinct values, and two
/// different tones still score 0.91 "similar"); real music changes all the
/// time (92–97% distinct values measured). Below half distinct, the
/// fingerprint proves nothing.
pub fn is_distinctive(raw: &[u32]) -> bool {
    if raw.len() < MIN_OVERLAP {
        return false;
    }
    let distinct = raw.iter().collect::<std::collections::HashSet<_>>().len();
    distinct * 2 >= raw.len()
}

pub fn similarity(a: &[u32], b: &[u32]) -> f64 {
    let mut best = 0.0f64;
    for offset in -MAX_OFFSET..=MAX_OFFSET {
        // Pair a[i] with b[i + offset].
        let (a, b) = if offset >= 0 {
            (a, b.get(offset as usize..).unwrap_or_default())
        } else {
            (a.get(offset.unsigned_abs()..).unwrap_or_default(), b)
        };
        let overlap = a.len().min(b.len());
        if overlap < MIN_OVERLAP {
            continue;
        }
        let differing: u32 = a.iter().zip(b).map(|(x, y)| (x ^ y).count_ones()).sum();
        let score = 1.0 - f64::from(differing) / (32.0 * overlap as f64);
        best = best.max(score);
    }
    best
}

/// Whether two compressed fingerprints (as printed by `fpcalc -json`) are
/// the same recording. `false` when either one cannot be decoded. The pipeline
/// uses [`crate::application::input_dedup::sounds_the_same`], which also checks lengths.
#[cfg(test)]
pub fn same_recording(a: &str, b: &str) -> bool {
    match (decode(a), decode(b)) {
        (Some(a), Some(b)) => similarity(&a, &b) >= SAME_RECORDING,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silence_and_steady_tones_prove_nothing() {
        assert!(!is_distinctive(&vec![0x1234_5678; 900]));
        assert!(!is_distinctive(&[1, 2, 3]));
        let music = (0..900_u32)
            .map(|value| value.wrapping_mul(2_654_435_761))
            .collect::<Vec<_>>();
        assert!(is_distinctive(&music));
        // A song with a long silent intro still counts.
        let mut intro = vec![0; 300];
        intro.extend_from_slice(&music[..600]);
        assert!(is_distinctive(&intro));
    }

    /// `fpcalc -json` (fpcalc 1.6.1) of the first 15 s of a synthetic test
    /// song, saved as FLAC.
    const FIXTURE: &str = "AQAAZOSSSImHO8d2nBeuHOsOTUxynMF5TD-eUEoObiMxS5FTlPyC9EOSdcvRRpKI_sSj409sWBNRKl6NHIlyJ_Cz9PiR-tCOP8H_DPkjfA6eBIcPmsN6NSJOp0j45MjL487h53jwExVXCk3CJwiSo9cSoinDBbeCMPl0JFMUPJXRfJmC8viOnglKRSSeMEV0PeB-yE_QfNkRqntw5sOOZ0cTtwrK_Lgj1AzyBsk2ZbiO68YjnfBx6AqeyCEeDl-HR8PF9OjRdFwNHH2I_sefSMi7Qz4ikzlaDk1vXKGLP0bzDKQQkUACy5gjCghBARDGAEMWEIwQBgAQRgKBGBEASCQQAYQQAiAgACDhKBDECEmAEIAAw4AAAiEDICGmJGYJAAaQIQgBBgEgDAEAAQ";
    /// `fpcalc -raw -json` of the same file.
    const FIXTURE_RAW: [u32; 100] = [
        302955656, 839760008, 839694508, 1644997036, 1611442924, 1611574888, 3759051322,
        3758981690, 2685178426, 2718741038, 2785797102, 2786783654, 2927248546, 2951370162,
        2913621395, 2896785844, 2901157796, 2892246964, 2888055220, 3978508788, 3978440020,
        1865424964, 1831870533, 1827741790, 749867134, 747765886, 746719327, 746588237, 746645517,
        1065396301, 973187148, 973253836, 973246668, 973242572, 973258988, 973333220, 415326816,
        411199008, 1484920323, 1217601026, 1284512258, 1288710690, 1288710434, 1290938402,
        1303776370, 1302987906, 1302987907, 1303053440, 1307323536, 1294264480, 1294444960,
        1327663569, 1327664898, 1311212290, 1324655410, 1320460834, 1253941794, 1253749298,
        1251516194, 1250449442, 1246234663, 3389524079, 3389638893, 3659196637, 3665469580,
        3672805772, 3672801672, 3664490888, 3643384312, 3626738408, 3626680936, 3626632824,
        3626569337, 3626515178, 3630706410, 4175966122, 4238869674, 2091385994, 2108163210,
        2108216456, 1969857672, 1999611016, 1999023496, 1999028040, 1730317832, 1730315800,
        3845169704, 3845169704, 3828392488, 3828519480, 2752676392, 2756018792, 2689370729,
        2689387243, 2723142638, 2723084798, 2791243214, 2726195406, 2725122190, 2726105278,
    ];

    /// Deterministic pseudo-random sub-fingerprints (xorshift32).
    fn noise(seed: u32, len: usize) -> Vec<u32> {
        let mut state = seed.max(1);
        (0..len)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                state
            })
            .collect()
    }

    #[test]
    fn decodes_what_fpcalc_prints() {
        assert_eq!(decode(FIXTURE).unwrap(), FIXTURE_RAW);
        // Surrounding whitespace and base64 padding are tolerated.
        assert_eq!(decode(&format!(" {FIXTURE}==\n")).unwrap(), FIXTURE_RAW);
    }

    #[test]
    fn malformed_fingerprints_are_rejected() {
        for input in ["", "AQ", "AQAA", "not base64!", "AQAD+/=="] {
            assert_eq!(decode(input), None, "{input:?}");
        }
        // Cut short: the 3-bit stream or the exceptional stream runs out.
        for cut in [5, FIXTURE.len() / 2, FIXTURE.len() - 4] {
            assert_eq!(decode(&FIXTURE[..cut]), None, "cut at {cut}");
        }
        // Claims more values than the data holds.
        let mut bytes = BASE64.decode(FIXTURE).unwrap();
        bytes[1] = 0xff;
        assert_eq!(decode(&BASE64.encode(&bytes)), None);
        // A bit position past 32: 7 + an exceptional 31 = bit 38.
        let mut bytes = vec![1, 0, 0, 1];
        bytes.extend([0xff; 5]); // 13 normal 7s and one bit more
        bytes.push(0x00); // a terminating 0
        bytes.extend([0xff; 10]); // exceptional values of 31
        assert_eq!(decode(&BASE64.encode(&bytes)), None);
    }

    #[test]
    fn an_empty_fingerprint_decodes_to_nothing() {
        assert_eq!(decode(&BASE64.encode([1, 0, 0, 0])), Some(Vec::new()));
    }

    #[test]
    fn identical_fingerprints_are_fully_similar() {
        let song = noise(1, 1000);
        assert_eq!(similarity(&song, &song), 1.0);
        assert_eq!(similarity(&FIXTURE_RAW, &FIXTURE_RAW), 1.0);
    }

    #[test]
    fn a_shifted_start_is_found() {
        let song = noise(2, 800);
        let mut with_intro = noise(3, 40);
        with_intro.extend(&song);
        assert_eq!(similarity(&song, &with_intro), 1.0);
        assert_eq!(similarity(&with_intro, &song), 1.0);
        // Beyond ±120 items the start is not looked for.
        let mut long_intro = noise(4, 200);
        long_intro.extend(&song);
        assert!(similarity(&song, &long_intro) < 0.6);
    }

    #[test]
    fn a_few_flipped_bits_still_match() {
        let song = noise(5, 1000);
        let mut copy = song.clone();
        for (index, value) in copy.iter_mut().enumerate() {
            *value ^= 1 << (index % 32); // 1 of 32 bits differs
        }
        let score = similarity(&song, &copy);
        assert!((score - 31.0 / 32.0).abs() < 1e-9, "{score}");
    }

    #[test]
    fn unrelated_fingerprints_score_about_half() {
        let score = similarity(&noise(6, 1000), &noise(7, 1000));
        assert!((0.45..0.55).contains(&score), "{score}");
        assert!(score < SAME_RECORDING);
    }

    #[test]
    fn too_little_overlap_scores_zero() {
        let short = noise(8, MIN_OVERLAP - 1);
        assert_eq!(similarity(&short, &short), 0.0);
        assert_eq!(similarity(&[], &noise(9, 500)), 0.0);
        let exact = noise(10, MIN_OVERLAP);
        assert_eq!(similarity(&exact, &exact), 1.0);
    }

    #[test]
    fn same_recording_compares_compressed_strings() {
        assert!(same_recording(FIXTURE, FIXTURE));
        assert!(!same_recording(FIXTURE, "garbage"));
        assert!(!same_recording("", ""));
    }

    /// Runs only when ffmpeg and fpcalc are installed.
    #[test]
    fn encodes_of_the_same_song_match() {
        let installed = |tool: &str| {
            std::process::Command::new(tool)
                .arg("-version")
                .output()
                .is_ok_and(|output| output.status.success())
        };
        if !installed("ffmpeg") || !installed("fpcalc") {
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let encode = |name: &str, source: &str, codec: &[&str]| {
            let path = directory.path().join(name);
            let status = std::process::Command::new("ffmpeg")
                .args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i"])
                .arg(source)
                .args(codec)
                .arg(&path)
                .status()
                .unwrap();
            assert!(status.success(), "{name}");
            path
        };
        let fingerprint = |path: &std::path::Path, raw: bool| {
            let mut command = std::process::Command::new("fpcalc");
            if raw {
                command.arg("-raw");
            }
            let output = command.arg("-json").arg(path).output().unwrap();
            assert!(output.status.success(), "{}", path.display());
            let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            json["fingerprint"].clone()
        };
        let song = "aevalsrc=0.4*sin(2*PI*t*220*pow(2\\,mod(floor(t*3)*5\\,12)/12))\
                    *(0.6+0.4*sin(3*PI*t))+0.3*sin(1250*2*PI*t*(1+0.2*sin(0.7*t)))\
                    +0.25*sin(2*PI*t*110*pow(2\\,mod(floor(t/2)*7\\,12)/12)):s=44100:d=30";
        let other = "aevalsrc=0.4*sin(2*PI*t*196*pow(2\\,mod(floor(t*2.3)*3\\,12)/12))\
                     *(0.6+0.4*sin(5*PI*t))+0.3*sin(880*2*PI*t*(1+0.1*sin(1.3*t))):s=44100:d=30";
        let files = [
            encode("song.flac", song, &["-c:a", "flac"]),
            encode("song.mp3", song, &["-c:a", "libmp3lame", "-b:a", "128k"]),
            encode("song.opus", song, &["-c:a", "libopus", "-b:a", "64k"]),
        ];
        let other = encode("other.mp3", other, &["-c:a", "libmp3lame", "-b:a", "128k"]);

        let mut compressed = Vec::new();
        for path in files.iter().chain([&other]) {
            let text = fingerprint(path, false);
            let raw: Vec<u32> = serde_json::from_value(fingerprint(path, true)).unwrap();
            let text = text.as_str().unwrap().to_owned();
            assert_eq!(decode(&text).unwrap(), raw, "{}", path.display());
            compressed.push(text);
        }
        let other = compressed.pop().unwrap();
        for a in &compressed {
            for b in &compressed {
                assert!(same_recording(a, b));
            }
            assert!(!same_recording(a, &other));
        }
    }
}
