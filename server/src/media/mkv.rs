//! Minimal streaming Matroska writer for live recordings (H.264 or VP8 + Opus).
//!
//! The segment has unknown size and every cluster is written whole, so a file cut
//! off by a crash or power loss stays readable up to its last complete cluster.

use std::io::{self, Write};

pub const TRACK_VIDEO: u64 = 1;
pub const TRACK_AUDIO: u64 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoCodec {
    H264,
    Vp8,
}

pub struct VideoTrack {
    pub codec: VideoCodec,
    pub width: u32,
    pub height: u32,
    /// avcC for H.264, empty for VP8.
    pub codec_private: Vec<u8>,
}

// Element ids.
const EBML: u32 = 0x1A45DFA3;
const SEGMENT: u32 = 0x18538067;
const INFO: u32 = 0x1549A966;
const TRACKS: u32 = 0x1654AE6B;
const TRACK_ENTRY: u32 = 0xAE;
const CLUSTER: u32 = 0x1F43B675;
const TIMESTAMP: u32 = 0xE7;
const SIMPLE_BLOCK: u32 = 0xA3;

pub struct MkvWriter<W: Write> {
    out: W,
    cluster: Vec<u8>,
    cluster_ts: Option<u64>,
    last_ts: [u64; 2],
}

impl<W: Write> MkvWriter<W> {
    /// Writes the header. Timestamps passed later are in milliseconds.
    pub fn new(mut out: W, video: &VideoTrack, audio_channels: u8) -> io::Result<Self> {
        let mut head = Vec::with_capacity(512);
        element(&mut head, EBML, &{
            let mut b = Vec::new();
            uint_el(&mut b, 0x4286, 1); // EBMLVersion
            uint_el(&mut b, 0x42F7, 1); // EBMLReadVersion
            uint_el(&mut b, 0x42F2, 4); // EBMLMaxIDLength
            uint_el(&mut b, 0x42F3, 8); // EBMLMaxSizeLength
            element(&mut b, 0x4282, b"matroska"); // DocType
            uint_el(&mut b, 0x4287, 4); // DocTypeVersion
            uint_el(&mut b, 0x4285, 2); // DocTypeReadVersion
            b
        });
        // Segment with unknown size: lets the file grow while streaming.
        write_id(&mut head, SEGMENT);
        head.extend_from_slice(&[0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]);

        element(&mut head, INFO, &{
            let mut b = Vec::new();
            uint_el(&mut b, 0x2AD7B1, 1_000_000); // TimestampScale: 1 ms
            element(&mut b, 0x4D80, b"SchoolLive"); // MuxingApp
            element(&mut b, 0x5741, b"SchoolLive"); // WritingApp
            b
        });

        let mut tracks = Vec::new();
        element(&mut tracks, TRACK_ENTRY, &{
            let mut b = Vec::new();
            uint_el(&mut b, 0xD7, TRACK_VIDEO); // TrackNumber
            uint_el(&mut b, 0x73C5, TRACK_VIDEO); // TrackUID
            uint_el(&mut b, 0x83, 1); // TrackType: video
            uint_el(&mut b, 0x9C, 0); // FlagLacing
            let codec_id: &[u8] = match video.codec {
                VideoCodec::H264 => b"V_MPEG4/ISO/AVC",
                VideoCodec::Vp8 => b"V_VP8",
            };
            element(&mut b, 0x86, codec_id);
            if !video.codec_private.is_empty() {
                element(&mut b, 0x63A2, &video.codec_private);
            }
            element(&mut b, 0xE0, &{
                let mut v = Vec::new();
                uint_el(&mut v, 0xB0, video.width.max(1) as u64);
                uint_el(&mut v, 0xBA, video.height.max(1) as u64);
                v
            });
            b
        });
        element(&mut tracks, TRACK_ENTRY, &{
            let mut b = Vec::new();
            uint_el(&mut b, 0xD7, TRACK_AUDIO);
            uint_el(&mut b, 0x73C5, TRACK_AUDIO);
            uint_el(&mut b, 0x83, 2); // TrackType: audio
            uint_el(&mut b, 0x9C, 0);
            element(&mut b, 0x86, b"A_OPUS");
            element(&mut b, 0x63A2, &opus_head(audio_channels));
            element(&mut b, 0xE1, &{
                let mut a = Vec::new();
                float_el(&mut a, 0xB5, 48_000.0);
                uint_el(&mut a, 0x9F, audio_channels as u64);
                a
            });
            b
        });
        element(&mut head, TRACKS, &tracks);
        out.write_all(&head)?;
        out.flush()?;
        Ok(MkvWriter { out, cluster: Vec::with_capacity(1 << 20), cluster_ts: None, last_ts: [0; 2] })
    }

