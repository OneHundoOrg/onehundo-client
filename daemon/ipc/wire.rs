use std::io::{Read, Write};

pub const WIRE_MAGIC: u32 = 0x3130_3048; // '100H' little-endian
pub const WIRE_VERSION: u16 = 1;
pub const MAX_PAYLOAD_BYTES: u32 = 4 * 1024 * 1024;

pub const MSG_HELLO: u16 = 0x0001;
pub const MSG_HELLO_ACK: u16 = 0x0002;
pub const MSG_ARENA_REGISTER: u16 = 0x0003;
pub const MSG_ARENA_ACK: u16 = 0x0004;
pub const MSG_EVENT_PUBLISH: u16 = 0x0010;
pub const MSG_EVENT_ACK: u16 = 0x0011;
pub const MSG_LIFECYCLE_SIGNAL: u16 = 0x0020;
pub const MSG_GOODBYE: u16 = 0x00FF;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WireHeader {
    pub magic: u32,
    pub version: u16,
    pub msg_type: u16,
    pub flags: u32,
    pub payload_length: u32,
    pub sequence_number: u64,
}

impl WireHeader {
    pub const SIZE: usize = 24;

    pub fn new(msg_type: u16, sequence_number: u64, payload_length: u32) -> Self {
        Self {
            magic: WIRE_MAGIC,
            version: WIRE_VERSION,
            msg_type,
            flags: 0,
            payload_length,
            sequence_number,
        }
    }

    pub fn to_bytes(&self) -> [u8; Self::SIZE] {
        let mut buf = [0u8; Self::SIZE];
        buf[0..4].copy_from_slice(&self.magic.to_le_bytes());
        buf[4..6].copy_from_slice(&self.version.to_le_bytes());
        buf[6..8].copy_from_slice(&self.msg_type.to_le_bytes());
        buf[8..12].copy_from_slice(&self.flags.to_le_bytes());
        buf[12..16].copy_from_slice(&self.payload_length.to_le_bytes());
        buf[16..24].copy_from_slice(&self.sequence_number.to_le_bytes());
        buf
    }

    pub fn from_bytes(buf: &[u8; Self::SIZE]) -> Result<Self, String> {
        let magic = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
        if magic != WIRE_MAGIC {
            return Err(format!("invalid wire magic 0x{:08x}", magic));
        }
        let version = u16::from_le_bytes([buf[4], buf[5]]);
        if version != WIRE_VERSION {
            return Err(format!("unsupported wire version {}", version));
        }
        let msg_type = u16::from_le_bytes([buf[6], buf[7]]);
        let flags = u32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]);
        let payload_length = u32::from_le_bytes([buf[12], buf[13], buf[14], buf[15]]);
        if payload_length > MAX_PAYLOAD_BYTES {
            return Err(format!("payload_length {} exceeds limit", payload_length));
        }
        let sequence_number = u64::from_le_bytes([
            buf[16], buf[17], buf[18], buf[19], buf[20], buf[21], buf[22], buf[23],
        ]);
        Ok(Self {
            magic,
            version,
            msg_type,
            flags,
            payload_length,
            sequence_number,
        })
    }
}

pub fn read_wire_frame<R: Read>(reader: &mut R) -> Result<Option<(WireHeader, Vec<u8>)>, String> {
    let mut hdr_buf = [0u8; WireHeader::SIZE];
    let mut read = 0usize;
    while read < WireHeader::SIZE {
        match reader.read(&mut hdr_buf[read..]) {
            Ok(0) => {
                if read == 0 {
                    return Ok(None);
                }
                return Err("unexpected EOF reading wire header".into());
            }
            Ok(n) => read += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(format!("wire header read error: {}", e)),
        }
    }
    let hdr = WireHeader::from_bytes(&hdr_buf)?;
    let mut payload = vec![0u8; hdr.payload_length as usize];
    if hdr.payload_length > 0 {
        reader
            .read_exact(&mut payload)
            .map_err(|e| format!("wire payload read error: {}", e))?;
    }
    Ok(Some((hdr, payload)))
}

pub fn write_wire_frame<W: Write>(
    writer: &mut W,
    msg_type: u16,
    sequence_number: u64,
    payload: &[u8],
) -> Result<(), String> {
    let hdr = WireHeader::new(msg_type, sequence_number, payload.len() as u32);
    writer
        .write_all(&hdr.to_bytes())
        .map_err(|e| format!("wire header write error: {}", e))?;
    if !payload.is_empty() {
        writer
            .write_all(payload)
            .map_err(|e| format!("wire payload write error: {}", e))?;
    }
    writer
        .flush()
        .map_err(|e| format!("wire flush error: {}", e))?;
    Ok(())
}
