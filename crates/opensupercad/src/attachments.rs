//! Images attached to an agent prompt: pasted, dropped or picked with
//! *Attach…*. They're checked, downscaled when large, and saved with the
//! project's data, so the thread can show them again later.

use std::io::Cursor;
use std::path::{Path, PathBuf};

use image::{GenericImageView, ImageFormat};

/// Longest edge sent to agents. Larger images are scaled down: models see
/// no more detail than this, and it keeps prompts small.
pub const MAX_EDGE: u32 = 1568;

/// Files bigger than this are re-encoded even when small enough in pixels.
const MAX_BYTES: usize = 3_750_000;

/// The MIME type agents get for a saved attachment, by extension.
pub fn mime_for(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ => return None,
    })
}

/// Check that `bytes` is a PNG, JPEG, GIF or WebP image and scale it down
/// if needed. Returns the bytes to save and their file extension. Blocking.
pub fn prepare(bytes: &[u8]) -> anyhow::Result<(Vec<u8>, &'static str)> {
    let format = image::guess_format(bytes)
        .map_err(|_| anyhow::anyhow!("not an image OpenSuperCAD can read"))?;
    let ext = match format {
        ImageFormat::Png => "png",
        ImageFormat::Jpeg => "jpg",
        ImageFormat::Gif => "gif",
        ImageFormat::WebP => "webp",
        other => anyhow::bail!(
            "{} images can't be sent to agents; use PNG, JPEG, GIF or WebP",
            other.extensions_str().first().unwrap_or(&"these")
        ),
    };
    let image = image::load_from_memory_with_format(bytes, format)?;
    let (w, h) = image.dimensions();
    if w.max(h) <= MAX_EDGE && bytes.len() <= MAX_BYTES {
        return Ok((bytes.to_vec(), ext));
    }
    let image = if w.max(h) > MAX_EDGE {
        image.resize(MAX_EDGE, MAX_EDGE, image::imageops::FilterType::Triangle)
    } else {
        image
    };
    let mut out = Cursor::new(Vec::new());
    // Photos stay compact as JPEG; anything with transparency stays PNG.
    if image.color().has_alpha() {
        image.write_to(&mut out, ImageFormat::Png)?;
        Ok((out.into_inner(), "png"))
    } else {
        image.to_rgb8().write_to(&mut out, ImageFormat::Jpeg)?;
        Ok((out.into_inner(), "jpg"))
    }
}

/// Save a prepared image into `dir` under a fresh name.
pub fn save(dir: &Path, bytes: &[u8], ext: &str) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let path = dir.join(format!("{stamp}.{ext}"));
    std::fs::write(&path, bytes)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(w: u32, h: u32, alpha: bool) -> Vec<u8> {
        let mut out = Cursor::new(Vec::new());
        if alpha {
            image::RgbaImage::from_pixel(w, h, image::Rgba([1, 2, 3, 128]))
                .write_to(&mut out, ImageFormat::Png)
                .unwrap();
        } else {
            image::RgbImage::from_pixel(w, h, image::Rgb([200, 10, 10]))
                .write_to(&mut out, ImageFormat::Png)
                .unwrap();
        }
        out.into_inner()
    }

    #[test]
    fn small_images_are_kept_as_they_are() {
        let bytes = png(40, 30, false);
        let (out, ext) = prepare(&bytes).unwrap();
        assert_eq!((out, ext), (bytes, "png"));
    }

    #[test]
    fn large_images_are_scaled_down() {
        let (out, ext) = prepare(&png(4000, 1000, false)).unwrap();
        assert_eq!(ext, "jpg");
        let img = image::load_from_memory(&out).unwrap();
        assert_eq!(img.dimensions(), (MAX_EDGE, MAX_EDGE / 4));
        // Transparency survives as PNG.
        let (out, ext) = prepare(&png(2000, 2000, true)).unwrap();
        assert_eq!(ext, "png");
        assert_eq!(
            image::load_from_memory(&out).unwrap().dimensions(),
            (MAX_EDGE, MAX_EDGE)
        );
    }

    #[test]
    fn rejects_what_agents_cant_read() {
        assert!(prepare(b"not an image").is_err());
        let mut bmp = Cursor::new(Vec::new());
        image::RgbImage::new(2, 2)
            .write_to(&mut bmp, ImageFormat::Bmp)
            .unwrap();
        assert!(prepare(&bmp.into_inner()).is_err());
    }

    #[test]
    fn mime_types_by_extension() {
        assert_eq!(mime_for(Path::new("a/b.JPG")), Some("image/jpeg"));
        assert_eq!(mime_for(Path::new("x.webp")), Some("image/webp"));
        assert_eq!(mime_for(Path::new("x.bmp")), None);
        let dir = tempfile::tempdir().unwrap();
        let saved = save(dir.path(), b"x", "png").unwrap();
        assert_eq!(mime_for(&saved), Some("image/png"));
        assert_eq!(std::fs::read(saved).unwrap(), b"x");
    }
}
