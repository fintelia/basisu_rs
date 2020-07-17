
use std::ops::{
    Index,
    IndexMut,
};
use crate::{
    Color32,
    Image,
    mask,
    Result,
    basis::{
        Header,
        SliceDesc,
        TextureType,
    },
    bitreader::BitReaderLSB,
    huffman::{
        self,
        HuffmanDecodingTable,
    }
};

const MAX_ENDPOINT_COUNT: usize = 18;

pub struct DecodedBlock {
    block_x: u32,
    block_y: u32,
    mode_index: usize,
    trans_flags: TranscodingFlags,
    pat: u8,
    compsel: u8,
    data: ModeData,
}

#[derive(Clone, Copy, Default)]
struct ModeE18W16 {
    endpoints: [u8; 18],
    weights: [u8; 16],
}

#[derive(Clone, Copy, Default)]
struct ModeE8W32 {
    endpoints: [u8; 8],
    weights: [u8; 32],
}

#[derive(Clone, Copy)]
enum ModeData {
    ModeE18W16(ModeE18W16),
    ModeE8W32(ModeE8W32),
    Mode8 {
        r: u8, g: u8, b: u8, a: u8,
        etc1i: u8, etc1s: u8,
        etc1r: u8, etc1g: u8, etc1b: u8,
    },
}

#[derive(Clone, Copy, Debug, Default)]
pub struct TranscodingFlags {
    bc1h0: bool,
    bc1h1: bool,
    etc1f: bool,
    etc1d: bool,
    etc1i0: u8,
    etc1i1: u8,
    etc1bias: u8,
    etc2tm: u8,
}

pub struct Decoder {

}

impl Decoder {
    pub(crate) fn from_file_bytes(header: &Header, bytes: &[u8]) -> Result<Self> {
        // TODO: LUTs
        Ok(Self { })
    }

    pub(crate) fn decode_to_rgba(&self, slice_desc: &SliceDesc, bytes: &[u8]) -> Result<Image<Color32>> {

        let mut image = Image {
            w: 4*slice_desc.num_blocks_x as u32,
            h: 4*slice_desc.num_blocks_y as u32,
            stride: 4*slice_desc.num_blocks_x as u32,
            y_flipped: false,
            data: vec![Color32::default(); slice_desc.num_blocks_x as usize * slice_desc.num_blocks_y as usize * 16],
        };

        let block_to_rgba = |block: DecodedBlock| {
            let rgba = block_to_rgba(&block);
            for y in 0..4 {
                let x_start = 4 * block.block_x as usize;
                let image_start = (4 * block.block_y as usize + y) * image.stride as usize + x_start;
                image.data[image_start..image_start + 4].copy_from_slice(&rgba[4 * y..4 * y + 4]);
            }
        };

        self.decode_blocks(slice_desc, bytes, block_to_rgba)?;

        Ok(image)
    }

    pub(crate) fn decode_blocks<F>(&self, slice_desc: &SliceDesc, bytes: &[u8], mut f: F) -> Result<()>
        where F: FnMut(DecodedBlock)
    {
        let num_blocks_x = slice_desc.num_blocks_x as u32;
        let num_blocks_y = slice_desc.num_blocks_y as u32;

        let bytes = {
            let start = slice_desc.file_ofs as usize;
            let len = slice_desc.file_size as usize;
            &bytes[start..start+len]
        };

        let mut block_offset = 0;

        const BLOCK_SIZE: usize = 16;

        if bytes.len() < BLOCK_SIZE * num_blocks_x as usize * num_blocks_y as usize {
            return Err("Not enough bytes for all blocks".into());
        }

        for block_y in 0..num_blocks_y {
            for block_x in 0..num_blocks_x {
                let block = decode_block(block_x, block_y, &bytes[block_offset..block_offset + BLOCK_SIZE]);
                f(block);
                block_offset += BLOCK_SIZE;
            }
        }

        Ok(())
    }
}

fn block_to_rgba(block: &DecodedBlock) -> [Color32; 16] {
    match block.data {
        ModeData::Mode8 { r, g, b, a, .. } => {
            let color = Color32::new(r, g, b, a);
            return [color; 16];
        }
        _ => {
            return [Color32::default(); 16];
        },

    }
}

fn decode_block(block_x: u32, block_y: u32, bytes: &[u8]) -> DecodedBlock {

    let reader = &mut BitReaderLSB::new(bytes);

    let mode_code = reader.peek(7) as usize;
    let mode_index = MODE_LUT[mode_code] as usize;
    let mode = MODES[mode_index];
    let endpoint_count = mode.endpoint_count as usize;

    reader.remove(mode.code_size as usize);

    let mut trans_flags = decode_trans_flags(reader, mode_index);

    let compsel = match mode_index {
        6 | 11 | 13 => {
            reader.read_u8(2)
        }
        _ => 0
    };

    let pat = match mode_index {
        3 => {
            reader.read_u8(4)
        }
        2 | 4 | 7 | 9 | 16 => {
            reader.read_u8(5)
        }
        _ => 0
    };

    let data = if mode_index == 8 {
        let r = reader.read_u8(8);
        let g = reader.read_u8(8);
        let b = reader.read_u8(8);
        let a = reader.read_u8(8);
        trans_flags.etc1d = reader.read_bool();
        let etc1i = reader.read_u8(3);
        let etc1s = reader.read_u8(2);
        let etc1r = reader.read_u8(5);
        let etc1g = reader.read_u8(5);
        let etc1b = reader.read_u8(5);
        ModeData::Mode8 {
            r, g, b, a,
            etc1i, etc1s,
            etc1r, etc1g, etc1b,
        }
    } else {
        let mut result = match mode.plane_count {
            1 => ModeData::ModeE18W16(ModeE18W16::default()),
            2 => ModeData::ModeE8W32(ModeE8W32::default()),
            _ => unreachable!()
        };

        let (endpoints, weights) = match result {
            ModeData::ModeE18W16(ref mut d) => (&mut d.endpoints[..], &mut d.weights[..]),
            ModeData::ModeE8W32(ref mut d) => (&mut d.endpoints[..], &mut d.weights[..]),
            _ => unreachable!()
        };

        let quant_endpoints = decode_endpoints(reader, mode.endpoint_range_index, endpoint_count);
        for (i, quant) in quant_endpoints.iter().take(endpoint_count).enumerate() {
            endpoints[i] = unquant_endpoint(*quant, mode.endpoint_range_index);
        }
        let plane_count = mode.plane_count as usize;
        decode_weights(reader, mode.weight_bits, plane_count, weights);
        unquant_weights(weights, mode.weight_bits);
        result
    };

    DecodedBlock {
        block_x, block_y, mode_index, trans_flags, compsel, pat, data,
    }
}

fn decode_trans_flags(reader: &mut BitReaderLSB, mode_index: usize) -> TranscodingFlags {
    let mut flags = TranscodingFlags::default();
    if mode_index == 8 {
        return flags; // Mode 8 has a different field order, flags will be read into this struct later
    }
    flags.bc1h0 = reader.read_bool();
    if mode_index < 10 || mode_index > 12 {
        flags.bc1h1 = reader.read_bool();
    }
    flags.etc1f = reader.read_bool();
    flags.etc1d = reader.read_bool();
    flags.etc1i0 = reader.read_u8(3);
    flags.etc1i1 = reader.read_u8(3);
    if mode_index < 10 || mode_index > 12 {
        flags.etc1bias = reader.read_u8(5);
    }
    if mode_index >= 9 && mode_index <= 17 {
        flags.etc2tm = reader.read_u8(8);
    }
    flags
}

#[derive(Clone, Copy, Default)]
pub struct Mode {
    code: u8,
    code_size: u8,
    block_size: u8,
    endpoint_range_index: u8,
    endpoint_count: u8,
    weight_bits: u8,
    plane_count: u8,
}

