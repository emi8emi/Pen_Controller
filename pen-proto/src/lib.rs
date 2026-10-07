//! The wire protocol between the pen controller and its clients (version 1).
//!
//! Frames: `u32 length (LE)` | `u8 tag` | payload, where `length` counts the tag and the payload.
//! All numbers are little-endian. Samples have a fixed size (see [`SAMPLE_BYTES`]), so a client in
//! any language can decode them with a plain struct read.
//!
//! Server -> client: [`ServerMsg`] (hello, presence, samples).
//! Client -> server: [`ClientMsg`] (hello, mode, canvas, brush, canvas options, clear).
//!
//! Transport (named pipe, local WebSocket bridge) is not part of this crate; use [`FrameDecoder`]
//! to turn whatever bytes arrive into messages.

use std::fmt;

pub const PROTOCOL_VERSION: u16 = 1;
/// Largest accepted frame body. Anything bigger is treated as a protocol error, not buffered.
pub const MAX_FRAME: usize = 64 * 1024;
/// Longest client name in a hello.
pub const MAX_NAME: usize = 255;

// ------------------------------------------------------------------------------------------------
// Data
// ------------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Phase {
    Hover = 0,
    Down = 1,
    Move = 2,
    Up = 3,
}

impl Phase {
    fn from_u8(v: u8) -> Result<Phase, DecodeError> {
        match v {
            0 => Ok(Phase::Hover),
            1 => Ok(Phase::Down),
            2 => Ok(Phase::Move),
            3 => Ok(Phase::Up),
            _ => Err(DecodeError::BadValue("phase")),
        }
    }
}

/// One pen sample. Capabilities a device does not have are `None`, never a fake zero.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sample {
    pub pen_id: u8,
    pub phase: Phase,
    /// Overlay pixels.
    pub x: f32,
    pub y: f32,
    /// Normalised tablet coordinates (0..1), when the source provides them.
    pub tablet: Option<(f32, f32)>,
    /// 0..1.
    pub pressure: Option<f32>,
    /// Degrees from vertical, x then y.
    pub tilt: Option<(f32, f32)>,
    /// Bit 0 = barrel button.
    pub buttons: u8,
    pub eraser: bool,
    /// Microseconds, monotonic. Only comparable between samples of the same session.
    pub t_us: u64,
    /// True if `t_us` came from the device, false if it is the time we received the sample.
    pub t_hardware: bool,
}

/// Wire size of a sample body (without the frame header and tag).
pub const SAMPLE_BYTES: usize = 40;

const F_PRESSURE: u8 = 1;
const F_TILT: u8 = 2;
const F_TABLET: u8 = 4;
const F_ERASER: u8 = 8;
const F_HW_TIME: u8 = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Mode {
    /// Only presence events; the controller has no window.
    Presence = 0,
    /// Captures and forwards samples; draws nothing.
    Stream = 1,
    /// Captures, draws the ink itself and forwards samples.
    Canvas = 2,
}

