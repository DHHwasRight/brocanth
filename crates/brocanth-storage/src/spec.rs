//! The size specification of a derived image, parsed with no image library in
//! the tree.
//!
//! PocketBase's thumb syntax, because it is terse, it is in URLs people have
//! already learned, and there is nothing to improve about it:
//!
//! | Spec     | Meaning                                              |
//! |----------|------------------------------------------------------|
//! | `WxH`    | crop to exactly W by H, anchored at the centre        |
//! | `WxHt`   | crop to exactly W by H, anchored at the top           |
//! | `WxHb`   | crop to exactly W by H, anchored at the bottom        |
//! | `WxHf`   | fit inside W by H, aspect preserved, nothing cropped  |
//! | `0xH`    | scale to height H, width follows the aspect ratio     |
//! | `Wx0`    | scale to width W, height follows the aspect ratio     |
//!
//! # Why this parses with the `variants` feature off
//!
//! §8.2: a feature that is off means the code is absent. The pixels need
//! `image`; a URL segment does not. Keeping the parser here means a router can
//! route `/files/{key}/{spec}`, a cache can key on a spec, and a build with no
//! codecs in it can still answer "that is not a size" with a 400 rather than
//! with a panic or a 500.
//!
//! # Two limits that are security rather than taste
//!
//! * **Dimensions are capped at parse time** ([`MAX_VARIANT_DIM`]). A request
//!   for `60000x60000` is refused before anything allocates fourteen gigabytes
//!   to satisfy it. The cap is on the *request*; the cap on what the source
//!   image may declare is separate and lives in
//!   [`MAX_SOURCE_PIXELS`](crate::MAX_SOURCE_PIXELS).
//! * **One spelling per variant.** `007x100` and `7x100` would name the same
//!   image, and a cache keyed on the text would store it twice — or ten
//!   thousand times, which is a cheap way to fill a disk with an image that
//!   was computed once. So leading zeros, `+`, whitespace and uppercase
//!   suffixes are refused, and [`VariantSpec`]'s `Display` is the canonical
//!   spelling the cache path uses.

use crate::error::StorageError;

/// The largest width or height a derived variant may ask for.
///
/// Four thousand ninety-six covers a retina hero image and refuses a request
/// whose only purpose is to make the server allocate.
pub const MAX_VARIANT_DIM: u32 = 4096;

/// Where a crop keeps the pixels it keeps.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CropAnchor {
    /// The middle of the image. A face in a portrait usually survives.
    Centre,
    /// The top edge. What a headshot wants.
    Top,
    /// The bottom edge.
    Bottom,
}

/// How the derived image relates to the source.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum VariantMode {
    /// `WxH`, `WxHt`, `WxHb` — the result is exactly W by H and some of the
    /// source is discarded.
    Crop(CropAnchor),
    /// `WxHf` — the result fits inside W by H and may be smaller on one axis.
    Fit,
    /// `Wx0` or `0xH` — one axis is fixed, the other follows the aspect ratio.
    Scale,
}

/// A parsed size specification.
///
/// The fields are private and every constructor checks the same invariants, so
/// a `VariantSpec` that exists is one this crate will render: both dimensions
/// are within [`MAX_VARIANT_DIM`], exactly one of them is zero when the mode is
/// [`VariantMode::Scale`] and neither is zero otherwise.
///
/// It is also a path component, through `Display`, which is safe for the same
/// reason an [`ObjectKey`](crate::key::ObjectKey) is: the text is rendered
/// from two integers and a letter from a fixed set, never echoed from input.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VariantSpec {
    width: u32,
    height: u32,
    mode: VariantMode,
}

impl VariantSpec {
    /// Builds a specification, checking the same invariants
    /// [`VariantSpec::parse`] does.
    ///
    /// # Errors
    /// Returns [`StorageError::InvalidSpec`] when a dimension is zero in the
    /// wrong place, both are zero, or either is over [`MAX_VARIANT_DIM`].
    pub fn new(width: u32, height: u32, mode: VariantMode) -> Result<Self, StorageError> {
        if width > MAX_VARIANT_DIM || height > MAX_VARIANT_DIM {
            return Err(StorageError::InvalidSpec("a dimension is past the limit"));
        }
        match mode {
            VariantMode::Scale => {
                if (width == 0) == (height == 0) {
                    return Err(StorageError::InvalidSpec(
                        "a scale fixes exactly one dimension",
                    ));
                }
            }
            VariantMode::Crop(_) | VariantMode::Fit => {
                if width == 0 || height == 0 {
                    return Err(StorageError::InvalidSpec(
                        "a crop or a fit needs both dimensions",
                    ));
                }
            }
        }
        Ok(Self {
            width,
            height,
            mode,
        })
    }

