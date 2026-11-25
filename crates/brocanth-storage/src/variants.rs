//! Deriving an image variant, which is the only part of this crate that needs
//! a codec.
//!
//! Behind the `variants` feature, and §8.2 is the reason rather than taste:
//! "a feature that is off must mean the code is absent". `image` brings PNG,
//! JPEG, GIF and WebP decoders — a large parsing surface pointed straight at
//! bytes a stranger uploaded — and an application that stores invoices must
//! not link any of it. [`VariantSpec`] parses with the feature off,
//! [`Storage::get_variant`] serves a cached variant with the feature off, and
//! only the pixels are gated.
//!
//! # Decompression bombs: the cap is before the decode
//!
//! A fifty kilobyte PNG can declare 40 000 by 40 000. Decoded that is 6.4
//! gigabytes of RGBA, allocated before a single resize instruction runs, and
//! the request that did it cost the attacker one HTTP POST. So:
//!
//! 1. The media type is sniffed from the magic bytes. A non-image never
//!    reaches a decoder.
//! 2. The *header* is read for the declared dimensions and they are checked
//!    against [`MAX_SOURCE_DIM`] and
//!    [`MAX_SOURCE_PIXELS`]. This is a header
//!    parse; nothing is decoded.
//! 3. Only then is the image decoded, and `image`'s own [`Limits`] are set as
//!    well, so a file that lies in its header is stopped by the decoder's
//!    allocator budget instead.
//!
//! The requested size is capped separately and much earlier, at
//! [`VariantSpec::parse`](crate::spec::VariantSpec::parse).
//!
//! # Computed once
//!
//! [`derive_variant`] asks the store for the cached variant first and writes it back
//! after rendering. Two simultaneous requests for a cold thumbnail may both
//! render it — there is no lock, because a lock here is a distributed lock the
//! moment there are two instances — but both produce the same bytes and the
//! backend's write is atomic, so a reader sees one or the other and never a
//! fragment. The cost of the race is one wasted resize; the cost of avoiding
//! it would be a coordination service.
//!
//! # Off the reactor, a few at a time
//!
//! A decode of a large source and its resize take tens or hundreds of
//! milliseconds of CPU and up to [`MAX_DECODE_ALLOC`] bytes. [`derive_variant`]
//! runs [`render`] on Tokio's blocking pool, so a request thread never waits
//! on it, and at most [`MAX_CONCURRENT_RENDERS`] renders run at once: a
//! burst of cold thumbnails waits its turn instead of taking every blocking
//! thread and gigabytes of memory.
//!
//! # What the result is encoded as
//!
//! The format the spec names, when it names one ([`VariantSpec::with_format`],
//! `&fmt=` on the routes). Otherwise JPEG at quality 82 when the source has no alpha channel, PNG when it has.
//! A variant is derived data and its format is ours to choose: a photograph
//! re-encoded as PNG is often larger than the original it was meant to make
//! cheaper, and a logo re-encoded as JPEG loses the transparency it was
//! uploaded for. An animated GIF yields a still first frame.

use image::imageops::FilterType;
use image::{DynamicImage, ImageFormat, ImageReader, Limits};
use std::io::Cursor;
use std::sync::OnceLock;

use tokio::sync::Semaphore;

use crate::error::StorageError;
use crate::key::ObjectKey;
use crate::meta::ContentType;
use crate::spec::{CropAnchor, VariantFormat, VariantMode, VariantSpec};
use crate::storage::Storage;
use crate::{MAX_SOURCE_DIM, MAX_SOURCE_PIXELS};

/// How many bytes the decoder may allocate: 256 MiB.
const MAX_DECODE_ALLOC: u64 = 256 * 1024 * 1024;

/// How many renders run at once, in this process. Each may hold up to
/// [`MAX_DECODE_ALLOC`] bytes, so this also bounds the memory renders take.
pub const MAX_CONCURRENT_RENDERS: usize = 4;

/// The renders' turns. See [`MAX_CONCURRENT_RENDERS`].
fn turns() -> &'static Semaphore {
    static TURNS: OnceLock<Semaphore> = OnceLock::new();
    TURNS.get_or_init(|| Semaphore::new(MAX_CONCURRENT_RENDERS))
}

/// The quality a derived JPEG is encoded at.
const JPEG_QUALITY: u8 = 82;