impl Mode {
    fn from_u8(v: u8) -> Result<Mode, DecodeError> {
        match v {
            0 => Ok(Mode::Presence),
            1 => Ok(Mode::Stream),
            2 => Ok(Mode::Canvas),
            _ => Err(DecodeError::BadValue("mode")),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Brush {
    pub size: f32,
    /// 0..1.
    pub opacity: f32,
    pub rgb: [u8; 3],
    pub size_from_pressure: bool,
}

/// How the controller's canvas behaves. Clients set these; the controller's own UI (tray) sets the same
/// options for clients that do not.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct CanvasOptions {
    /// Clear the ink whenever the overlay is hidden, for any reason.
    pub clear_on_dismiss: bool,
}

/// What the connected pen/tablet can report. Clients should check this instead of assuming.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Capabilities {
    pub pressure: bool,
    pub tilt: bool,
    pub hover: bool,
    pub eraser: bool,
    pub hardware_time: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ServerMsg {
    Hello { protocol: u16, capabilities: Capabilities },
    /// The pen came into range / left the overlay (Presence mode clients mostly care about this).
    Presence { active: bool },
    Sample(Sample),
}

#[derive(Clone, Debug, PartialEq)]
pub enum ClientMsg {
    Hello { protocol: u16, name: String },
    SetMode(Mode),
    SetCanvas(Rect),
    SetBrush(Brush),
    SetCanvasOptions(CanvasOptions),
    ClearCanvas,
}

// ------------------------------------------------------------------------------------------------
// Errors
// ------------------------------------------------------------------------------------------------

#[derive(Debug, PartialEq, Eq)]
pub enum DecodeError {
    /// The frame body ended before the message did.
    Truncated,
    /// The frame body had bytes left over after the message.
    TrailingBytes,
    BadTag(u8),
    BadValue(&'static str),
    /// A frame declared a body larger than [`MAX_FRAME`] (or zero).
    BadLength(usize),
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DecodeError::Truncated => write!(f, "frame ended early"),
            DecodeError::TrailingBytes => write!(f, "frame has trailing bytes"),
            DecodeError::BadTag(t) => write!(f, "unknown message tag {t}"),
            DecodeError::BadValue(what) => write!(f, "invalid {what}"),
            DecodeError::BadLength(n) => write!(f, "bad frame length {n}"),
        }
    }
}

impl std::error::Error for DecodeError {}

// ------------------------------------------------------------------------------------------------
// Byte helpers
// ------------------------------------------------------------------------------------------------

struct Writer(Vec<u8>);

impl Writer {
    fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    fn u16(&mut self, v: u16) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn i32(&mut self, v: i32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn u64(&mut self, v: u64) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn f32(&mut self, v: f32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], DecodeError> {
        if self.0.len() < n {
            return Err(DecodeError::Truncated);
        }
        let (head, tail) = self.0.split_at(n);
        self.0 = tail;
        Ok(head)
    }
    fn u8(&mut self) -> Result<u8, DecodeError> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, DecodeError> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    fn u32(&mut self) -> Result<u32, DecodeError> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn i32(&mut self) -> Result<i32, DecodeError> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> Result<u64, DecodeError> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn f32(&mut self) -> Result<f32, DecodeError> {
        Ok(f32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn finish(self) -> Result<(), DecodeError> {
        if self.0.is_empty() {
            Ok(())
        } else {
            Err(DecodeError::TrailingBytes)
        }
    }
}

fn write_sample(w: &mut Writer, s: &Sample) {
    let mut flags = 0u8;
    if s.pressure.is_some() {
        flags |= F_PRESSURE;
    }
    if s.tilt.is_some() {
        flags |= F_TILT;
    }
    if s.tablet.is_some() {
        flags |= F_TABLET;
    }
    if s.eraser {
        flags |= F_ERASER;
    }
    if s.t_hardware {
        flags |= F_HW_TIME;
    }
    w.u8(s.pen_id);
    w.u8(s.phase as u8);
    w.u8(flags);
    w.u8(s.buttons);
    w.f32(s.x);
    w.f32(s.y);
    w.f32(s.pressure.unwrap_or(0.0));
    let (tx, ty) = s.tilt.unwrap_or((0.0, 0.0));
    w.f32(tx);
    w.f32(ty);
    let (ax, ay) = s.tablet.unwrap_or((0.0, 0.0));
    w.f32(ax);
    w.f32(ay);
    w.u64(s.t_us);
}

fn read_sample(r: &mut Reader) -> Result<Sample, DecodeError> {
    let pen_id = r.u8()?;
    let phase = Phase::from_u8(r.u8()?)?;
    let flags = r.u8()?;
    if flags & !(F_PRESSURE | F_TILT | F_TABLET | F_ERASER | F_HW_TIME) != 0 {
        return Err(DecodeError::BadValue("sample flags"));
    }
    let buttons = r.u8()?;
    let x = r.f32()?;
    let y = r.f32()?;
    let pressure = r.f32()?;
    let tilt_x = r.f32()?;
    let tilt_y = r.f32()?;
    let tablet_x = r.f32()?;
    let tablet_y = r.f32()?;
    let t_us = r.u64()?;
    Ok(Sample {
        pen_id,
        phase,
        x,
        y,
        tablet: if flags & F_TABLET != 0 { Some((tablet_x, tablet_y)) } else { None },
        pressure: if flags & F_PRESSURE != 0 { Some(pressure) } else { None },
        tilt: if flags & F_TILT != 0 { Some((tilt_x, tilt_y)) } else { None },
        buttons,
        eraser: flags & F_ERASER != 0,
        t_us,
        t_hardware: flags & F_HW_TIME != 0,
    })
}

fn caps_to_byte(c: &Capabilities) -> u8 {
    (c.pressure as u8) | ((c.tilt as u8) << 1) | ((c.hover as u8) << 2) | ((c.eraser as u8) << 3) | ((c.hardware_time as u8) << 4)
}

fn caps_from_byte(b: u8) -> Result<Capabilities, DecodeError> {
    if b & !0x1F != 0 {
        return Err(DecodeError::BadValue("capabilities"));
    }
    Ok(Capabilities {
        pressure: b & 1 != 0,
        tilt: b & 2 != 0,
        hover: b & 4 != 0,
        eraser: b & 8 != 0,
        hardware_time: b & 16 != 0,
    })
}

// ------------------------------------------------------------------------------------------------
// Messages
// ------------------------------------------------------------------------------------------------

/// A message that can be framed. Implemented for [`ServerMsg`] and [`ClientMsg`].
pub trait Message: Sized {
    #[doc(hidden)]
    fn write_body(&self, out: &mut Vec<u8>) -> u8;
    #[doc(hidden)]
    fn read_body(tag: u8, body: &[u8]) -> Result<Self, DecodeError>;
}

const S_HELLO: u8 = 1;
const S_PRESENCE: u8 = 2;
const S_SAMPLE: u8 = 3;

impl Message for ServerMsg {
    fn write_body(&self, out: &mut Vec<u8>) -> u8 {
        let mut w = Writer(std::mem::take(out));
        let tag = match self {
            ServerMsg::Hello { protocol, capabilities } => {
                w.u16(*protocol);
                w.u8(caps_to_byte(capabilities));
                S_HELLO
            }
            ServerMsg::Presence { active } => {
                w.u8(*active as u8);
                S_PRESENCE
            }
            ServerMsg::Sample(s) => {
                write_sample(&mut w, s);
                S_SAMPLE
            }
        };
        *out = w.0;
        tag
    }

    fn read_body(tag: u8, body: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader(body);
        let msg = match tag {
            S_HELLO => ServerMsg::Hello { protocol: r.u16()?, capabilities: caps_from_byte(r.u8()?)? },
            S_PRESENCE => ServerMsg::Presence {
                active: match r.u8()? {
                    0 => false,
                    1 => true,
                    _ => return Err(DecodeError::BadValue("presence")),
                },
            },
            S_SAMPLE => ServerMsg::Sample(read_sample(&mut r)?),
            t => return Err(DecodeError::BadTag(t)),
        };
        r.finish()?;
        Ok(msg)
    }
}

const C_HELLO: u8 = 1;
const C_MODE: u8 = 2;
const C_CANVAS: u8 = 3;
const C_BRUSH: u8 = 4;
const C_CLEAR: u8 = 5;
const C_OPTIONS: u8 = 6;

const OPT_CLEAR_ON_DISMISS: u8 = 1;

impl Message for ClientMsg {
    fn write_body(&self, out: &mut Vec<u8>) -> u8 {
        let mut w = Writer(std::mem::take(out));
        let tag = match self {
            ClientMsg::Hello { protocol, name } => {
                // cut on a char boundary so the name stays valid UTF-8
                let mut n = name.len().min(MAX_NAME);
                while !name.is_char_boundary(n) {
                    n -= 1;
                }
                w.u16(*protocol);
                w.u8(n as u8);
                w.0.extend_from_slice(&name.as_bytes()[..n]);
                C_HELLO
            }
            ClientMsg::SetMode(m) => {
                w.u8(*m as u8);
                C_MODE
            }
            ClientMsg::SetCanvas(r) => {
                w.i32(r.x);
                w.i32(r.y);
                w.u32(r.w);
                w.u32(r.h);
                C_CANVAS
            }
            ClientMsg::SetBrush(b) => {
                w.f32(b.size);
                w.f32(b.opacity);
                w.u8(b.rgb[0]);
                w.u8(b.rgb[1]);
                w.u8(b.rgb[2]);
                w.u8(b.size_from_pressure as u8);
                C_BRUSH
            }
            ClientMsg::SetCanvasOptions(o) => {
                w.u8(if o.clear_on_dismiss { OPT_CLEAR_ON_DISMISS } else { 0 });
                C_OPTIONS
            }
            ClientMsg::ClearCanvas => C_CLEAR,
        };
        *out = w.0;
        tag
    }

    fn read_body(tag: u8, body: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader(body);
        let msg = match tag {
            C_HELLO => {
                let protocol = r.u16()?;
                let n = r.u8()? as usize;
                let name = std::str::from_utf8(r.take(n)?).map_err(|_| DecodeError::BadValue("name"))?.to_string();
                ClientMsg::Hello { protocol, name }
            }
            C_MODE => ClientMsg::SetMode(Mode::from_u8(r.u8()?)?),
            C_CANVAS => ClientMsg::SetCanvas(Rect { x: r.i32()?, y: r.i32()?, w: r.u32()?, h: r.u32()? }),
            C_BRUSH => {
                let size = r.f32()?;
                let opacity = r.f32()?;
                let rgb = [r.u8()?, r.u8()?, r.u8()?];
                let size_from_pressure = match r.u8()? {
                    0 => false,
                    1 => true,
                    _ => return Err(DecodeError::BadValue("size_from_pressure")),
                };
                ClientMsg::SetBrush(Brush { size, opacity, rgb, size_from_pressure })
            }
            C_OPTIONS => {
                let flags = r.u8()?;
                if flags & !OPT_CLEAR_ON_DISMISS != 0 {
                    return Err(DecodeError::BadValue("canvas options"));
                }
                ClientMsg::SetCanvasOptions(CanvasOptions { clear_on_dismiss: flags & OPT_CLEAR_ON_DISMISS != 0 })
            }
            C_CLEAR => ClientMsg::ClearCanvas,
            t => return Err(DecodeError::BadTag(t)),
        };
        r.finish()?;
        Ok(msg)
    }
}

/// Encodes one message as a complete frame, ready to write to the transport.
pub fn encode<M: Message>(msg: &M) -> Vec<u8> {
    let mut body = Vec::with_capacity(SAMPLE_BYTES);
    let tag = msg.write_body(&mut body);
    let len = (body.len() + 1) as u32;
    let mut frame = Vec::with_capacity(4 + 1 + body.len());
    frame.extend_from_slice(&len.to_le_bytes());
    frame.push(tag);
    frame.extend_from_slice(&body);
    frame
}

/// Turns a stream of bytes (as they arrive from a pipe or socket) into messages.
///
/// A frame whose body is invalid is consumed and reported as an `Err`, and decoding can continue with
/// the next frame. A bad *length* means the stream is out of sync: drop the connection.
#[derive(Default)]
pub struct FrameDecoder {
    buf: Vec<u8>,
}

impl FrameDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// The next complete message, `Ok(None)` if more bytes are needed.
    pub fn next<M: Message>(&mut self) -> Result<Option<M>, DecodeError> {
        if self.buf.len() < 4 {
            return Ok(None);
        }
        let len = u32::from_le_bytes(self.buf[..4].try_into().unwrap()) as usize;
        if len == 0 || len > MAX_FRAME {
            return Err(DecodeError::BadLength(len));
        }
        if self.buf.len() < 4 + len {
            return Ok(None);
        }
        let tag = self.buf[4];
        let result = M::read_body(tag, &self.buf[5..4 + len]);
        self.buf.drain(..4 + len);
        result.map(Some)
    }
}

// ------------------------------------------------------------------------------------------------
// Tests
// ------------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn full_sample() -> Sample {
        Sample {
            pen_id: 1,
            phase: Phase::Move,
            x: 812.5,
            y: 301.25,
            tablet: Some((0.41, 0.33)),
            pressure: Some(0.62),
            tilt: Some((-12.0, 30.5)),
            buttons: 1,
            eraser: true,
            t_us: 118_234_501,
            t_hardware: true,
        }
    }