    /// Parses `WxH`, `WxHt`, `WxHb`, `WxHf`, `0xH` or `Wx0`.
    ///
    /// # Errors
    /// Returns [`StorageError::InvalidSpec`] for anything else, including a
    /// non-canonical spelling of something valid. The reason is a fixed string
    /// and never repeats the input.
    pub fn parse(text: &str) -> Result<Self, StorageError> {
        if text.len() > 12 {
            return Err(StorageError::InvalidSpec("too long to be a size"));
        }
        let (body, mode_letter) = match text.as_bytes().last() {
            Some(b't') => (&text[..text.len() - 1], Some(b't')),
            Some(b'b') => (&text[..text.len() - 1], Some(b'b')),
            Some(b'f') => (&text[..text.len() - 1], Some(b'f')),
            _ => (text, None),
        };
        let (w, h) = body
            .split_once('x')
            .ok_or(StorageError::InvalidSpec("expected WIDTHxHEIGHT"))?;
        let width = parse_dim(w)?;
        let height = parse_dim(h)?;
        let scaling = width == 0 || height == 0;
        let mode = match (mode_letter, scaling) {
            (None, true) => VariantMode::Scale,
            (None, false) => VariantMode::Crop(CropAnchor::Centre),
            (Some(b't'), false) => VariantMode::Crop(CropAnchor::Top),
            (Some(b'b'), false) => VariantMode::Crop(CropAnchor::Bottom),
            (Some(b'f'), false) => VariantMode::Fit,
            // `0x100f` and `100x0t` are refused rather than quietly treated as
            // `0x100`. There is no crop and no fit to perform when one axis is
            // free, so the suffix means the caller expected something this
            // cannot do, and a silent reinterpretation is a surprise cached
            // under the caller's own URL.
            (Some(_), true) => {
                return Err(StorageError::InvalidSpec(
                    "a free dimension takes no crop or fit suffix",
                ))
            }
            (Some(_), false) => unreachable!("the suffix set is closed above"),
        };
        Self::new(width, height, mode)
    }

    /// The requested width. Zero means "follow the aspect ratio".
    pub fn width(self) -> u32 {
        self.width
    }

    /// The requested height. Zero means "follow the aspect ratio".
    pub fn height(self) -> u32 {
        self.height
    }

    /// How the result relates to the source.
    pub fn mode(self) -> VariantMode {
        self.mode
    }
}

fn parse_dim(text: &str) -> Result<u32, StorageError> {
    if text.is_empty() || text.len() > 4 {
        return Err(StorageError::InvalidSpec(
            "a dimension is one to four digits",
        ));
    }
    if !text.bytes().all(|b| b.is_ascii_digit()) {
        return Err(StorageError::InvalidSpec("a dimension is digits only"));
    }
    // `0` is a dimension; `00` and `0100` are not, because they spell a number
    // that already has a spelling.
    if text.len() > 1 && text.starts_with('0') {
        return Err(StorageError::InvalidSpec("a dimension has no leading zero"));
    }
    text.parse()
        .map_err(|_| StorageError::InvalidSpec("a dimension is past the limit"))
}

impl std::fmt::Display for VariantSpec {
    /// The canonical spelling, which is also the cache path component.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let suffix = match self.mode {
            VariantMode::Crop(CropAnchor::Centre) | VariantMode::Scale => "",
            VariantMode::Crop(CropAnchor::Top) => "t",
            VariantMode::Crop(CropAnchor::Bottom) => "b",
            VariantMode::Fit => "f",
        };
        write!(f, "{}x{}{}", self.width, self.height, suffix)
    }
}

impl std::fmt::Debug for VariantSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "VariantSpec({self})")
    }
}

impl std::str::FromStr for VariantSpec {
    type Err = StorageError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}
