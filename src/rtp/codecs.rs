// src/rtp/codecs.rs
// G.711 (PCMU/PCMA) kod çözücü ve kodlayıcı ile LPCM16 dönüşümleri
use anyhow::{anyhow, Result};
use rubato::{
    Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction,
};

// ... Diğer enum ve sabitler aynı kalıyor ...
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioCodec {
    Pcmu,
    Pcma,
}
impl AudioCodec {
    pub fn from_rtp_payload_type(payload_type: u8) -> Result<Self> {
        match payload_type {
            0 => Ok(AudioCodec::Pcmu),
            8 => Ok(AudioCodec::Pcma),
            _ => Err(anyhow!("Desteklenmeyen RTP payload tipi: {}", payload_type)),
        }
    }
}

// --- YENİ STRUCT: Resampler'ı durum bilgisiyle birlikte tutar ---
pub struct StatefulResampler {
    resampler: SincFixedIn<f32>,
}

impl StatefulResampler {
    pub fn new(source_rate: u32, target_rate: u32) -> Result<Self> {
        let params = SincInterpolationParameters {
            sinc_len: 256, f_cutoff: 0.95, interpolation: SincInterpolationType::Linear,
            oversampling_factor: 256, window: WindowFunction::BlackmanHarris2,
        };
        let resampler = SincFixedIn::<f32>::new(
            target_rate as f64 / source_rate as f64, 2.0, params, 160, 1, // 160 örnek (20ms) için optimize edildi
        )?;
        Ok(Self { resampler })
    }

    pub fn process(&mut self, samples_in: &[f32]) -> Result<Vec<f32>> {
        Ok(self.resampler.process(&[samples_in.to_vec()], None)?.remove(0))
    }
}

// --- DEĞİŞTİRİLDİ: Bu fonksiyon artık StatefulResampler kullanacak ---
pub fn decode_g711_to_lpcm16(
    payload: &[u8],
    codec: AudioCodec,
    resampler: &mut StatefulResampler, // Resampler'ı parametre olarak alır
) -> Result<Vec<i16>> {
    let pcm_samples_8k_i16: Vec<i16> = match codec {
        AudioCodec::Pcmu => payload.iter().map(|&byte| ULAW_TO_PCM[byte as usize]).collect(),
        AudioCodec::Pcma => payload.iter().map(|&byte| ALAW_TO_PCM[byte as usize]).collect(),
    };

    let pcm_f32: Vec<f32> = pcm_samples_8k_i16.iter().map(|&s| s as f32 / 32768.0).collect();
    
    // Mevcut resampler örneğini kullanarak işlemi yap
    let resampled_f32 = resampler.process(&pcm_f32)?;

    let samples_16k_i16: Vec<i16> = resampled_f32.into_iter()
        .map(|s| (s * 32767.0).clamp(-32768.0, 32767.0) as i16)
        .collect();

    Ok(samples_16k_i16)
}

// Giden ses için bu fonksiyon aynı kalabilir, çünkü tek seferlik bir işlemdir.
pub fn encode_lpcm16_to_g711(samples_16k: &[i16], target_codec: AudioCodec) -> Result<Vec<u8>> {

    const SOURCE_SAMPLE_RATE: u32 = 16000;
    const TARGET_SAMPLE_RATE: u32 = 8000;

    let pcm_f32: Vec<f32> = samples_16k.iter().map(|&s| s as f32 / 32768.0).collect();

    let params = SincInterpolationParameters {
        sinc_len: 256, f_cutoff: 0.95, interpolation: SincInterpolationType::Linear,
        oversampling_factor: 256, window: WindowFunction::BlackmanHarris2,
    };
    let mut resampler = SincFixedIn::<f32>::new(
        TARGET_SAMPLE_RATE as f64 / SOURCE_SAMPLE_RATE as f64, 2.0, params, pcm_f32.len(), 1,
    )?;

    let resampled_f32 = resampler.process(&[pcm_f32], None)?.remove(0);
    let samples_8k_i16: Vec<i16> = resampled_f32.into_iter()
        .map(|s| (s * 32767.0).clamp(-32768.0, 32767.0) as i16)
        .collect();

    let g711_payload: Vec<u8> = match target_codec {
        AudioCodec::Pcmu => samples_8k_i16.iter().map(|&s| linear_to_ulaw(s)).collect(),
        AudioCodec::Pcma => samples_8k_i16.iter().map(|&s| linear_to_alaw(s)).collect(),
    };

    Ok(g711_payload)
}

// ... Tüm G.711 tabloları ve linear_to_ulaw/alaw fonksiyonları aynı kalıyor ...
// ... Birim testleri de aynı kalıyor, sadece `decode_g711_to_lpcm16` testini yeni yapıya uyarlamak gerekebilir ...