    fn bare_sample() -> Sample {
        Sample {
            pen_id: 0,
            phase: Phase::Hover,
            x: 1.0,
            y: 2.0,
            tablet: None,
            pressure: None,
            tilt: None,
            buttons: 0,
            eraser: false,
            t_us: 5,
            t_hardware: false,
        }
    }

    fn roundtrip_server(m: ServerMsg) {
        let frame = encode(&m);
        let mut d = FrameDecoder::new();
        d.push(&frame);
        assert_eq!(d.next::<ServerMsg>().unwrap(), Some(m));
        assert_eq!(d.next::<ServerMsg>().unwrap(), None);
    }

    fn roundtrip_client(m: ClientMsg) {
        let frame = encode(&m);
        let mut d = FrameDecoder::new();
        d.push(&frame);
        assert_eq!(d.next::<ClientMsg>().unwrap(), Some(m));
    }

    #[test]
    fn server_messages_roundtrip() {
        roundtrip_server(ServerMsg::Hello {
            protocol: PROTOCOL_VERSION,
            capabilities: Capabilities { pressure: true, tilt: false, hover: true, eraser: true, hardware_time: false },
        });
        roundtrip_server(ServerMsg::Presence { active: true });
        roundtrip_server(ServerMsg::Presence { active: false });
        roundtrip_server(ServerMsg::Sample(full_sample()));
        roundtrip_server(ServerMsg::Sample(bare_sample()));
    }