static MODES: [Mode; 20] = [
    Mode { code: 0x01, code_size: 4, block_size: 128, endpoint_range_index: 19, endpoint_count:  6, weight_bits: 4, plane_count: 1 }, //  0
    Mode { code: 0x35, code_size: 6, block_size: 100, endpoint_range_index: 20, endpoint_count:  6, weight_bits: 2, plane_count: 1 }, //  1
    Mode { code: 0x1D, code_size: 5, block_size: 119, endpoint_range_index:  8, endpoint_count: 12, weight_bits: 3, plane_count: 1 }, //  2
    Mode { code: 0x03, code_size: 5, block_size: 118, endpoint_range_index:  7, endpoint_count: 18, weight_bits: 2, plane_count: 1 }, //  3
    Mode { code: 0x13, code_size: 5, block_size: 119, endpoint_range_index: 12, endpoint_count: 12, weight_bits: 2, plane_count: 1 }, //  4
    Mode { code: 0x0B, code_size: 5, block_size: 115, endpoint_range_index: 20, endpoint_count:  6, weight_bits: 3, plane_count: 1 }, //  5
    Mode { code: 0x1B, code_size: 5, block_size: 128, endpoint_range_index: 18, endpoint_count:  6, weight_bits: 2, plane_count: 2 }, //  6
    Mode { code: 0x07, code_size: 5, block_size: 119, endpoint_range_index: 12, endpoint_count: 12, weight_bits: 2, plane_count: 1 }, //  7

    Mode { code: 0x17, code_size: 5, block_size:  58, endpoint_range_index:  0, endpoint_count:  0, weight_bits: 0, plane_count: 0 }, //  8

    Mode { code: 0x0F, code_size: 5, block_size: 127, endpoint_range_index:  8, endpoint_count: 16, weight_bits: 2, plane_count: 1 }, //  9
    Mode { code: 0x02, code_size: 3, block_size: 128, endpoint_range_index: 13, endpoint_count:  8, weight_bits: 4, plane_count: 1 }, // 10
    Mode { code: 0x00, code_size: 2, block_size: 128, endpoint_range_index: 13, endpoint_count:  8, weight_bits: 2, plane_count: 2 }, // 11
    Mode { code: 0x06, code_size: 3, block_size: 128, endpoint_range_index: 19, endpoint_count:  8, weight_bits: 3, plane_count: 1 }, // 12
    Mode { code: 0x1F, code_size: 5, block_size: 124, endpoint_range_index: 20, endpoint_count:  8, weight_bits: 1, plane_count: 2 }, // 13
    Mode { code: 0x0D, code_size: 5, block_size: 123, endpoint_range_index: 20, endpoint_count:  8, weight_bits: 2, plane_count: 1 }, // 14

    Mode { code: 0x05, code_size: 7, block_size: 125, endpoint_range_index: 20, endpoint_count:  4, weight_bits: 4, plane_count: 1 }, // 15
    Mode { code: 0x15, code_size: 6, block_size: 128, endpoint_range_index: 20, endpoint_count:  8, weight_bits: 2, plane_count: 1 }, // 16
    Mode { code: 0x25, code_size: 6, block_size: 123, endpoint_range_index: 20, endpoint_count:  4, weight_bits: 2, plane_count: 2 }, // 17

    Mode { code: 0x09, code_size: 4, block_size: 128, endpoint_range_index: 11, endpoint_count:  6, weight_bits: 5, plane_count: 1 }, // 18

    Mode { code: 0x45, code_size: 7, block_size:   0, endpoint_range_index:  0, endpoint_count:  0, weight_bits: 0, plane_count: 0 }, // 19 reserved
];

static MODE_LUT: [u8; 128] = [
    11,  0, 10, 3, 11, 15, 12,  7,
    11, 18, 10, 5, 11, 14, 12,  9,
    11,  0, 10, 4, 11, 16, 12,  8,
    11, 18, 10, 6, 11,  2, 12, 13,
    11,  0, 10, 3, 11, 17, 12,  7,
    11, 18, 10, 5, 11, 14, 12,  9,
    11,  0, 10, 4, 11,  1, 12,  8,
    11, 18, 10, 6, 11,  2, 12, 13,
    11,  0, 10, 3, 11, 19, 12,  7,
    11, 18, 10, 5, 11, 14, 12,  9,
    11,  0, 10, 4, 11, 16, 12,  8,
    11, 18, 10, 6, 11,  2, 12, 13,
    11,  0, 10, 3, 11, 17, 12,  7,
    11, 18, 10, 5, 11, 14, 12,  9,
    11,  0, 10, 4, 11,  1, 12,  8,
    11, 18, 10, 6, 11,  2, 12, 13,
];

#[derive(Clone, Copy, Default)]
struct QuantEndpoint {
    trit_quint: u8,
    bits: u8,
}

fn unquant_endpoint(quant: QuantEndpoint, range_index: u8) -> u8 {
    let range = BISE_RANGES[range_index as usize];
    let quant_bits = quant.bits as u16;
    if range.trits == 0 && range.quints == 0 && range.bits > 0 {
        // Left align bits
        let mut bits_la = quant_bits << (8 - range.bits);
        let mut val: u16 = 0;
        // Repeat bits into val
        while bits_la > 0 {
            val |= bits_la;
            bits_la >>= range.bits;
        }
        val as u8
    } else {
        let a = if quant_bits & 1 != 0 { 511 } else { 0 };
        let mut b: u16 = 0;
        for j in 0..9 {
            b <<= 1;
            let shift = range.deq_b[j];
            if shift != '0' as u8 {
                b |= (quant_bits >> (shift - 'a' as u8)) & 0x1;
            }
        }
        let c = range.deq_c as u16;
        let d = quant.trit_quint as u16;
        let mut val = d * c + b;
        val = val ^ a;
        (a & 0x80 | val >> 2) as u8
    }
}

fn decode_endpoints(reader: &mut BitReaderLSB, range_index: u8, value_count: usize) -> [QuantEndpoint; MAX_ENDPOINT_COUNT] {
    assert!(value_count <= MAX_ENDPOINT_COUNT);

    let mut output = [QuantEndpoint::default(); MAX_ENDPOINT_COUNT];

    let range = BISE_RANGES[range_index as usize];

    let bit_count = range.bits;

    if range.quints > 0 {
        const QUINTS_PER_GROUP: usize = 3;
        const BITS_PER_GROUP: usize = 7;
        let mut out_pos = 0;
        for _ in 0..(value_count / QUINTS_PER_GROUP) as usize {
            let mut quints = reader.read_u8(BITS_PER_GROUP);
            for _ in 0..QUINTS_PER_GROUP {
                output[out_pos as usize].trit_quint = quints % 5;
                quints /= 5;
                out_pos += 1;
            }
        }
        let remaining = value_count - out_pos;
        if remaining > 0 {
            let bits_used = match remaining {
                1 => 3,
                2 => 5,
                _ => unreachable!(),
            };
            let mut quints = reader.read_u8(bits_used);
            for _ in 0..remaining {
                output[out_pos as usize].trit_quint = quints % 5;
                quints /= 5;
                out_pos += 1;
            }
        }
    }

    if range.trits > 0 {
        const TRITS_PER_GROUP: usize = 5;
        const BITS_PER_GROUP: usize = 8;
        let mut out_pos = 0;
        for _ in 0..(value_count / TRITS_PER_GROUP) as usize {
            let mut trits = reader.read_u8(BITS_PER_GROUP);
            for _ in 0..TRITS_PER_GROUP {
                output[out_pos as usize].trit_quint = trits % 3;
                trits /= 3;
                out_pos += 1;
            }
        }
        let remaining = value_count - out_pos;
        if remaining > 0 {
            let bits_used = match remaining {
                1 => 2,
                2 => 4,
                3 => 5,
                4 => 7,
                _ => unreachable!(),
            };
            let mut trits = reader.read_u8(bits_used);
            for _ in 0..remaining {
                output[out_pos as usize].trit_quint = trits % 3;
                trits /= 3;
                out_pos += 1;
            }
        }
    }

    if bit_count > 0 {
        for i in 0..value_count {
            let bits = reader.read_u8(bit_count as usize);
            output[i].bits = bits;
        }
    }

    output
}