/// A derived image and what to serve it as.
#[derive(Clone, Debug)]
pub struct Variant {
    /// The encoded image.
    pub bytes: Vec<u8>,
    /// `image/jpeg`, `image/png` or `image/webp`. Always inline-safe.
    pub content_type: ContentType,
}

/// Reads a variant from the cache, or renders it and caches it.
///
/// Takes `&dyn Storage` rather than a generic, which is both the proof that
/// the trait is object-safe and what lets an application hold one backend
/// behind an `Arc`.
///
/// # Errors
/// [`StorageError::NotFound`] when the object is not stored,
/// [`StorageError::NotAnImage`] when it is not one,
/// [`StorageError::ImageTooLarge`] when it declares more pixels than
/// [`MAX_SOURCE_PIXELS`], and [`StorageError::Image`] when a codec refused the
/// work.
pub async fn derive_variant(
    store: &dyn Storage,
    key: &ObjectKey,
    spec: VariantSpec,
) -> Result<Variant, StorageError> {
    derive_variant_with(store, key, spec, MAX_SOURCE_PIXELS).await
}

/// [`derive_variant`], with a lower cap on the pixels a source may declare.
///
/// `max_pixels` above [`MAX_SOURCE_PIXELS`] is held to it: the decoder's
/// memory budget is sized for that cap. A variant already in the cache is
/// served whatever its source declared, because it was checked when it was
/// rendered.
///
/// # Errors
/// See [`derive_variant`].
pub async fn derive_variant_with(
    store: &dyn Storage,
    key: &ObjectKey,
    spec: VariantSpec,
    max_pixels: u64,
) -> Result<Variant, StorageError> {
    if let Some(bytes) = store.get_variant(key, spec).await? {
        // The cached bytes are ours, so sniffing them is reading back what we
        // wrote rather than trusting a stranger.
        let content_type = ContentType::sniff(&bytes).unwrap_or_else(ContentType::octet_stream);
        return Ok(Variant {
            bytes,
            content_type,
        });
    }
    let source = store.get(key).await?;
    // The semaphore is never closed, so `acquire` cannot fail.
    let _turn = turns()
        .acquire()
        .await
        .map_err(|e| StorageError::backend(format!("waiting for a render: {e}")))?;
    let variant = crate::local::offload(move || render_with(&source, spec, max_pixels)).await?;
    store.put_variant(key, spec, &variant.bytes).await?;
    Ok(variant)
}

/// Renders a variant, with no store and no cache.
///
/// # Errors
/// See [`derive_variant`].
pub fn render(source: &[u8], spec: VariantSpec) -> Result<Variant, StorageError> {
    render_with(source, spec, MAX_SOURCE_PIXELS)
}

/// [`render`], with a lower cap on the pixels the source may declare. See
/// [`derive_variant_with`].
///
/// # Errors
/// See [`derive_variant`].
pub fn render_with(
    source: &[u8],
    spec: VariantSpec,
    max_pixels: u64,
) -> Result<Variant, StorageError> {
    let max_pixels = max_pixels.min(MAX_SOURCE_PIXELS);
    let declared = ContentType::sniff(source).ok_or(StorageError::NotAnImage)?;
    if !declared.is_image() {
        return Err(StorageError::NotAnImage);
    }

    // Step one: the header only. `into_dimensions` parses the size field and
    // stops; it does not decode a pixel, so a bomb is refused for the price of
    // reading a few dozen bytes.
    let (width, height) = reader(source)?
        .into_dimensions()
        .map_err(|e| StorageError::Image(e.to_string()))?;
    if width > MAX_SOURCE_DIM
        || height > MAX_SOURCE_DIM
        || u64::from(width) * u64::from(height) > max_pixels
    {
        return Err(StorageError::ImageTooLarge { width, height });
    }

    // Step two: decode, with the decoder's own budget set as well, because a
    // header can lie and a truncated stream can claim to be small.
    let mut reader = reader(source)?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_SOURCE_DIM);
    limits.max_image_height = Some(MAX_SOURCE_DIM);
    limits.max_alloc = Some(MAX_DECODE_ALLOC);
    reader.limits(limits);
    let image = reader
        .decode()
        .map_err(|e| StorageError::Image(e.to_string()))?;

    let resized = resize(&image, spec);
    match spec.format() {
        None => encode(&resized),
        Some(format) => encode_as(&resized, format),
    }
}