    #[test]
    fn client_messages_roundtrip() {
        roundtrip_client(ClientMsg::Hello { protocol: 1, name: "written-to-voice".into() });
        roundtrip_client(ClientMsg::SetMode(Mode::Canvas));
        roundtrip_client(ClientMsg::SetCanvas(Rect { x: -50, y: 10, w: 1920, h: 1079 }));
        roundtrip_client(ClientMsg::SetBrush(Brush { size: 6.0, opacity: 0.5, rgb: [255, 72, 176], size_from_pressure: true }));
        roundtrip_client(ClientMsg::SetCanvasOptions(CanvasOptions { clear_on_dismiss: true }));
        roundtrip_client(ClientMsg::SetCanvasOptions(CanvasOptions::default()));
        roundtrip_client(ClientMsg::ClearCanvas);
    }

    #[test]
    fn canvas_options_default_to_keeping_the_ink() {
        assert!(!CanvasOptions::default().clear_on_dismiss);
    }

    #[test]
    fn unknown_canvas_option_bits_are_rejected() {
        let mut d = FrameDecoder::new();
        d.push(&[2, 0, 0, 0, C_OPTIONS, 0x80]);
        assert_eq!(d.next::<ClientMsg>(), Err(DecodeError::BadValue("canvas options")));
    }

