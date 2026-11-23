//! Decode a bounded static PNG into the Channel's unassociated RGBA8 format.
use crate::protocol_v4::{MAX_IMAGE_BYTES, MAX_IMAGE_DIMENSION};
use png::{BitDepth, ColorType, Decoder, Limits, Transformations};
use std::io::Cursor;

pub(crate) fn decode(input: &[u8], width: u32, height: u32) -> Option<Vec<u8>> {
    if input.len() > MAX_IMAGE_BYTES
        || width == 0
        || height == 0
        || width > MAX_IMAGE_DIMENSION
        || height > MAX_IMAGE_DIMENSION
    {
        return None;
    }
    let bytes = (width as usize) * (height as usize) * 4;
    if bytes > MAX_IMAGE_BYTES {
        return None;
    }
    let mut decoder = Decoder::new_with_limits(
        Cursor::new(input),
        Limits {
            bytes: MAX_IMAGE_BYTES,
        },
    );
    decoder.set_ignore_text_chunk(true);
    decoder.set_transformations(Transformations::EXPAND | Transformations::STRIP_16);
    let mut reader = decoder.read_info().ok()?;
    let size = reader.output_buffer_size()?;
    let info = reader.info();
    if (info.width, info.height) != (width, height)
        || info.animation_control.is_some()
        || size > bytes
    {
        return None;
    }
    let mut samples = vec![0; size];
    let frame = reader.next_frame(&mut samples).ok()?;
    if (frame.width, frame.height, frame.bit_depth) != (width, height, BitDepth::Eight) {
        return None;
    }
    reader.finish().ok()?;
    samples.truncate(frame.buffer_size());
    if frame.color_type == ColorType::Rgba {
        return (samples.len() == bytes).then_some(samples);
    }
    let mut rgba = Vec::with_capacity(bytes);
    for pixel in samples.chunks_exact(frame.color_type.samples()) {
        let pixel = match *pixel {
            [gray] => [gray, gray, gray, 255],
            [gray, alpha] => [gray, gray, gray, alpha],
            [red, green, blue] => [red, green, blue, 255],
            _ => return None,
        };
        rgba.extend_from_slice(&pixel);
    }
    (rgba.len() == bytes).then_some(rgba)
}