    /// Adds a frame. Video keyframes start a new cluster (after at least 1 s), so
    /// clusters line up with keyframes and seeking stays precise.
    pub fn write_frame(&mut self, track: u64, ts_ms: u64, keyframe: bool, data: &[u8]) -> io::Result<()> {
        let idx = (track - 1) as usize;
        let ts = ts_ms.max(self.last_ts[idx]);
        self.last_ts[idx] = ts;

        let start_new = match self.cluster_ts {
            None => true,
            Some(c) => {
                let age = ts.saturating_sub(c);
                (track == TRACK_VIDEO && keyframe && age >= 1000)
                    || age >= 30_000
                    || self.cluster.len() > 16 << 20
            }
        };
        if start_new {
            self.flush_cluster()?;
            self.cluster_ts = Some(ts);
            uint_el(&mut self.cluster, TIMESTAMP, ts);
        }
        let base = self.cluster_ts.unwrap_or(ts);
        let rel = (ts as i64 - base as i64).clamp(i16::MIN as i64, i16::MAX as i64) as i16;

        let mut block = Vec::with_capacity(data.len() + 4);
        block.push(0x80 | track as u8); // track number as 1-byte vint
        block.extend_from_slice(&rel.to_be_bytes());
        block.push(if keyframe || track == TRACK_AUDIO { 0x80 } else { 0x00 });
        block.extend_from_slice(data);
        element(&mut self.cluster, SIMPLE_BLOCK, &block);
        Ok(())
    }

    pub fn flush_cluster(&mut self) -> io::Result<()> {
        if self.cluster.is_empty() {
            return Ok(());
        }
        let mut head = Vec::with_capacity(12);
        write_id(&mut head, CLUSTER);
        write_size(&mut head, self.cluster.len() as u64);
        self.out.write_all(&head)?;
        self.out.write_all(&self.cluster)?;
        self.out.flush()?;
        self.cluster.clear();
        Ok(())
    }

    pub fn finish(mut self) -> io::Result<W> {
        self.flush_cluster()?;
        Ok(self.out)
    }
}

fn opus_head(channels: u8) -> Vec<u8> {
    let mut h = Vec::with_capacity(19);
    h.extend_from_slice(b"OpusHead");
    h.push(1); // version
    h.push(channels);
    h.extend_from_slice(&0u16.to_le_bytes()); // pre-skip
    h.extend_from_slice(&48_000u32.to_le_bytes());
    h.extend_from_slice(&0i16.to_le_bytes()); // output gain
    h.push(0); // mapping family
    h
}

fn write_id(out: &mut Vec<u8>, id: u32) {
    let bytes = id.to_be_bytes();
    let skip = bytes.iter().take_while(|b| **b == 0).count();
    out.extend_from_slice(&bytes[skip..]);
}

fn write_size(out: &mut Vec<u8>, size: u64) {
    // Smallest EBML vint that holds the size (all-ones values are reserved).
    for len in 1..=8u32 {
        let max = (1u64 << (7 * len)) - 1;
        if size < max {
            let marker = 1u64 << (7 * len);
            let v = size | marker;
            let bytes = v.to_be_bytes();
            out.extend_from_slice(&bytes[8 - len as usize..]);
            return;
        }
    }
    panic!("EBML size too large");
}

fn element(out: &mut Vec<u8>, id: u32, payload: &[u8]) {
    write_id(out, id);
    write_size(out, payload.len() as u64);
    out.extend_from_slice(payload);
}

fn uint_el(out: &mut Vec<u8>, id: u32, v: u64) {
    let bytes = v.to_be_bytes();
    let skip = bytes.iter().take_while(|b| **b == 0).count().min(7);
    element(out, id, &bytes[skip..]);
}

fn float_el(out: &mut Vec<u8>, id: u32, v: f64) {
    element(out, id, &v.to_be_bytes());
}

// ---------------------------------------------------------------------------
// H.264 helpers

/// Splits an Annex-B byte stream into NAL units (without start codes).
pub fn annexb_nalus(data: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let mut i = 0;
    let mut start: Option<usize> = None;
    while i + 3 <= data.len() {
        let sc3 = data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1;
        if sc3 {
            if let Some(s) = start {
                let mut end = i;
                // A 4-byte start code leaves one extra zero on the previous unit.
                if end > s && data[end - 1] == 0 {
                    end -= 1;
                }
                if end > s {
                    out.push(&data[s..end]);
                }
            }
            i += 3;
            start = Some(i);
        } else {
            i += 1;
        }
    }
    if let Some(s) = start
        && s < data.len() {
            out.push(&data[s..]);
        }
    out
}

/// Annex-B to length-prefixed (AVCC) as Matroska and MP4 expect.
pub fn annexb_to_avcc(data: &[u8]) -> Vec<u8> {
    let nalus = annexb_nalus(data);
    let mut out = Vec::with_capacity(data.len() + nalus.len() * 4);
    for n in nalus {
        out.extend_from_slice(&(n.len() as u32).to_be_bytes());
        out.extend_from_slice(n);
    }
    out
}