    #[test]
    fn absent_capabilities_stay_absent() {
        let frame = encode(&ServerMsg::Sample(bare_sample()));
        let mut d = FrameDecoder::new();
        d.push(&frame);
        match d.next::<ServerMsg>().unwrap() {
            Some(ServerMsg::Sample(s)) => {
                assert_eq!(s.pressure, None);
                assert_eq!(s.tilt, None);
                assert_eq!(s.tablet, None);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn sample_frame_has_the_documented_size() {
        let frame = encode(&ServerMsg::Sample(full_sample()));
        // 4 length bytes + 1 tag + the fixed sample body
        assert_eq!(frame.len(), 4 + 1 + SAMPLE_BYTES);
        assert_eq!(u32::from_le_bytes(frame[..4].try_into().unwrap()) as usize, 1 + SAMPLE_BYTES);
    }

    #[test]
    fn decoder_handles_split_and_batched_input() {
        let a = encode(&ServerMsg::Presence { active: true });
        let b = encode(&ServerMsg::Sample(full_sample()));
        let mut all = a.clone();
        all.extend_from_slice(&b);

        // one byte at a time
        let mut d = FrameDecoder::new();
        let mut got = Vec::new();
        for byte in &all {
            d.push(&[*byte]);
            while let Some(m) = d.next::<ServerMsg>().unwrap() {
                got.push(m);
            }
        }
        assert_eq!(got, vec![ServerMsg::Presence { active: true }, ServerMsg::Sample(full_sample())]);

        // both frames in a single push
        let mut d = FrameDecoder::new();
        d.push(&all);
        assert!(matches!(d.next::<ServerMsg>().unwrap(), Some(ServerMsg::Presence { active: true })));
        assert!(matches!(d.next::<ServerMsg>().unwrap(), Some(ServerMsg::Sample(_))));
        assert_eq!(d.next::<ServerMsg>().unwrap(), None);
    }

    #[test]
    fn oversize_and_zero_lengths_are_rejected() {
        let mut d = FrameDecoder::new();
        d.push(&(MAX_FRAME as u32 + 1).to_le_bytes());
        assert_eq!(d.next::<ServerMsg>(), Err(DecodeError::BadLength(MAX_FRAME + 1)));

        let mut d = FrameDecoder::new();
        d.push(&0u32.to_le_bytes());
        assert_eq!(d.next::<ServerMsg>(), Err(DecodeError::BadLength(0)));
    }

    #[test]
    fn unknown_tags_and_values_are_rejected() {
        let mut d = FrameDecoder::new();
        d.push(&[1, 0, 0, 0, 99]);
        assert_eq!(d.next::<ServerMsg>(), Err(DecodeError::BadTag(99)));

        // a sample with an invalid phase
        let mut frame = encode(&ServerMsg::Sample(full_sample()));
        frame[5 + 1] = 9; // phase byte
        let mut d = FrameDecoder::new();
        d.push(&frame);
        assert_eq!(d.next::<ServerMsg>(), Err(DecodeError::BadValue("phase")));

        // a sample with unknown flag bits
        let mut frame = encode(&ServerMsg::Sample(full_sample()));
        frame[5 + 2] |= 0x80;
        let mut d = FrameDecoder::new();
        d.push(&frame);
        assert_eq!(d.next::<ServerMsg>(), Err(DecodeError::BadValue("sample flags")));
    }

    #[test]
    fn truncated_and_trailing_bodies_are_rejected() {
        // presence frame whose body is empty
        let mut d = FrameDecoder::new();
        d.push(&[1, 0, 0, 0, S_PRESENCE]);
        assert_eq!(d.next::<ServerMsg>(), Err(DecodeError::Truncated));

        // presence frame with an extra byte
        let mut d = FrameDecoder::new();
        d.push(&[3, 0, 0, 0, S_PRESENCE, 1, 0]);
        assert_eq!(d.next::<ServerMsg>(), Err(DecodeError::TrailingBytes));
    }

    #[test]
    fn long_client_names_are_cut_on_a_char_boundary() {
        let name = "é".repeat(200); // 400 bytes
        let frame = encode(&ClientMsg::Hello { protocol: 1, name });
        let mut d = FrameDecoder::new();
        d.push(&frame);
        match d.next::<ClientMsg>().unwrap() {
            Some(ClientMsg::Hello { name, .. }) => {
                assert!(name.len() <= MAX_NAME);
                assert!(name.chars().all(|c| c == 'é'));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn the_decoder_keeps_its_place_after_a_bad_body() {
        // a bad frame followed by a good one: the bad frame is consumed, the next one still decodes
        let mut bad = vec![2, 0, 0, 0, S_PRESENCE, 7];
        bad.extend_from_slice(&encode(&ServerMsg::Presence { active: false }));
        let mut d = FrameDecoder::new();
        d.push(&bad);
        assert_eq!(d.next::<ServerMsg>(), Err(DecodeError::BadValue("presence")));
        assert_eq!(d.next::<ServerMsg>().unwrap(), Some(ServerMsg::Presence { active: false }));
    }
}