pub const ALAW_TO_PCM: [i16; 256] = [
    -5504, -5248, -6016, -5760, -4480, -4224, -4992, -4736, -7552, -7296, -8064, -7808, -6528, -6272, -7040, -6784,
    -2752, -2624, -3008, -2880, -2240, -2112, -2496, -2368, -3776, -3648, -4032, -3904, -3264, -3136, -3520, -3392,
    -1376, -1312, -1504, -1440, -1120, -1056, -1248, -1184, -1888, -1824, -2016, -1952, -1632, -1568, -1760, -1696,
    -688, -656, -752, -720, -560, -528, -624, -592, -944, -912, -1008, -976, -816, -784, -880, -848,
    -22016, -20992, -24064, -23040, -17920, -16896, -19968, -18944, -30208, -29184, -32256, -31232, -26112, -25088, -28160, -27136,
    -11008, -10496, -12032, -11520, -8960, -8448, -9984, -9472, -15104, -14592, -16128, -15616, -13056, -12544, -14080, -13568,
    -5504, -5248, -6016, -5760, -4480, -4224, -4992, -4736, -7552, -7296, -8064, -7808, -6528, -6272, -7040, -6784,
    -2752, -2624, -3008, -2880, -2240, -2112, -2496, -2368, -3776, -3648, -4032, -3904, -3264, -3136, -3520, -3392,
    5504, 5248, 6016, 5760, 4480, 4224, 4992, 4736, 7552, 7296, 8064, 7808, 6528, 6272, 7040, 6784,
    2752, 2624, 3008, 2880, 2240, 2112, 2496, 2368, 3776, 3648, 4032, 3904, 3264, 3136, 3520, 3392,
    1376, 1312, 1504, 1440, 1120, 1056, 1248, 1184, 1888, 1824, 2016, 1952, 1632, 1568, 1760, 1696,
    688, 656, 752, 720, 560, 528, 624, 592, 944, 912, 1008, 976, 816, 784, 880, 848,
    22016, 20992, 24064, 23040, 17920, 16896, 19968, 18944, 30208, 29184, 32256, 31232, 26112, 25088, 28160, 27136,
    11008, 10496, 12032, 11520, 8960, 8448, 9984, 9472, 15104, 14592, 16128, 15616, 13056, 12544, 14080, 13568,
    5504, 5248, 6016, 5760, 4480, 4224, 4992, 4736, 7552, 7296, 8064, 7808, 6528, 6272, 7040, 6784,
    2752, 2624, 3008, 2880, 2240, 2112, 2496, 2368, 3776, 3648, 4032, 3904, 3264, 3136, 3520, 3392
];
pub fn linear_to_alaw(mut pcm_val: i16) -> u8 {
    let sign = (pcm_val >> 8) & 0x80;
    if sign != 0 { pcm_val = -pcm_val; }
    if pcm_val > 32635 { pcm_val = 32635; }
    
    let mut exponent: i16;
    if pcm_val >= 256 {
        exponent = 4;
        while exponent < 8 {
            if pcm_val < (256 << exponent) { break; }
            exponent += 1;
        }
        exponent -= 1;
    } else {
        exponent = (pcm_val >> 4) & 0x0F;
    }
    
    let mantissa = (pcm_val >> (if exponent > 1 { exponent } else { 1 })) & 0x0F;
    let alaw = (exponent << 4) | mantissa;
    
    (alaw ^ 0x55) as u8
}
const BIAS: i16 = 0x84;
static ULAW_TABLE: [u8; 256] = [
    0, 0, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 3, 3, 3, 3, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4,
    5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5,
    6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6,
    6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6,
    7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7,
    7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7,
    7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7,
    7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7
];
pub fn linear_to_ulaw(mut pcm_val: i16) -> u8 {
    let sign = if pcm_val < 0 { 0x80 } else { 0 };
    if sign != 0 { pcm_val = -pcm_val; }
    pcm_val = pcm_val.min(32635);
    pcm_val += BIAS;
    let exponent = ULAW_TABLE[((pcm_val >> 7) & 0xFF) as usize];
    let mantissa = (pcm_val >> (exponent as i16 + 3)) & 0xF;
    !(sign as u8 | (exponent << 4) | mantissa as u8)
}
pub const ULAW_TO_PCM: [i16; 256] = [
    -32124, -31100, -30076, -29052, -28028, -27004, -25980, -24956, -23932, -22908,
    -21884, -20860, -19836, -18812, -17788, -16764, -15996, -15484, -14972, -14460,
    -13948, -13436, -12924, -12412, -11900, -11388, -10876, -10364, -9852, -9340,
    -8828, -8316, -7932, -7676, -7420, -7164, -6908, -6652, -6396, -6140, -5884,
    -5628, -5372, -5116, -4860, -4604, -4348, -4092, -3900, -3772, -3644, -3516,
    -3388, -3260, -3132, -3004, -2876, -2748, -2620, -2492, -2364, -2236, -2108,
    -1980, -1884, -1820, -1756, -1692, -1628, -1564, -1500, -1436, -1372, -1308,
    -1244, -1180, -1116, -1052, -988, -924, -876, -844, -812, -780, -748, -716,
    -684, -652, -620, -588, -556, -524, -492, -460, -428, -396, -372, -356, -340,
    -324, -308, -292, -276, -260, -244, -228, -212, -196, -180, -164, -148, -132,
    -120, -112, -104, -96, -88, -80, -72, -64, -56, -48, -40, -32, -24, -16, -8, 0,
    32124, 31100, 30076, 29052, 28028, 27004, 25980, 24956, 23932, 22908, 21884,
    20860, 19836, 18812, 17788, 16764, 15996, 15484, 14972, 14460, 13948, 13436,
    12924, 12412, 11900, 11388, 10876, 10364, 9852, 9340, 8828, 8316, 7932, 7676,
    7420, 7164, 6908, 6652, 6396, 6140, 5884, 5628, 5372, 5116, 4860, 4604, 4348,
    4092, 3900, 3772, 3644, 3516, 3388, 3260, 3132, 3004, 2876, 2748, 2620, 2492,
    2364, 2236, 2108, 1980, 1884, 1820, 1756, 1692, 1628, 1564, 1500, 1436, 1372,
    1308, 1244, 1180, 1116, 1052, 988, 924, 876, 844, 812, 780, 748, 716, 684, 652,
    620, 588, 556, 524, 492, 460, 428, 396, 372, 356, 340, 324, 308, 292, 276,
    260, 244, 228, 212, 196, 180, 164, 148, 132, 120, 112, 104, 96, 88, 80, 72, 64,
    56, 48, 40, 32, 24, 16, 8, 0,
];