fn unquant_weights(weights: &mut [u8], weight_bits: u8) {
    const LUT1: [u8; 2] = [ 0, 64 ];
    const LUT2: [u8; 4] = [ 0, 21, 43, 64 ];
    const LUT3: [u8; 8] = [ 0, 9, 18, 27, 37, 46, 55, 64 ];
    const LUT4: [u8; 16] = [ 0, 4, 8, 12, 17, 21, 25, 29, 35, 39, 43, 47, 52, 56, 60, 64 ];
    const LUT5: [u8; 32] = [ 0, 2, 4, 6, 8, 10, 12, 14, 16, 18, 20, 22, 24, 26, 28, 30, 34, 36, 38, 40, 42, 44, 46, 48, 50, 52, 54, 56, 58, 60, 62, 64 ];

    let lut = match weight_bits {
        1 => &LUT1[..],
        2 => &LUT2[..],
        3 => &LUT3[..],
        4 => &LUT4[..],
        5 => &LUT5[..],
        _ => unreachable!()
    };

    for weight in weights {
        *weight = lut[*weight as usize];
    }
}

fn decode_weights(reader: &mut BitReaderLSB, weight_bits: u8, plane_count: usize, output: &mut [u8]) {
    for plane in 0..plane_count {
        // First weight of each subset is encoded with one less bit (MSB = 0)
        let pos = 16 * plane;
        output[pos] = reader.read_u8((weight_bits-1) as usize);
        for i in 1..15 {
            output[pos + i] = reader.read_u8(weight_bits as usize);
        }
    }
}

#[derive(Clone, Copy)]
struct BiseCounts {
    bits: u8,
    trits: u8,
    quints: u8,
    max: u8,
    deq_b: &'static[u8; 9],
    deq_c: u8,
}

static BISE_RANGES: [BiseCounts; 21] = [
    BiseCounts { bits: 1, trits: 0, quints: 0, max:   1, deq_b: b"         ", deq_c:   0 }, //  0
    BiseCounts { bits: 0, trits: 1, quints: 0, max:   2, deq_b: b"         ", deq_c:   0 }, //  1
    BiseCounts { bits: 2, trits: 0, quints: 0, max:   3, deq_b: b"         ", deq_c:   0 }, //  2
    BiseCounts { bits: 0, trits: 0, quints: 1, max:   4, deq_b: b"         ", deq_c:   0 }, //  3
    BiseCounts { bits: 1, trits: 1, quints: 0, max:   5, deq_b: b"000000000", deq_c: 204 }, //  4
    BiseCounts { bits: 3, trits: 0, quints: 0, max:   7, deq_b: b"         ", deq_c:   0 }, //  5
    BiseCounts { bits: 1, trits: 0, quints: 1, max:   9, deq_b: b"000000000", deq_c: 113 }, //  6
    BiseCounts { bits: 2, trits: 1, quints: 0, max:  11, deq_b: b"b000b0bb0", deq_c:  93 }, //  7
    BiseCounts { bits: 4, trits: 0, quints: 0, max:  15, deq_b: b"         ", deq_c:   0 }, //  8
    BiseCounts { bits: 2, trits: 0, quints: 1, max:  19, deq_b: b"b0000bb00", deq_c:  54 }, //  9
    BiseCounts { bits: 3, trits: 1, quints: 0, max:  23, deq_b: b"cb000cbcb", deq_c:  44 }, // 10
    BiseCounts { bits: 5, trits: 0, quints: 0, max:  31, deq_b: b"         ", deq_c:   0 }, // 11
    BiseCounts { bits: 3, trits: 0, quints: 1, max:  39, deq_b: b"cb0000cbc", deq_c:  26 }, // 12
    BiseCounts { bits: 4, trits: 1, quints: 0, max:  47, deq_b: b"dcb000dcb", deq_c:  22 }, // 13
    BiseCounts { bits: 6, trits: 0, quints: 0, max:  63, deq_b: b"         ", deq_c:   0 }, // 14
    BiseCounts { bits: 4, trits: 0, quints: 1, max:  79, deq_b: b"dcb0000dc", deq_c:  13 }, // 15
    BiseCounts { bits: 5, trits: 1, quints: 0, max:  95, deq_b: b"edcb000ed", deq_c:  11 }, // 16
    BiseCounts { bits: 7, trits: 0, quints: 0, max: 127, deq_b: b"         ", deq_c:   0 }, // 17
    BiseCounts { bits: 5, trits: 0, quints: 1, max: 159, deq_b: b"edcb0000e", deq_c:   6 }, // 18
    BiseCounts { bits: 6, trits: 1, quints: 0, max: 191, deq_b: b"fedcb000f", deq_c:   5 }, // 19
    BiseCounts { bits: 8, trits: 0, quints: 0, max: 255, deq_b: b"         ", deq_c:   0 }, // 20
];

#[cfg(test)]
mod tests {

    use super::*;

    #[test]
    fn test_uastc() -> Result<()> {
        for block in TEST_BLOCK_DATA.iter() {
            let decoded_block = decode_block(0, 0, block);
        }
        Ok(())
    }

