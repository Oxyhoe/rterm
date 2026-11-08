//! Image decoding: PNG / JPEG / animated GIF → RGBA8 frames.

use std::io::Cursor;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};

#[derive(Debug, Clone)]
pub struct Image {
    pub w: u32,
    pub h: u32,
    pub rgba: Arc<Vec<u8>>,
}

impl Image {
    pub fn bytes(&self) -> usize {
        self.rgba.len()
    }

    pub fn from_rgba(w: u32, h: u32, rgba: Vec<u8>) -> Self {
        Self {
            w,
            h,
            rgba: Arc::new(rgba),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Frame {
    pub image: Image,
    pub delay: Duration,
}

pub enum Decoded {
    Static(Image),
    Animated(Vec<Frame>),
}

fn unpremultiply(rgba: &mut [u8]) {
    for px in rgba.chunks_exact_mut(4) {
        let a = px[3];
        if a > 0 && a < 255 {
            let f = 255.0 / f32::from(a);
            px[0] = (f32::from(px[0]) * f).min(255.0) as u8;
            px[1] = (f32::from(px[1]) * f).min(255.0) as u8;
            px[2] = (f32::from(px[2]) * f).min(255.0) as u8;
        }
    }
}

pub fn decode_png(bytes: &[u8]) -> Result<Image> {
    let decoder = png::Decoder::new(Cursor::new(bytes));
    let mut reader = decoder.read_info().context("png header")?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).context("png frame")?;
    let (w, h) = (info.width, info.height);
    let rgba = match info.color_type {
        png::ColorType::Rgba => buf,
        png::ColorType::Rgb => {
            let mut out = Vec::with_capacity((w * h * 4) as usize);
            for px in buf.chunks_exact(3) {
                out.extend_from_slice(&[px[0], px[1], px[2], 255]);
            }
            out
        }
        png::ColorType::Grayscale => {
            let mut out = Vec::with_capacity((w * h * 4) as usize);
            for &v in &buf {
                out.extend_from_slice(&[v, v, v, 255]);
            }
            out
        }
        png::ColorType::GrayscaleAlpha => {
            let mut out = Vec::with_capacity((w * h * 4) as usize);
            for px in buf.chunks_exact(2) {
                out.extend_from_slice(&[px[0], px[0], px[0], px[1]]);
            }
            out
        }
        other => bail!("unsupported png color type {other:?}"),
    };
    Ok(Image::from_rgba(w, h, rgba))
}

pub fn decode_jpeg(bytes: &[u8]) -> Result<Image> {
    let mut decoder = jpeg_decoder::Decoder::new(Cursor::new(bytes));
    decoder.read_info().map_err(|e| anyhow!("jpeg: {e}"))?;
    let pixels = decoder.decode().map_err(|e| anyhow!("jpeg: {e}"))?;
    let info = decoder.info().ok_or_else(|| anyhow!("jpeg: no info"))?;
    let (w, h) = (u32::from(info.width), u32::from(info.height));
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    match info.pixel_format {
        jpeg_decoder::PixelFormat::RGB24 => {
            for px in pixels.chunks_exact(3) {
                rgba.extend_from_slice(&[px[0], px[1], px[2], 255]);
            }
        }
        jpeg_decoder::PixelFormat::L8 => {
            for &v in &pixels {
                rgba.extend_from_slice(&[v, v, v, 255]);
            }
        }
        other => bail!("unsupported jpeg format {other:?}"),
    }
    Ok(Image::from_rgba(w, h, rgba))
}

/// Decode GIF with full canvas compositing (disposal methods, transparency).
pub fn decode_gif(bytes: &[u8]) -> Result<Vec<Frame>> {
    let mut opts = gif::DecodeOptions::new();
    opts.set_color_output(gif::ColorOutput::RGBA);
    let mut decoder = opts.read_info(Cursor::new(bytes)).context("gif header")?;
    let cw = u32::from(decoder.width());
    let ch = u32::from(decoder.height());
    if cw == 0 || ch == 0 {
        bail!("gif has zero size");
    }
    let mut canvas = vec![0u8; (cw * ch * 4) as usize];
    let mut saved: Option<Vec<u8>> = None;
    let mut prev_dispose = gif::DisposalMethod::Any;
    let mut prev_rect = (0u32, 0u32, 0u32, 0u32);
    let mut frames = Vec::new();

    loop {
        let frame = match decoder.read_next_frame() {
            Ok(Some(frame)) => frame,
            Ok(None) => break, // end of stream
            Err(_) => break,   // a broken tail — keep what we decoded
        };
        let (fx, fy) = (u32::from(frame.left), u32::from(frame.top));
        let (fw, fh) = (u32::from(frame.width), u32::from(frame.height));

        // Apply the *previous* frame's disposal before drawing the new one.
        match prev_dispose {
            gif::DisposalMethod::Background => clear_rect(&mut canvas, cw, prev_rect),
            gif::DisposalMethod::Previous => {
                if let Some(s) = saved.take() {
                    canvas = s;
                }
            }
            _ => {}
        }
        if frame.dispose == gif::DisposalMethod::Previous {
            saved = Some(canvas.clone());
        }

        blit(&mut canvas, cw, fx, fy, fw, fh, &frame.buffer);

        let mut rgba = vec![0u8; (cw * ch * 4) as usize];
        rgba.copy_from_slice(&canvas);
        let delay_ms: u64 = if frame.delay == 0 {
            100
        } else {
            u64::from(frame.delay) * 10
        };
        frames.push(Frame {
            image: Image::from_rgba(cw, ch, rgba),
            delay: Duration::from_millis(delay_ms),
        });

        prev_dispose = frame.dispose;
        prev_rect = (fx, fy, fw, fh);
    }

    if frames.is_empty() {
        bail!("gif has no frames");
    }
    Ok(frames)
}

fn blit(canvas: &mut [u8], stride: u32, x: u32, y: u32, w: u32, h: u32, src: &[u8]) {
    for row in 0..h {
        let cy = y + row;
        for col in 0..w {
            let cx = x + col;
            let si = ((row * w + col) * 4) as usize;
            if si + 4 > src.len() {
                return;
            }
            if src[si + 3] == 0 {
                continue; // transparent: keep canvas pixel
            }
            let di = ((cy * stride + cx) * 4) as usize;
            if di + 4 > canvas.len() {
                continue;
            }
            canvas[di..di + 4].copy_from_slice(&src[si..si + 4]);
        }
    }
}

fn clear_rect(canvas: &mut [u8], stride: u32, (x, y, w, h): (u32, u32, u32, u32)) {
    for row in y..y + h {
        for col in x..x + w {
            let di = ((row * stride + col) * 4) as usize;
            if di + 4 <= canvas.len() {
                canvas[di..di + 4].fill(0);
            }
        }
    }
}

/// Detect format from magic bytes and decode. Returns static or animated content.
pub fn decode_any(bytes: &[u8]) -> Result<Decoded> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Ok(Decoded::Static(decode_png(bytes)?));
    }
    if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        return Ok(Decoded::Static(decode_jpeg(bytes)?));
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Ok(Decoded::Animated(decode_gif(bytes)?));
    }
    bail!("unsupported image format (png/jpeg/gif only)")
}