/// Builds an AVCDecoderConfigurationRecord from one SPS and one PPS.
pub fn avcc_record(sps: &[u8], pps: &[u8]) -> Vec<u8> {
    let mut r = Vec::with_capacity(11 + sps.len() + pps.len());
    r.push(1);
    r.push(sps.get(1).copied().unwrap_or(66));
    r.push(sps.get(2).copied().unwrap_or(0));
    r.push(sps.get(3).copied().unwrap_or(31));
    r.push(0xFF); // 4-byte NAL lengths
    r.push(0xE1); // one SPS
    r.extend_from_slice(&(sps.len() as u16).to_be_bytes());
    r.extend_from_slice(sps);
    r.push(1); // one PPS
    r.extend_from_slice(&(pps.len() as u16).to_be_bytes());
    r.extend_from_slice(pps);
    r
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpsInfo {
    pub profile: u8,
    pub constraints: u8,
    pub level: u8,
    pub width: u32,
    pub height: u32,
}

impl SpsInfo {
    /// RFC 6381 codec string, e.g. `avc1.42e01f`.
    pub fn codec_string(&self) -> String {
        format!("avc1.{:02x}{:02x}{:02x}", self.profile, self.constraints, self.level)
    }
}

/// Parses resolution and profile from an SPS NAL unit (including its header byte).
pub fn parse_sps(nal: &[u8]) -> Option<SpsInfo> {
    if nal.len() < 4 || nal[0] & 0x1F != 7 {
        return None;
    }
    let rbsp = unescape(&nal[1..]);
    let mut r = BitReader::new(&rbsp);
    let profile = r.bits(8)? as u8;
    let constraints = r.bits(8)? as u8;
    let level = r.bits(8)? as u8;
    r.ue()?; // seq_parameter_set_id
    let mut chroma_format_idc = 1;
    if matches!(profile, 100 | 110 | 122 | 244 | 44 | 83 | 86 | 118 | 128 | 138 | 139 | 134 | 135) {
        chroma_format_idc = r.ue()?;
        if chroma_format_idc == 3 {
            r.bits(1)?; // separate_colour_plane_flag
        }
        r.ue()?; // bit_depth_luma_minus8
        r.ue()?; // bit_depth_chroma_minus8
        r.bits(1)?; // qpprime_y_zero_transform_bypass_flag
        if r.bits(1)? == 1 {
            let lists = if chroma_format_idc != 3 { 8 } else { 12 };
            for i in 0..lists {
                if r.bits(1)? == 1 {
                    let size = if i < 6 { 16 } else { 64 };
                    let (mut last, mut next) = (8i64, 8i64);
                    for _ in 0..size {
                        if next != 0 {
                            let delta = r.se()?;
                            next = (last + delta + 256) % 256;
                        }
                        if next != 0 {
                            last = next;
                        }
                    }
                }
            }
        }
    }
    r.ue()?; // log2_max_frame_num_minus4
    let poc_type = r.ue()?;
    if poc_type == 0 {
        r.ue()?;
    } else if poc_type == 1 {
        r.bits(1)?;
        r.se()?;
        r.se()?;
        let n = r.ue()?;
        for _ in 0..n {
            r.se()?;
        }
    }
    r.ue()?; // max_num_ref_frames
    r.bits(1)?; // gaps_in_frame_num_value_allowed_flag
    let width_mbs = r.ue()? + 1;
    let height_map_units = r.ue()? + 1;
    let frame_mbs_only = r.bits(1)?;
    if frame_mbs_only == 0 {
        r.bits(1)?;
    }
    r.bits(1)?; // direct_8x8_inference_flag
    let (mut cl, mut cr, mut ct, mut cb) = (0, 0, 0, 0);
    if r.bits(1)? == 1 {
        cl = r.ue()?;
        cr = r.ue()?;
        ct = r.ue()?;
        cb = r.ue()?;
    }
    let (sub_w, sub_h) = match chroma_format_idc {
        0 => (1, 1),
        1 => (2, 2),
        2 => (2, 1),
        _ => (1, 1),
    };
    let crop_x = if chroma_format_idc == 0 { 1 } else { sub_w };
    let crop_y = (if chroma_format_idc == 0 { 1 } else { sub_h }) * (2 - frame_mbs_only);
    let width = width_mbs * 16 - (cl + cr) * crop_x;
    let height = (2 - frame_mbs_only) * height_map_units * 16 - (ct + cb) * crop_y;
    Some(SpsInfo { profile, constraints, level, width, height })
}

/// VP8 keyframe header carries the frame size.
pub fn vp8_size(frame: &[u8]) -> Option<(u32, u32)> {
    if frame.len() < 10 || frame[0] & 1 != 0 || frame[3..6] != [0x9d, 0x01, 0x2a] {
        return None;
    }
    let w = u16::from_le_bytes([frame[6], frame[7]]) & 0x3FFF;
    let h = u16::from_le_bytes([frame[8], frame[9]]) & 0x3FFF;
    Some((w as u32, h as u32))
}

fn unescape(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    let mut zeros = 0;
    for &b in data {
        if zeros >= 2 && b == 3 {
            zeros = 0;
            continue;
        }
        zeros = if b == 0 { zeros + 1 } else { 0 };
        out.push(b);
    }
    out
}

struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        BitReader { data, pos: 0 }
    }

    fn bits(&mut self, n: u32) -> Option<u32> {
        let mut v = 0u32;
        for _ in 0..n {
            let byte = *self.data.get(self.pos / 8)?;
            let bit = (byte >> (7 - (self.pos % 8))) & 1;
            v = (v << 1) | bit as u32;
            self.pos += 1;
        }
        Some(v)
    }

    fn ue(&mut self) -> Option<u32> {
        let mut zeros = 0;
        while self.bits(1)? == 0 {
            zeros += 1;
            if zeros > 31 {
                return None;
            }
        }
        if zeros == 0 {
            return Some(0);
        }
        Some((1u32 << zeros) - 1 + self.bits(zeros)?)
    }

    fn se(&mut self) -> Option<i64> {
        let k = self.ue()? as i64;
        Some(if k % 2 == 1 { (k + 1) / 2 } else { -(k / 2) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Real SPS units produced by `ffmpeg -c:v libx264` (see tests for the expected sizes).
    const SPS_720P_BASELINE: [u8; 25] = [
        0x67, 0x42, 0xc0, 0x1f, 0xd9, 0x00, 0x50, 0x05, 0xbb, 0x01, 0x10, 0x00, 0x00, 0x03, 0x00,
        0x10, 0x00, 0x00, 0x03, 0x03, 0xc0, 0xf1, 0x83, 0x24, 0x80,
    ];
    const SPS_1080P_HIGH: [u8; 27] = [
        0x67, 0x64, 0x00, 0x28, 0xac, 0xd9, 0x40, 0x78, 0x02, 0x27, 0xe5, 0xc0, 0x44, 0x00, 0x00,
        0x03, 0x00, 0x04, 0x00, 0x00, 0x03, 0x00, 0xf0, 0x3c, 0x60, 0xc6, 0x58,
    ];
    const SPS_360P_HIGH: [u8; 26] = [
        0x67, 0x64, 0x00, 0x1e, 0xac, 0xd9, 0x40, 0xa0, 0x2f, 0xf9, 0x70, 0x11, 0x00, 0x00, 0x03,
        0x00, 0x01, 0x00, 0x00, 0x03, 0x00, 0x3c, 0x0f, 0x16, 0x2d, 0x96,
    ];

    #[test]
    fn parses_baseline_sps() {
        let s = parse_sps(&SPS_720P_BASELINE).unwrap();
        assert_eq!((s.width, s.height), (1280, 720));
        assert_eq!(s.codec_string(), "avc1.42c01f");
    }

    #[test]
    fn parses_high_profile_sps_with_cropping() {
        let s = parse_sps(&SPS_1080P_HIGH).unwrap();
        assert_eq!(s.profile, 100);
        assert_eq!((s.width, s.height), (1920, 1080));
        let s = parse_sps(&SPS_360P_HIGH).unwrap();
        assert_eq!((s.width, s.height), (640, 360));
    }

    #[test]
    fn splits_annexb() {
        let data = [0, 0, 0, 1, 0x67, 1, 2, 0, 0, 1, 0x68, 3, 0, 0, 0, 1, 0x65, 9, 9];
        let n = annexb_nalus(&data);
        assert_eq!(n, vec![&[0x67, 1, 2][..], &[0x68, 3][..], &[0x65, 9, 9][..]]);
        let avcc = annexb_to_avcc(&data);
        assert_eq!(&avcc[..7], &[0, 0, 0, 3, 0x67, 1, 2]);
    }

    #[test]
    fn ebml_sizes() {
        let mut v = Vec::new();
        write_size(&mut v, 5);
        assert_eq!(v, [0x85]);
        v.clear();
        write_size(&mut v, 127);
        assert_eq!(v, [0x40, 0x7F]);
        v.clear();
        write_id(&mut v, CLUSTER);
        assert_eq!(v, [0x1F, 0x43, 0xB6, 0x75]);
    }

    #[test]
    fn vp8_keyframe_size() {
        let f = [0x10, 0x02, 0x00, 0x9d, 0x01, 0x2a, 0x80, 0x02, 0xe0, 0x01];
        assert_eq!(vp8_size(&f), Some((640, 480)));
    }
}