    static TEST_BLOCK_DATA: [[u8; 16]; 64] = [
        [ 0x08, 0x18, 0x43, 0x67, 0x57, 0x4F, 0xB0, 0x8A, 0x5E, 0xB4, 0x62, 0x35, 0x42, 0xE2, 0x0E, 0x22 ],
        [ 0xF1, 0xF0, 0x18, 0x8F, 0xE4, 0x6B, 0x3C, 0x3A, 0x90, 0xFB, 0x89, 0x5D, 0x56, 0x82, 0x9B, 0x6C ],
        [ 0x84, 0x9F, 0x81, 0x8D, 0xF5, 0xD3, 0x64, 0x04, 0x5B, 0xBB, 0x43, 0x87, 0x31, 0xAF, 0xC1, 0x3D ],
        [ 0x0E, 0xB7, 0xC7, 0xFB, 0x50, 0x02, 0x79, 0x69, 0xDE, 0x93, 0xE2, 0x3F, 0xB5, 0x1B, 0x38, 0xEE ],
        [ 0x71, 0x9E, 0xB0, 0x2F, 0xA7, 0x6D, 0x26, 0xAC, 0x12, 0xC0, 0xB8, 0xA2, 0xB5, 0xCA, 0x11, 0x48 ],
        [ 0x0D, 0x6A, 0x1E, 0x11, 0x35, 0x44, 0xB2, 0xE8, 0x5B, 0xDB, 0xD3, 0xB5, 0x4E, 0x00, 0x0D, 0xB9 ],
        [ 0xCC, 0x6A, 0x83, 0x46, 0x83, 0xD4, 0xCF, 0x8A, 0xD7, 0xF6, 0xEF, 0xBC, 0x83, 0x69, 0xB0, 0xB4 ],
        [ 0x2B, 0x24, 0x05, 0x47, 0x26, 0xD6, 0x5E, 0xE2, 0xAA, 0x54, 0x5F, 0x72, 0xED, 0x77, 0x4C, 0x21 ],
        [ 0x3A, 0x66, 0xAA, 0x96, 0x00, 0xFA, 0xB7, 0xE2, 0x93, 0x35, 0xC4, 0xBE, 0x32, 0xC4, 0xA3, 0x97 ],
        [ 0xFD, 0x09, 0xB0, 0xF0, 0x15, 0x99, 0xB6, 0x06, 0xA5, 0x66, 0x7A, 0x74, 0xBA, 0x27, 0x6B, 0xDE ],
        [ 0x33, 0x1E, 0x79, 0x42, 0xF3, 0x98, 0xB7, 0x11, 0x2D, 0xFF, 0xE5, 0x59, 0xAD, 0x23, 0x10, 0x0C ],
        [ 0x56, 0xD2, 0x6D, 0xEC, 0x43, 0xDC, 0xDF, 0x14, 0x8A, 0x08, 0xD9, 0xC8, 0x9E, 0x8C, 0xF5, 0x92 ],
        [ 0x5D, 0x77, 0x5C, 0x2C, 0xF5, 0x39, 0x7F, 0x00, 0x49, 0xE8, 0xB6, 0xDC, 0x42, 0x90, 0x85, 0xB3 ],
        [ 0x5A, 0x22, 0xC3, 0x5E, 0xCE, 0xB6, 0x4D, 0xD4, 0x81, 0x0E, 0x73, 0x21, 0x94, 0xA8, 0x18, 0xDD ],
        [ 0xBA, 0x1C, 0xD2, 0xD2, 0x87, 0xDE, 0x02, 0x3F, 0x71, 0x82, 0xD7, 0x91, 0xED, 0xDA, 0xFA, 0xDD ],
        [ 0xE0, 0x4A, 0xB5, 0x1E, 0x43, 0x9C, 0xA5, 0x52, 0xE6, 0x91, 0x8A, 0x8D, 0x51, 0x75, 0x19, 0xC2 ],
        [ 0x8F, 0x6C, 0x7A, 0x0B, 0x65, 0xEC, 0x26, 0x74, 0x16, 0x37, 0x39, 0x57, 0xB3, 0xB0, 0xF2, 0xFD ],
        [ 0xFA, 0xB5, 0x91, 0xC1, 0x2A, 0x5D, 0xAC, 0xFF, 0x90, 0x33, 0xB6, 0xB7, 0x53, 0xF6, 0x64, 0x2A ],
        [ 0xBD, 0xF8, 0xF6, 0x25, 0x70, 0xD0, 0xF5, 0x71, 0x63, 0x02, 0x3E, 0x63, 0xD1, 0x10, 0x46, 0x35 ],
        [ 0xE2, 0xE0, 0xAF, 0xB0, 0x89, 0xB1, 0x35, 0x6F, 0xA2, 0x83, 0x66, 0x8B, 0x31, 0x1D, 0x24, 0x5D ],
        [ 0xC3, 0x9A, 0x71, 0x34, 0xA4, 0xC9, 0x9F, 0x55, 0xF6, 0x5C, 0x73, 0x7E, 0x44, 0xD3, 0x77, 0x47 ],
        [ 0x3D, 0x5D, 0xF2, 0x24, 0x7B, 0x2A, 0x0D, 0x30, 0xD2, 0x20, 0x26, 0x28, 0xF1, 0xB5, 0x5D, 0x8E ],
        [ 0x26, 0x74, 0xD6, 0xA5, 0x80, 0x68, 0xBB, 0x68, 0x65, 0x29, 0xAC, 0x93, 0xCC, 0x42, 0x8D, 0xF8 ],
        [ 0x57, 0x41, 0x44, 0xB6, 0x80, 0x57, 0x53, 0x0A, 0xE9, 0x41, 0x82, 0xD7, 0xCC, 0x27, 0xF6, 0x24 ],
        [ 0x23, 0x63, 0x1A, 0x93, 0x84, 0x27, 0xD7, 0x4A, 0xCF, 0xDC, 0x34, 0xAC, 0x8D, 0xA1, 0x8F, 0x91 ],
        [ 0x58, 0x97, 0xEA, 0x3B, 0xF8, 0x08, 0xA8, 0x4D, 0x2F, 0xF7, 0xAD, 0x19, 0x70, 0x9F, 0x0A, 0x26 ],
        [ 0xD0, 0xD7, 0xD4, 0x65, 0x3C, 0x91, 0x0F, 0x55, 0x75, 0x93, 0x94, 0x83, 0xF6, 0x65, 0xCA, 0xBB ],
        [ 0x0B, 0x70, 0x30, 0x86, 0xDA, 0x91, 0x69, 0xCC, 0xBA, 0xC8, 0xFB, 0x00, 0xB3, 0x79, 0x7B, 0x9E ],
        [ 0x6E, 0xD2, 0x1B, 0xEA, 0x46, 0xB5, 0xF4, 0x65, 0xA7, 0xB3, 0xE0, 0x2D, 0x27, 0x58, 0xB2, 0xAE ],
        [ 0x3C, 0xD0, 0xC5, 0x50, 0x79, 0x22, 0xD1, 0xD6, 0x10, 0x2F, 0x7F, 0x83, 0x77, 0x73, 0xCC, 0xD9 ],
        [ 0x05, 0xC2, 0x40, 0x2D, 0xBB, 0x89, 0xFC, 0x8D, 0x43, 0x7E, 0x4F, 0xF6, 0x52, 0x54, 0x4F, 0x8D ],
        [ 0x4B, 0x31, 0x58, 0xE5, 0x51, 0xD4, 0x27, 0x7C, 0x26, 0xA5, 0x58, 0x93, 0x9B, 0x15, 0x00, 0x49 ],
        [ 0xED, 0x81, 0x24, 0xB3, 0x21, 0x60, 0x56, 0xF9, 0x9A, 0x79, 0xFB, 0x56, 0xDC, 0x86, 0xDE, 0x83 ],
        [ 0x1C, 0x23, 0x9F, 0x13, 0xF7, 0xF2, 0xB4, 0xA9, 0x7A, 0xCE, 0x32, 0x2E, 0x47, 0x31, 0xB2, 0xE8 ],
        [ 0x4D, 0xBA, 0x4C, 0x63, 0x02, 0x79, 0xEB, 0x3D, 0x3C, 0x92, 0x23, 0x82, 0x37, 0x60, 0x3F, 0x46 ],
        [ 0x99, 0x7E, 0xDF, 0x6B, 0xB8, 0xBA, 0x52, 0x4C, 0xBC, 0x11, 0x21, 0xC2, 0xEB, 0xDE, 0x12, 0xE6 ],
        [ 0xEC, 0x42, 0xA4, 0xE5, 0x71, 0x07, 0xE5, 0x64, 0xF0, 0xBE, 0x81, 0xFF, 0xD5, 0x4D, 0xB9, 0x9E ],
        [ 0x5C, 0x90, 0xD1, 0x60, 0x14, 0xA3, 0x52, 0x17, 0xAA, 0xE6, 0x63, 0x07, 0x4D, 0x2F, 0x12, 0x2B ],
        [ 0x57, 0x5C, 0xFB, 0xD8, 0xD5, 0x01, 0xEB, 0xFA, 0xDE, 0xDC, 0x5B, 0x29, 0x96, 0x4D, 0xB0, 0x59 ],
        [ 0x6F, 0xBB, 0x89, 0xF2, 0xC5, 0x32, 0x3E, 0xAA, 0xC7, 0x31, 0x0E, 0x5A, 0x01, 0x87, 0x3F, 0x78 ],
        [ 0x2F, 0x71, 0x68, 0xF2, 0xB9, 0xF9, 0x1A, 0x52, 0x90, 0x33, 0xEB, 0x57, 0x0A, 0xC4, 0xA7, 0xEF ],
        [ 0x62, 0x80, 0x01, 0xD3, 0x84, 0x3A, 0xA9, 0x06, 0x25, 0x54, 0xD0, 0xCC, 0x25, 0x29, 0xFB, 0xA5 ],
        [ 0xC3, 0xAF, 0xA0, 0x20, 0xE8, 0x96, 0xE8, 0x1C, 0x38, 0x56, 0xA6, 0x81, 0xB1, 0xDD, 0x5B, 0xC7 ],
        [ 0x6D, 0x2C, 0xFB, 0xB8, 0x0E, 0x05, 0x60, 0xDF, 0xB8, 0x1B, 0xE1, 0x5E, 0x0E, 0xAA, 0x5C, 0xF5 ],
        [ 0x71, 0x57, 0x7C, 0x1D, 0x51, 0xB2, 0xE6, 0xEE, 0xC5, 0x07, 0x04, 0xD7, 0xC2, 0x13, 0x56, 0x52 ],
        [ 0x62, 0xB4, 0x53, 0x4C, 0x8B, 0x25, 0x4F, 0x7B, 0x89, 0xF1, 0x0B, 0x1E, 0xC5, 0xC1, 0x33, 0x9A ],
        [ 0xB7, 0x73, 0x25, 0xF5, 0xF0, 0xD9, 0x24, 0x27, 0x52, 0x83, 0x21, 0xA5, 0x29, 0x35, 0x06, 0x70 ],
        [ 0x0E, 0x7E, 0x12, 0x12, 0x7F, 0x5E, 0x93, 0x54, 0xC9, 0x0F, 0xEC, 0x4D, 0x19, 0x2F, 0xA5, 0x39 ],
        [ 0x1A, 0x6E, 0xED, 0xBB, 0x35, 0xAE, 0x2F, 0x56, 0x71, 0x2D, 0xCE, 0xBC, 0xAC, 0x88, 0x5D, 0x87 ],
        [ 0xB5, 0x24, 0xC0, 0xE7, 0xA2, 0x04, 0x8C, 0xA8, 0x22, 0x5D, 0x7B, 0xE0, 0x0F, 0xD2, 0x8C, 0x55 ],
        [ 0x97, 0x43, 0x6F, 0x3C, 0x4F, 0x89, 0x3F, 0xB7, 0x48, 0x90, 0xDB, 0x7F, 0x9A, 0xF6, 0x1B, 0x58 ],
        [ 0xE8, 0xA2, 0x97, 0xFB, 0x3A, 0xCA, 0xFB, 0x8C, 0xAC, 0xB6, 0xD3, 0x87, 0x64, 0xB3, 0x52, 0xA9 ],
        [ 0x1D, 0xFD, 0x4B, 0x46, 0xDF, 0x8E, 0x09, 0x4E, 0x96, 0x9B, 0x37, 0x82, 0x6F, 0x14, 0x7F, 0xE3 ],
        [ 0xAE, 0x3A, 0x07, 0x3C, 0x80, 0xC1, 0xC2, 0x14, 0xC0, 0x4E, 0x43, 0x88, 0x63, 0x8C, 0x7B, 0x5F ],
        [ 0x94, 0x25, 0xA8, 0x52, 0x1B, 0x8B, 0x47, 0x24, 0x2F, 0xBE, 0x66, 0x93, 0xDB, 0x26, 0xA3, 0x81 ],
        [ 0x5F, 0x2E, 0xE2, 0x34, 0x73, 0x5C, 0x8D, 0xF0, 0x96, 0xE9, 0xE7, 0xD7, 0x53, 0x4B, 0xF2, 0x0B ],
        [ 0x1F, 0xA6, 0x31, 0x90, 0xE6, 0x2C, 0xF8, 0x7B, 0x23, 0x3A, 0x94, 0xE0, 0xA5, 0xD1, 0x3F, 0x03 ],
        [ 0x40, 0xC6, 0x80, 0x68, 0xBC, 0x90, 0xC7, 0xFE, 0xF6, 0x58, 0x3D, 0xEB, 0x59, 0x2A, 0x42, 0x3B ],
        [ 0xD0, 0x7C, 0x96, 0x1B, 0x35, 0x44, 0xA1, 0x5F, 0x55, 0x45, 0x64, 0x74, 0xDC, 0x9B, 0x24, 0x42 ],
        [ 0x4E, 0x47, 0x01, 0x69, 0x3F, 0x3B, 0xF3, 0x6F, 0xBB, 0x43, 0xDA, 0x5F, 0x16, 0x9A, 0xDC, 0x38 ],
        [ 0x2F, 0xB5, 0xB1, 0x51, 0x5D, 0x71, 0xCB, 0xFC, 0x36, 0x66, 0xBF, 0x19, 0x79, 0xF3, 0xA3, 0x7E ],
        [ 0xC1, 0xF5, 0xFA, 0xFC, 0x12, 0x1A, 0x6C, 0xEB, 0xF6, 0x47, 0xEE, 0x1F, 0x75, 0x0C, 0x56, 0x23 ],
        [ 0xB8, 0xF8, 0x33, 0x54, 0xEA, 0xCE, 0x7B, 0x02, 0x6C, 0xD7, 0x15, 0xA0, 0xD0, 0x40, 0x4E, 0x91 ],
        [ 0xFB, 0x31, 0xEB, 0x91, 0x34, 0xFF, 0x84, 0x1F, 0x31, 0x99, 0x3D, 0xD0, 0x67, 0x81, 0x6D, 0x0D ],
    ];