fn reader(source: &[u8]) -> Result<ImageReader<Cursor<&[u8]>>, StorageError> {
    ImageReader::new(Cursor::new(source))
        .with_guessed_format()
        .map_err(|e| StorageError::Image(e.to_string()))
}

fn resize(image: &DynamicImage, spec: VariantSpec) -> DynamicImage {
    // Lanczos3 because a thumbnail is computed once and looked at many times,
    // so the extra milliseconds buy quality on every later request.
    const FILTER: FilterType = FilterType::Lanczos3;
    let (sw, sh) = (image.width().max(1), image.height().max(1));
    match spec.mode() {
        VariantMode::Fit => image.resize(spec.width(), spec.height(), FILTER),
        VariantMode::Scale => {
            let (w, h) = if spec.width() == 0 {
                let w = scale_dim(sw, spec.height(), sh);
                (w, spec.height())
            } else {
                let h = scale_dim(sh, spec.width(), sw);
                (spec.width(), h)
            };
            image.resize_exact(w, h, FILTER)
        }
        VariantMode::Crop(anchor) => {
            let (tw, th) = (spec.width(), spec.height());
            // Cover the target box on both axes, then take the window the
            // anchor asks for. `resize_to_fill` is exactly this with a centred
            // window, so the centre case defers to it.
            if anchor == CropAnchor::Centre {
                return image.resize_to_fill(tw, th, FILTER);
            }
            let scale = f64::from(tw) / f64::from(sw);
            let scale = scale.max(f64::from(th) / f64::from(sh));
            let cw = ((f64::from(sw) * scale).ceil() as u32).max(tw);
            let ch = ((f64::from(sh) * scale).ceil() as u32).max(th);
            let covered = image.resize_exact(cw, ch, FILTER);
            let x = (cw.saturating_sub(tw)) / 2;
            let y = match anchor {
                CropAnchor::Top => 0,
                CropAnchor::Bottom => ch.saturating_sub(th),
                CropAnchor::Centre => (ch.saturating_sub(th)) / 2,
            };
            covered.crop_imm(x, y, tw, th)
        }
    }
}

/// `source * target / other`, rounded, and never zero: a one pixel wide image
/// is still an image, a zero pixel wide one is a decoder error.
fn scale_dim(source: u32, target: u32, other: u32) -> u32 {
    let other = u64::from(other.max(1));
    let value = (u64::from(source) * u64::from(target) + other / 2) / other;
    (value.max(1).min(u64::from(MAX_SOURCE_DIM))) as u32
}

/// Encodes in the format the request named with `&fmt=`.
///
/// JPEG drops the alpha channel. WebP is lossless, so it is written from
/// 8-bit RGB or RGBA, the two layouts its encoder takes.
fn encode_as(image: &DynamicImage, format: VariantFormat) -> Result<Variant, StorageError> {
    let mut bytes = Vec::new();
    let failed = |e: image::ImageError| StorageError::Image(e.to_string());
    match format {
        VariantFormat::Jpeg => {
            let rgb = image.to_rgb8();
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, JPEG_QUALITY)
                .encode_image(&rgb)
                .map_err(failed)?;
        }
        VariantFormat::Png => image
            .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
            .map_err(failed)?,
        VariantFormat::WebP => {
            let layout = if image.color().has_alpha() {
                DynamicImage::ImageRgba8(image.to_rgba8())
            } else {
                DynamicImage::ImageRgb8(image.to_rgb8())
            };
            layout
                .write_to(&mut Cursor::new(&mut bytes), ImageFormat::WebP)
                .map_err(failed)?;
        }
    }
    Ok(Variant {
        bytes,
        content_type: ContentType::parse(format.media_type()).expect("a literal media type"),
    })
}

fn encode(image: &DynamicImage) -> Result<Variant, StorageError> {
    let mut bytes = Vec::new();
    if image.color().has_alpha() {
        image
            .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
            .map_err(|e| StorageError::Image(e.to_string()))?;
        Ok(Variant {
            bytes,
            content_type: ContentType::parse("image/png").expect("a literal media type"),
        })
    } else {
        let rgb = image.to_rgb8();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, JPEG_QUALITY)
            .encode_image(&rgb)
            .map_err(|e| StorageError::Image(e.to_string()))?;
        Ok(Variant {
            bytes,
            content_type: ContentType::parse("image/jpeg").expect("a literal media type"),
        })
    }
}
