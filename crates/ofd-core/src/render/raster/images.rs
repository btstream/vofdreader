use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use resvg::tiny_skia as sk;

/// Decoded images shared across pages (templates and seals repeat).
#[derive(Default)]
pub(crate) struct ImageCache {
    entries: HashMap<(u64, usize), Arc<sk::Pixmap>>,
    bytes: usize,
    order: VecDeque<(u64, usize)>,
}

impl ImageCache {
    pub(crate) fn set_limit(&mut self, limit: usize) {
        while self.bytes > limit {
            self.evict_oldest();
        }
    }

    fn evict_oldest(&mut self) {
        if let Some(key) = self.order.pop_front()
            && let Some(image) = self.entries.remove(&key)
        {
            self.bytes = self.bytes.saturating_sub(image.data().len());
        }
    }

    pub(crate) fn get_or_decode(
        &mut self,
        data: &[u8],
        limit: &AtomicUsize,
    ) -> Option<Arc<sk::Pixmap>> {
        self.set_limit(limit.load(Ordering::Relaxed));
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        data.hash(&mut hasher);
        let key = (hasher.finish(), data.len());
        if let Some(image) = self.entries.get(&key) {
            self.order.retain(|entry| *entry != key);
            self.order.push_back(key);
            return Some(image.clone());
        }
        let image = Arc::new(decode_raster(data)?);
        let size = image.data().len();
        let limit = limit.load(Ordering::Relaxed);
        self.set_limit(limit);
        // Decode is still necessary to paint a large source, but an oversized
        // result must not defeat the retention limit or flush useful entries.
        if size > limit {
            return Some(image);
        }
        while size > limit.saturating_sub(self.bytes) {
            self.evict_oldest();
        }
        self.bytes += size;
        self.entries.insert(key, image.clone());
        self.order.push_back(key);
        Some(image)
    }
}

/// Decode by content, as resvg would for a correctly labelled data URL.
pub(crate) fn decode_raster(data: &[u8]) -> Option<sk::Pixmap> {
    if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        sk::Pixmap::decode_png(data).ok()
    } else if data.starts_with(&[0xFF, 0xD8]) {
        decode_jpeg(data)
    } else if data.starts_with(b"GIF8") {
        decode_gif(data)
    } else if data.len() > 12 && &data[0..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        decode_webp(data)
    } else {
        None
    }
}

pub(crate) fn decode_jpeg(data: &[u8]) -> Option<sk::Pixmap> {
    use zune_jpeg::zune_core::colorspace::ColorSpace;
    use zune_jpeg::zune_core::options::DecoderOptions;
    let options = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGBA);
    let mut decoder = zune_jpeg::JpegDecoder::new_with_options(std::io::Cursor::new(data), options);
    decoder.decode_headers().ok()?;
    if decoder.output_colorspace()? != ColorSpace::RGBA {
        return None;
    }
    let pixels = decoder.decode().ok()?;
    let info = decoder.info()?;
    let size = sk::IntSize::from_wh(u32::from(info.width), u32::from(info.height))?;
    sk::Pixmap::from_vec(pixels, size)
}

pub(crate) fn decode_gif(data: &[u8]) -> Option<sk::Pixmap> {
    let mut options = gif::DecodeOptions::new();
    options.set_color_output(gif::ColorOutput::RGBA);
    let mut decoder = options.read_info(data).ok()?;
    let frame = decoder.read_next_frame().ok()??;
    let mut pixmap = sk::Pixmap::new(u32::from(frame.width), u32::from(frame.height))?;
    premultiply_rgba(&frame.buffer, pixmap.data_mut());
    Some(pixmap)
}

pub(crate) fn decode_webp(data: &[u8]) -> Option<sk::Pixmap> {
    let mut decoder = image_webp::WebPDecoder::new(std::io::Cursor::new(data)).ok()?;
    let mut frame = vec![0; decoder.output_buffer_size()?];
    decoder.read_image(&mut frame).ok()?;
    let (width, height) = decoder.dimensions();
    let mut pixmap = sk::Pixmap::new(width, height)?;
    if decoder.has_alpha() {
        premultiply_rgba(&frame, pixmap.data_mut());
    } else {
        for (dst, src) in pixmap
            .data_mut()
            .chunks_exact_mut(4)
            .zip(frame.chunks_exact(3))
        {
            dst[..3].copy_from_slice(src);
            dst[3] = 255;
        }
    }
    Some(pixmap)
}

pub(crate) fn premultiply_rgba(src: &[u8], dst: &mut [u8]) {
    for (dst, src) in dst.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
        let alpha = f64::from(src[3]) / 255.0;
        dst[0] = (f64::from(src[0]) * alpha + 0.5) as u8;
        dst[1] = (f64::from(src[1]) * alpha + 0.5) as u8;
        dst[2] = (f64::from(src[2]) * alpha + 0.5) as u8;
        dst[3] = src[3];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(side: u32, value: u8) -> Vec<u8> {
        let mut pixmap = sk::Pixmap::new(side, side).unwrap();
        pixmap.fill(sk::Color::from_rgba8(value, 0, 0, 255));
        pixmap.encode_png().unwrap()
    }

    #[test]
    fn decoded_image_cache_evicts_lru_without_flushing_everything() {
        let mut cache = ImageCache::default();
        let limit = AtomicUsize::new(32);
        let (a, b, c) = (png(2, 1), png(2, 2), png(2, 3));
        let first = cache.get_or_decode(&a, &limit).unwrap();
        let second = cache.get_or_decode(&b, &limit).unwrap();
        assert!(Arc::ptr_eq(
            &first,
            &cache.get_or_decode(&a, &limit).unwrap()
        ));
        cache.get_or_decode(&c, &limit).unwrap();
        assert_eq!(cache.bytes, 32);
        assert_eq!(cache.entries.len(), 2);
        assert!(Arc::ptr_eq(
            &first,
            &cache.get_or_decode(&a, &limit).unwrap()
        ));
        assert!(!Arc::ptr_eq(
            &second,
            &cache.get_or_decode(&b, &limit).unwrap()
        ));
        limit.store(0, Ordering::Relaxed);
        cache.get_or_decode(&a, &limit).unwrap();
        assert_eq!(cache.bytes, 0);
        assert!(cache.entries.is_empty());
        assert_eq!(
            first.data().len(),
            16,
            "an active painter may still own an evicted source"
        );
    }

    #[test]
    fn oversized_decoded_source_is_painted_but_not_cached() {
        let mut cache = ImageCache::default();
        let limit = AtomicUsize::new(16);
        let small = png(1, 1);
        let kept = cache.get_or_decode(&small, &limit).unwrap();
        let large = cache.get_or_decode(&png(4, 2), &limit).unwrap();
        assert_eq!(large.data().len(), 64);
        assert_eq!(cache.bytes, 4);
        assert!(Arc::ptr_eq(
            &kept,
            &cache.get_or_decode(&small, &limit).unwrap()
        ));
        cache.set_limit(0);
        assert!(cache.order.is_empty());
        assert_eq!(cache.bytes, 0);
    }
}