    static TEST_RGBA_DATA: [[u32; 16]; 64] = [
        [ 0x255CF1C4, 0x255CF1C4, 0x1061EFC4, 0x5151F4AE, 0x3B56F3D9, 0x3B56F3C4, 0x255CF1C4, 0x5151F4D9, 0x3B56F3D9, 0x1061EFC4, 0x3B56F3D9, 0x3B56F398, 0x3B56F398, 0x1061EFD9, 0x3B56F3D9, 0x3B56F3D9 ],
        [ 0xFFDD964A, 0xFFEA8066, 0xFFED7C6C, 0xFFF27278, 0xFFEA8066, 0xFFE98263, 0xFFF07672, 0xFFE48A59, 0xFFE6885C, 0xFFE48A59, 0xFFE09250, 0xFFE98263, 0xFFED7C6C, 0xFFEA8066, 0xFFEE796F, 0xFFE6885C ],
        [ 0xFAB92099, 0xCAD06299, 0x67FFEA93, 0x67FFEA93, 0x67FFEA9E, 0xFAB92099, 0x67FFEA99, 0xFAB92093, 0xCAD0629E, 0x67FFEA9E, 0x67FFEA8E, 0x97E8A893, 0xCAD0629E, 0xFAB9208E, 0xCAD0628E, 0x67FFEA9E ],
        [ 0x3A6C2648, 0x2F726628, 0x2578A30A, 0x2578A30A, 0x33704F33, 0x376E3B3D, 0x2C747A1E, 0x2C747A1E, 0x33704F33, 0x33704F33, 0x3E6A1252, 0x2F726628, 0x33704F33, 0x2F726628, 0x33704F33, 0x2578A30A ],
        [ 0xFFC1678D, 0xFFC1678D, 0xFFBF6C8F, 0xFFD22770, 0xFFCC3E7A, 0xFFD02E73, 0xFFC2618A, 0xFFCF3375, 0xFFC75083, 0xFFD02E73, 0xFFCF3375, 0xFFD22770, 0xFFC1678D, 0xFFC1678D, 0xFFCC3E7A, 0xFFC55685 ],
        [ 0x47BB464C, 0x47BB464C, 0x47BB464C, 0x5DB58B43, 0x47BB464C, 0x52B86947, 0x3DBE2451, 0x3DBE2451, 0x3DBE2451, 0x3DBE2451, 0x52B86947, 0x52B86947, 0x47BB464C, 0x3DBE2451, 0x52B86947, 0x3DBE2451 ],
        [ 0x3BD6BB05, 0x3BD6BB1B, 0x5CBCA10C, 0x7CA3881B, 0x7CA3881B, 0x5CBCA11B, 0x1BEFD41B, 0x7CA38813, 0x7CA38805, 0x1BEFD413, 0x3BD6BB13, 0x5CBCA10C, 0x1BEFD405, 0x7CA38813, 0x1BEFD40C, 0x7CA38813 ],
        [ 0xFF4B896D, 0xFF4B896D, 0xFF4B896D, 0xFF87C667, 0xFF9BDA66, 0xFFAEED64, 0xFF4B896D, 0xFF4B896D, 0xFF9BDA66, 0xFF87C667, 0xFF9BDA66, 0xFF9BDA66, 0xFFAEED64, 0xFF87C667, 0xFF5F9D6B, 0xFF74B369 ],
        [ 0xF8AD9305, 0xB7CA9008, 0xD8BB9106, 0xE9B39206, 0xE0B89206, 0x9ED58F09, 0x8FDC8E0A, 0xA8D18F09, 0xF1B09205, 0xE9B39206, 0xE0B89206, 0x9ED58F09, 0xE9B39206, 0xAFCE9009, 0xC9C29107, 0xB7CA9008 ],
        [ 0xFF375790, 0xFF3F6480, 0xFF293FAD, 0xFF467072, 0xFF558855, 0xFF2233BB, 0xFF467072, 0xFF375790, 0xFF304B9E, 0xFF558855, 0xFF4E7C63, 0xFF939CBA, 0xFF3F6480, 0xFF7D96CD, 0xFF6A92DE, 0xFF578DEF ],
        [ 0xFF54977B, 0xFF78A081, 0xFF669B7E, 0xFF669B7E, 0xFF669B7E, 0xFF8AA484, 0xFF78A081, 0xFF7527AB, 0xFF8AA484, 0xFF669B7E, 0xFF7527AB, 0xFF622FC5, 0xFF54977B, 0xFF7527AB, 0xFF4D38E0, 0xFF7527AB ],
        [ 0x09167DBF, 0x561D9E9E, 0x561D9E9E, 0x7220AA91, 0x7220AA91, 0x8C23B586, 0xBF28CA70, 0x7220AA91, 0x7220AA91, 0x221888B4, 0xA625BF7B, 0x3C1B93A9, 0xBF28CA70, 0x8C23B586, 0x7220AA91, 0x7220AA91 ],
        [ 0xFFCCAA66, 0xFFA7E78B, 0xFF3539A9, 0xFF267550, 0xFFA0F392, 0xFF3A26C6, 0xFF267550, 0xFF2B626C, 0xFFA7E78B, 0xFF4400FF, 0xFF3A26C6, 0xFF4400FF, 0xFF3A26C6, 0xFF267550, 0xFF3A26C6, 0xFF4400FF ],
        [ 0x563BA3C4, 0x65299A73, 0x6F1D9439, 0x563BA3C4, 0x5B35A0A8, 0x622C9C81, 0x5839A2BB, 0x5937A1B2, 0x5D329F9D, 0x6627996A, 0x65299A73, 0x68259860, 0x65299A73, 0x5839A2BB, 0x6E1F9542, 0x6E1F9542 ],
        [ 0x8EFF83EF, 0xA4AE6091, 0x94E979D5, 0xA89D597E, 0xA4AE6091, 0xB8623F3A, 0x91F47EE2, 0xAB925471, 0xB8623F3A, 0xBB573B2D, 0xAE874F64, 0xB8623F3A, 0xAE874F64, 0xBE4C3620, 0xB8623F3A, 0xB8623F3A ],
        [ 0x57BE9B5B, 0x865164AF, 0x57BE9B5B, 0x57759B5B, 0x867564AF, 0x2B75CF0B, 0x57519B5B, 0x2B75CF0B, 0x57BE9B5B, 0x579A9B5B, 0x579A9B5B, 0xB39A30FF, 0x57759B5B, 0x57BE9B5B, 0x86BE64AF, 0x2B51CF0B ],
        [ 0xB6BBB0AA, 0xAA336622, 0x5A1C712D, 0x83286C27, 0xB6BBB0AA, 0xBBCCBBBB, 0xB0AAA499, 0x83286C27, 0xB6BBB0AA, 0xB6BBB0AA, 0xB0AAA499, 0xAA999988, 0xAA999988, 0xB0AAA499, 0xAA999988, 0xAA999988 ],
        [ 0x88307CD9, 0xB0733AC2, 0x944468D2, 0x944468D2, 0xA25B52CA, 0xB8812CBD, 0xA6624BC8, 0xB8812CBD, 0x944468D2, 0x9D5458CD, 0xA25B52CA, 0xC99E10B3, 0x994D5FCF, 0xA25B52CA, 0xB47A33C0, 0x903D6ED5 ],
        [ 0xFFFF00FF, 0xFFFF00FF, 0xFFFF00FF, 0xFFFF00FF, 0xFFFF00FF, 0xFFFF00FF, 0xFFFF00FF, 0xFFFF00FF, 0xFFFF00FF, 0xFFFF00FF, 0xFFFF00FF, 0xFFFF00FF, 0xFFFF00FF, 0xFFFF00FF, 0xFFFF00FF, 0xFFFF00FF ], // invalid UASTC block
        [ 0xD1584B29, 0xE59A814D, 0xD5655631, 0xE18D7646, 0xDC7C683D, 0xDC7C683D, 0xE7A18751, 0xE18D7646, 0xD1584B29, 0xD5655631, 0xEBB19359, 0xD1584B29, 0xD86E5D35, 0xD35E512D, 0xEBB19359, 0xDA756339 ],
        [ 0xFF6DAB5C, 0xFF3D6C3D, 0xFF2E5C8B, 0xFF2E5C8B, 0xFFA3D15C, 0xFF356465, 0xFF457417, 0xFF356465, 0xFF35825C, 0xFF64C294, 0xFF45BABA, 0xFF64C294, 0xFF005C5C, 0xFFA3D145, 0xFF64C294, 0xFFA3D145 ],
        [ 0xFF998866, 0xFF7C432B, 0xFF998866, 0xFF83563B, 0xFF8B6649, 0xFF998866, 0xFF8B6649, 0xFF927758, 0xFF927758, 0xFF6D210E, 0xFF661100, 0xFF8B6649, 0xFF729554, 0xFF8F4B88, 0xFF8F4B88, 0xFF866378 ],
        [ 0xEF51CD3E, 0xF32CD1A0, 0xF520D2BF, 0xED5DCC1F, 0xED5DCC1F, 0xED5DCC1F, 0xF045CE5E, 0xF520D2BF, 0xEF51CD3E, 0xEB69CA00, 0xF32CD1A0, 0xF520D2BF, 0xEB69CA00, 0xED5DCC1F, 0xF520D2BF, 0xF615D3DE ],
        [ 0x05B2220A, 0x05B2220A, 0x05B2220A, 0x05B2220A, 0x05B2220A, 0x05B2220A, 0x05B2220A, 0x05B2220A, 0x05B2220A, 0x05B2220A, 0x05B2220A, 0x05B2220A, 0x05B2220A, 0x05B2220A, 0x05B2220A, 0x05B2220A ],
        [ 0xFF455C8B, 0xFFA3E8D1, 0xFF84BABA, 0xFF84BABA, 0xFF648AA2, 0xFFA3E8D1, 0xFF455C8B, 0xFF84BABA, 0xFF54E17A, 0xFF5CE845, 0xFF5CE845, 0xFF54E17A, 0xFFAB548A, 0xFF2E74E8, 0xFF2E74E8, 0xFFE8455C ],
        [ 0xD54F0E9B, 0xB94210C0, 0x9E360EE4, 0x9E360BE4, 0xD54F0B9B, 0xB9420CC0, 0xD54F0C9B, 0xD54F109B, 0xEF5C1077, 0x9E360EE4, 0x9E360BE4, 0xD54F0C9B, 0xB9420CC0, 0xEF5C1077, 0xB9420EC0, 0xB94210C0 ],
        [ 0xDBEF4466, 0xDFAB7720, 0xDFEF7720, 0xDB644466, 0xD9AB2B88, 0xDB644466, 0xDFEF7720, 0xD9642B88, 0xDDAB5E42, 0xDF207720, 0xDBAB4466, 0xDDAB5E42, 0xDD645E42, 0xD9202B88, 0xDF647720, 0xDF647720 ],
        [ 0xFFBB5180, 0xFFBF4076, 0xFFB7658B, 0xFFB7658B, 0xFFAC99A8, 0xFFB0889E, 0xFFAC99A8, 0xFFC61D63, 0xFFC61D63, 0xFFB7658B, 0xFFC32E6D, 0xFFBB5180, 0xFFBB5180, 0xFFB0889E, 0xFFBB5180, 0xFFBB5180 ],
        [ 0xC665B5B9, 0x6D3C9580, 0x2D1E7E56, 0x42288664, 0x9B51A69D, 0x42288664, 0xB15BADAB, 0xB15BADAB, 0xC665B5B9, 0x85479E8F, 0xB15BADAB, 0xB15BADAB, 0x85479E8F, 0x58328E72, 0x85479E8F, 0x58328E72 ],
        [ 0xDF264C7C, 0xE3513F7C, 0xEAA926B3, 0xE77E327C, 0xEAA926B3, 0xEAA9268E, 0xEAA9267C, 0xDF264CA1, 0xEAA9268E, 0xEAA9268E, 0xEAA9267C, 0xEAA9268E, 0xDF264CB3, 0xDF264CB3, 0xE3513FA1, 0xE3513FB3 ],
        [ 0xA99F9F9F, 0xE7E0E0E0, 0xDBD4D4D4, 0x37262626, 0x695A5A5A, 0x9D929292, 0xDBD4D4D4, 0x695A5A5A, 0x9D929292, 0x80737373, 0xDBD4D4D4, 0x74676767, 0x74676767, 0x9D929292, 0x74676767, 0xA99F9F9F ],
        [ 0xFFA95545, 0xFF8D6535, 0xFFA95545, 0xFFA95545, 0xFFB54D4D, 0xFF747525, 0xFFA95545, 0xFF9C5C3E, 0xFFA95545, 0xFF747525, 0xFF816D2D, 0xFFB54D4D, 0xFF9C5C3E, 0xFF816D2D, 0xFFC24555, 0xFFC24555 ],
        [ 0x9FA87512, 0x87A0860A, 0xB7AF661B, 0x87A0860A, 0x6F999502, 0x87A0860A, 0x9FA87512, 0x6F999502, 0xB7AF661B, 0xB7AF661B, 0x9FA87512, 0x6F999502, 0x6F999502, 0x87A0860A, 0x6F999502, 0x9FA87512 ],
        [ 0x5C986C9E, 0x5C30E4A9, 0x5C52BDA6, 0x5C986C9E, 0x5C52BDA6, 0x5C30E4A9, 0x5C52BDA6, 0x5C52BDA6, 0x5C30E4A9, 0x5C986C9E, 0x5C7693A2, 0x5C30E4A9, 0x5C52BDA6, 0x5C30E4A9, 0x5C986C9E, 0x5C52BDA6 ],
        [ 0x39C3B726, 0x39C3B726, 0x2223DE90, 0x2223DE90, 0x2957D26D, 0x318FC449, 0x39C3B726, 0x39C3B726, 0x39C3B726, 0x2223DE90, 0x2957D26D, 0x2223DE90, 0x2223DE90, 0x318FC449, 0x39C3B726, 0x2223DE90 ],
        [ 0xFF9631B9, 0xFFA81DD6, 0xFF79518C, 0xFF6C5F77, 0xFF64686A, 0xFF982EBD, 0xFFA323CE, 0xFFA323CE, 0xFFA81DD6, 0xFF5C705E, 0xFF67656F, 0xFF5F6D62, 0xFF8B3DA9, 0xFF9631B9, 0xFF6C5F77, 0xFF626A66 ],
        [ 0xB3BEF461, 0xFAEF269E, 0xE3DF6A9E, 0xFAEF268A, 0xCBCEB161, 0xB3BEF48A, 0xFAEF269E, 0xFAEF269E, 0xCBCEB175, 0xCBCEB19E, 0xCBCEB19E, 0xB3BEF475, 0xCBCEB18A, 0xFAEF268A, 0xE3DF6A9E, 0xCBCEB18A ],
        [ 0xD94941DF, 0x73754C92, 0x73494C92, 0x739E4C92, 0x4120516C, 0x73494C92, 0x4149516C, 0xD92041DF, 0xA79E46B9, 0xD94941DF, 0x419E516C, 0x73204C92, 0x73204C92, 0xA72046B9, 0x4175516C, 0x73204C92 ],
        [ 0xAEC7DAE2, 0xAEC7DAE2, 0xAEC7DAE2, 0xAEC7DAE2, 0xAEC7DAE2, 0xAEC7DAE2, 0xAEC7DAE2, 0xAEC7DAE2, 0xAEC7DAE2, 0xAEC7DAE2, 0xAEC7DAE2, 0xAEC7DAE2, 0xAEC7DAE2, 0xAEC7DAE2, 0xAEC7DAE2, 0xAEC7DAE2 ],
        [ 0xFFFF00FF, 0xFFFF00FF, 0xFFFF00FF, 0xFFFF00FF, 0xFFFF00FF, 0xFFFF00FF, 0xFFFF00FF, 0xFFFF00FF, 0xFFFF00FF, 0xFFFF00FF, 0xFFFF00FF, 0xFFFF00FF, 0xFFFF00FF, 0xFFFF00FF, 0xFFFF00FF, 0xFFFF00FF ], // invalid UASTC block
        [ 0xFFFF00FF, 0xFFFF00FF, 0xFFFF00FF, 0xFFFF00FF, 0xFFFF00FF, 0xFFFF00FF, 0xFFFF00FF, 0xFFFF00FF, 0xFFFF00FF, 0xFFFF00FF, 0xFFFF00FF, 0xFFFF00FF, 0xFFFF00FF, 0xFFFF00FF, 0xFFFF00FF, 0xFFFF00FF ], // invalid UASTC block
        [ 0xD6379812, 0xD6379812, 0xBF519D15, 0xB45DA017, 0xEA209310, 0x5AC2B423, 0x65B7B222, 0x65B7B222, 0xB45DA017, 0xD6379812, 0x8691AA1D, 0xD6379812, 0x72A8AF20, 0x46D9B926, 0xB45DA017, 0x7C9CAD1E ],
        [ 0xFF00458B, 0xFF00458B, 0xFFD1A35C, 0xFFD1A35C, 0xFF17A3E8, 0xFF00458B, 0xFFBAD12E, 0xFF742EA3, 0xFF0F84CA, 0xFF0764AA, 0xFF742EA3, 0xFFA39C54, 0xFF17A3E8, 0xFF17A3E8, 0xFFC2543D, 0xFFCA7D4D ],
        [ 0x599C51B8, 0xA6ACA583, 0x118D00EB, 0xEEBBF650, 0x599C51B8, 0x118D00EB, 0x118D00EB, 0x599C51B8, 0x599C51B8, 0x599C51B8, 0x599C51B8, 0xA6ACA583, 0xEEBBF650, 0xA6ACA583, 0xA6ACA583, 0xA6ACA583 ],
        [ 0xFF90D111, 0xFF89E314, 0xFF8DDA12, 0xFF91CE11, 0xFF8ED512, 0xFF91CE11, 0xFF8DDA12, 0xFF88E414, 0xFF90D111, 0xFF89E314, 0xFF8FD311, 0xFF90D011, 0xFF8DD812, 0xFF8ED712, 0xFF90D111, 0xFF8ED712 ],
        [ 0x9FA93DB9, 0xA18B6C9B, 0x9EBE1ACE, 0xA35CB96C, 0xA2788C88, 0x9EC410D4, 0xA362AF72, 0x9EBE1ACE, 0xA0A247B2, 0xA26F9980, 0x9EBE1ACE, 0xA26F9980, 0x9FB12FC1, 0x9FB12FC1, 0xA17E828E, 0xA1857795 ],
        [ 0x87A92B9D, 0x87A92B9D, 0x87A92B9D, 0x87A92B9D, 0x87A92B9D, 0x87A92B9D, 0x87A92B9D, 0x87A92B9D, 0x87A92B9D, 0x87A92B9D, 0x87A92B9D, 0x87A92B9D, 0x87A92B9D, 0x87A92B9D, 0x87A92B9D, 0x87A92B9D ],
        [ 0xA2472F80, 0xDA33297D, 0xFE26257A, 0xEC2C277B, 0xC8392B7E, 0xA2472F80, 0xEC2C277B, 0x7E543483, 0xFE26257A, 0xDA33297D, 0xC8392B7E, 0xA2472F80, 0xA2472F80, 0xB4402D7F, 0xEC2C277B, 0x904D3282 ],
        [ 0xAEC4CF56, 0x84DFB6A3, 0x5CF89FEA, 0xA3CCC86B, 0x57FC9CF5, 0x62F4A2E0, 0x62F4A2E0, 0x6AF0A7D2, 0x62F4A2E0, 0x6FECAAC8, 0x7BE5B1B3, 0x7BE5B1B3, 0x5CF89FEA, 0x90D8BD8E, 0x84DFB6A3, 0x7BE5B1B3 ],
        [ 0xFF343831, 0xFF44253E, 0xFF343831, 0xFF156017, 0xFF343831, 0xFF343831, 0xFF156017, 0xFF244C23, 0xFF156017, 0xFF343831, 0xFF44253E, 0xFF44253E, 0xFF244C23, 0xFF156017, 0xFF156017, 0xFF156017 ],
        [ 0x79E37A1C, 0x79E37A1C, 0x79E37A1C, 0x79E37A1C, 0x79E37A1C, 0x79E37A1C, 0x79E37A1C, 0x79E37A1C, 0x79E37A1C, 0x79E37A1C, 0x79E37A1C, 0x79E37A1C, 0x79E37A1C, 0x79E37A1C, 0x79E37A1C, 0x79E37A1C ],
        [ 0xA69E3567, 0x5FC25D56, 0x5F9E5D56, 0x1BC28346, 0x1B7C8346, 0xA6E43567, 0x1B9E8346, 0xEAC21077, 0xEA9E1077, 0x5F9E5D56, 0x1B7C8346, 0x1BC28346, 0x5F7C5D56, 0xA69E3567, 0xA6C23567, 0x5FC25D56 ],
        [ 0xFFBE6B3A, 0xFFC25327, 0xFFCA2E09, 0xFFC25327, 0xFFC25327, 0xFFC5461C, 0xFFBB7744, 0xFF77FF33, 0xFFCC2200, 0xFFC25327, 0xFFC25327, 0xFFA8A778, 0xFFC05F31, 0xFFC5461C, 0xFFCC66AA, 0xFFCC66AA ],
        [ 0x76122C23, 0x6D162C00, 0x76122C23, 0x93092F91, 0x6D162C00, 0xAF0031FA, 0x6D162C00, 0x890C2E69, 0x93092F91, 0x76122C23, 0xA60330D7, 0x9D062FB4, 0xAF0031FA, 0xA60330D7, 0xAF0031FA, 0x800F2D46 ],
        [ 0x9CF93749, 0x83F61558, 0x83F67C58, 0x6CF45A67, 0x83F63758, 0x83F63758, 0x6CF41567, 0x9CF95A49, 0x6CF45A67, 0x9CF97C49, 0x83F63758, 0x83F61558, 0x6CF41567, 0x83F65A58, 0x9CF91549, 0xB3FA5A3B ],
        [ 0x9F5B35CC, 0x5FA6C271, 0x5FA6C271, 0x9F5B35CC, 0x9FA6C271, 0x9FA6C271, 0x5FA6C271, 0x5F5B35CC, 0x9F5B35CC, 0x9FA6C271, 0x5F5B35CC, 0x9F5B35CC, 0x5FA6C271, 0x5FA6C271, 0x5FA6C271, 0x5F5B35CC ],
        [ 0x508DE09A, 0x82E8EFB3, 0x82E8E0B3, 0x82E8E0B3, 0x508DEF9A, 0x508DEF9A, 0x82E8E0B3, 0x508DE09A, 0x82E8E0B3, 0x82E8EFB3, 0x82E8EFB3, 0x82E8EFB3, 0x82E8EFB3, 0x508DE09A, 0x82E8EFB3, 0x508DE09A ],
        [ 0x95E54283, 0xAEAE7C15, 0x88FF2639, 0x95E5425F, 0x95E54215, 0xAEAE7C83, 0xAEAE7C39, 0xA2C96015, 0x95E54239, 0x95E5425F, 0xA2C96039, 0xA2C96083, 0xA2C96083, 0x88FF265F, 0xAEAE7C39, 0xAEAE7C83 ],
        [ 0xCD4CF168, 0xCD5CF168, 0xCD5CF168, 0xC45CFF98, 0xC45CFF98, 0xD65CE235, 0xC45CFF98, 0xDF5CD405, 0xC47CFF98, 0xCD7CF168, 0xDF6CD405, 0xCD6CF168, 0xC45CFF98, 0xD64CE235, 0xD64CE235, 0xC45CFF98 ],
        [ 0xAD8DC3B7, 0x7A82AC9E, 0x126D7E6C, 0x126D7E6C, 0x44779584, 0x5E7DA091, 0x44779584, 0xC692CEC3, 0x9388B8AB, 0x7A82AC9E, 0x9388B8AB, 0x2B728978, 0x44779584, 0xAD8DC3B7, 0x2B728978, 0xAD8DC3B7 ],
        [ 0xCCFF33BB, 0x9EE98E49, 0x88DDBB11, 0xB6F46083, 0x88DDBB11, 0xCCFF33BB, 0x88DDBB11, 0x88DDBB11, 0xC75A99D8, 0xB6F46083, 0xCCFF33BB, 0xB6F46083, 0xC75A99D8, 0x7766BBAA, 0x88DDBB11, 0x88DDBB11 ],
        [ 0xFFCAC4AD, 0xFFC73206, 0xFFC99476, 0xFFC9B69D, 0xFFC73D12, 0xFFC73D12, 0xFFC73206, 0xFFCADAC6, 0xFFC9AA90, 0xFFC99476, 0xFFC8532C, 0xFFCAE5D3, 0xFFC99F83, 0xFFC9AA90, 0xFFCAC4AD, 0xFFCACFBA ],
        [ 0x038EC473, 0x078EA38C, 0x0B8E83A3, 0x03BEC473, 0x038EC473, 0x0377C473, 0x0077E45C, 0x07A7A38C, 0x0077E45C, 0x03BEC473, 0x0077E45C, 0x008EE45C, 0x07BEA38C, 0x008EE45C, 0x0377C473, 0x03A7C473 ],
        [ 0xFF84B4B6, 0xFF844181, 0xFF418EA5, 0xFF418EA5, 0xFF218EA5, 0xFF844181, 0xFF84B4B6, 0xFF218EA5, 0xFF634181, 0xFF636792, 0xFF848EA5, 0xFF41B4B6, 0xFF218EA5, 0xFF636792, 0xFF218EA5, 0xFF84B4B6 ],
    ];

}