/// zlib inflate (kitty z=32/z=33 compressed payloads).
pub fn inflate(data: &[u8]) -> Result<Vec<u8>> {
    use std::io::Read;
    let mut out = Vec::with_capacity(data.len() * 3);
    flate2::read::ZlibDecoder::new(data)
        .read_to_end(&mut out)
        .context("zlib inflate")?;
    Ok(out)
}

/// Scale RGBA image to target size with nearest-neighbour (media is already screen-sized).
pub fn scale_nearest(src: &Image, w: u32, h: u32) -> Image {
    if src.w == w && src.h == h {
        return src.clone();
    }
    let mut out = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        let sy = (u64::from(y) * u64::from(src.h) / u64::from(h)) as u32;
        for x in 0..w {
            let sx = (u64::from(x) * u64::from(src.w) / u64::from(w)) as u32;
            let si = ((sy * src.w + sx) * 4) as usize;
            let di = ((y * w + x) * 4) as usize;
            out[di..di + 4].copy_from_slice(&src.rgba[si..si + 4]);
        }
    }
    Image::from_rgba(w, h, out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_gif() -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut enc = gif::Encoder::new(&mut out, 2, 2, &[]).unwrap();
            let mut f1 = gif::Frame::from_rgba_speed(
                2,
                2,
                &mut [
                    255, 0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 255,
                ],
                1,
            );
            f1.delay = 7;
            enc.write_frame(&f1).unwrap();
            let mut f2 = gif::Frame::from_rgba_speed(
                2,
                2,
                &mut [
                    0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 255, 255,
                ],
                1,
            );
            f2.delay = 0;
            enc.write_frame(&f2).unwrap();
        }
        out
    }

    #[test]
    fn gif_decodes_two_frames_with_delays() {
        let frames = decode_gif(&make_gif()).unwrap();
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].delay, Duration::from_millis(70));
        assert_eq!(frames[1].delay, Duration::from_millis(100)); // 0 → 100ms
        assert_eq!(&frames[0].image.rgba[0..4], &[255, 0, 0, 255]);
        assert_eq!(&frames[1].image.rgba[0..4], &[0, 0, 255, 255]);
        assert_eq!((frames[0].image.w, frames[0].image.h), (2, 2));
    }

    #[test]
    fn png_roundtrip() {
        let mut buf = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut buf, 2, 1);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            let mut w = enc.write_header().unwrap();
            w.write_image_data(&[10, 20, 30, 40, 50, 60, 70, 80])
                .unwrap();
        }
        let img = decode_png(&buf).unwrap();
        assert_eq!((img.w, img.h), (2, 1));
        assert_eq!(&img.rgba[..], &[10, 20, 30, 40, 50, 60, 70, 80]);
        match decode_any(&buf).unwrap() {
            Decoded::Static(i) => assert_eq!(i.w, 2),
            _ => panic!("expected static"),
        }
        match decode_any(&make_gif()).unwrap() {
            Decoded::Animated(f) => assert_eq!(f.len(), 2),
            _ => panic!("expected animated"),
        }
    }

    #[test]
    fn zlib_inflate_roundtrip() {
        use std::io::Write;
        let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(b"terminal media payload").unwrap();
        let z = enc.finish().unwrap();
        assert_eq!(inflate(&z).unwrap(), b"terminal media payload");
    }

    #[test]
    fn unsupported_format_rejected() {
        assert!(decode_any(b"BM_not_an_image").is_err());
    }
}
