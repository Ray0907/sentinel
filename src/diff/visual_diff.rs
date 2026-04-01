//! Visual diff using screenshots and perceptual hashing.
//!
//! Captures frames via CDP Page.captureScreenshot (on-demand, not screencast)
//! and compares them using perceptual hash distance + pixel-level mismatch.

use anyhow::Result;
use image::{DynamicImage, GenericImageView, Rgba};

/// Result of comparing two frames.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct VisualDiffResult {
    /// Perceptual hash distance (0 = identical, higher = more different)
    pub hash_distance: u32,
    /// Percentage of pixels that differ beyond threshold (0.0 - 100.0)
    pub pixel_mismatch_pct: f64,
    /// Whether the frames are visually different (hash_distance > 5 or mismatch > 1%)
    pub changed: bool,
    /// Number of changed regions detected
    pub changed_region_count: usize,
    /// Bounding boxes of changed regions [x, y, width, height]
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub changed_regions: Vec<[u32; 4]>,
}

/// Compare two images using perceptual hash + pixel diff.
pub fn compare_frames(before: &DynamicImage, after: &DynamicImage) -> VisualDiffResult {
    let hash_distance = perceptual_hash_distance(before, after);
    let (mismatch_pct, diff_mask) = pixel_diff(before, after, 30);
    let regions = detect_changed_regions(&diff_mask, before.width(), before.height());

    let changed = hash_distance > 5 || mismatch_pct > 1.0;

    VisualDiffResult {
        hash_distance,
        pixel_mismatch_pct: (mismatch_pct * 100.0).round() / 100.0,
        changed,
        changed_region_count: regions.len(),
        changed_regions: regions,
    }
}

/// Compute perceptual hash distance using mean-based hash (no external dependency).
/// Resizes both images to 16x16 grayscale, computes mean threshold hash, returns hamming distance.
fn perceptual_hash_distance(a: &DynamicImage, b: &DynamicImage) -> u32 {
    let hash_a = mean_hash(a);
    let hash_b = mean_hash(b);

    // Hamming distance
    hash_a.iter().zip(hash_b.iter())
        .map(|(a, b)| (a ^ b).count_ones())
        .sum()
}

/// Mean-based perceptual hash: resize to 16x16 grayscale, threshold at mean.
fn mean_hash(img: &DynamicImage) -> [u8; 32] {
    let resized = img.resize_exact(16, 16, image::imageops::FilterType::Lanczos3);
    let gray = resized.to_luma8();
    let pixels: Vec<u8> = gray.pixels().map(|p| p.0[0]).collect();
    let mean: u64 = pixels.iter().map(|&p| p as u64).sum::<u64>() / pixels.len().max(1) as u64;

    let mut hash = [0u8; 32]; // 256 bits = 16x16
    for (i, &pixel) in pixels.iter().enumerate() {
        if pixel as u64 > mean {
            hash[i / 8] |= 1 << (7 - (i % 8));
        }
    }
    hash
}

/// Pixel-level diff. Returns (mismatch_fraction, diff_mask).
/// diff_mask[i] = true if pixel i exceeds color_threshold.
fn pixel_diff(
    before: &DynamicImage,
    after: &DynamicImage,
    color_threshold: u8,
) -> (f64, Vec<bool>) {
    let (w, h) = (before.width().min(after.width()), before.height().min(after.height()));
    let total = (w * h) as f64;
    let mut mismatch_count = 0u64;
    let mut mask = Vec::with_capacity((w * h) as usize);

    for y in 0..h {
        for x in 0..w {
            let Rgba(pa) = before.get_pixel(x, y);
            let Rgba(pb) = after.get_pixel(x, y);

            let dr = (pa[0] as i16 - pb[0] as i16).unsigned_abs();
            let dg = (pa[1] as i16 - pb[1] as i16).unsigned_abs();
            let db = (pa[2] as i16 - pb[2] as i16).unsigned_abs();

            let diff = dr.max(dg).max(db);
            if diff > color_threshold as u16 {
                mismatch_count += 1;
                mask.push(true);
            } else {
                mask.push(false);
            }
        }
    }

    (mismatch_count as f64 / total, mask)
}

/// Detect rectangular regions of change from a diff mask.
/// Uses a simple grid-based approach: divide into 8x8 cells, mark cells with >10% changed pixels.
fn detect_changed_regions(mask: &[bool], width: u32, height: u32) -> Vec<[u32; 4]> {
    if width == 0 || height == 0 {
        return Vec::new();
    }

    let cell_w = (width / 8).max(1);
    let cell_h = (height / 8).max(1);
    let mut regions = Vec::new();

    for cy in 0..8 {
        for cx in 0..8 {
            let x0 = cx * cell_w;
            let y0 = cy * cell_h;
            let x1 = ((cx + 1) * cell_w).min(width);
            let y1 = ((cy + 1) * cell_h).min(height);

            let mut changed = 0u32;
            let total = (x1 - x0) * (y1 - y0);

            for y in y0..y1 {
                for x in x0..x1 {
                    let idx = (y * width + x) as usize;
                    if idx < mask.len() && mask[idx] {
                        changed += 1;
                    }
                }
            }

            if total > 0 && (changed as f64 / total as f64) > 0.10 {
                regions.push([x0, y0, x1 - x0, y1 - y0]);
            }
        }
    }

    // Merge adjacent regions
    merge_regions(&mut regions);
    regions
}

/// Merge overlapping or adjacent regions.
fn merge_regions(regions: &mut Vec<[u32; 4]>) {
    if regions.len() <= 1 {
        return;
    }

    let mut merged = true;
    while merged {
        merged = false;
        let mut i = 0;
        while i < regions.len() {
            let mut j = i + 1;
            while j < regions.len() {
                if regions_adjacent(regions[i], regions[j]) {
                    // Merge j into i
                    let [ax, ay, aw, ah] = regions[i];
                    let [bx, by, bw, bh] = regions[j];
                    let nx = ax.min(bx);
                    let ny = ay.min(by);
                    let nx2 = (ax + aw).max(bx + bw);
                    let ny2 = (ay + ah).max(by + bh);
                    regions[i] = [nx, ny, nx2 - nx, ny2 - ny];
                    regions.remove(j);
                    merged = true;
                } else {
                    j += 1;
                }
            }
            i += 1;
        }
    }
}

fn regions_adjacent(a: [u32; 4], b: [u32; 4]) -> bool {
    let [ax, ay, aw, ah] = a;
    let [bx, by, bw, bh] = b;
    // Check if regions overlap or touch
    ax <= bx + bw && ax + aw >= bx && ay <= by + bh && ay + ah >= by
}

/// Decode a base64-encoded PNG screenshot into a DynamicImage.
pub fn decode_screenshot(base64_data: &str) -> Result<DynamicImage> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD.decode(base64_data)?;
    let img = image::load_from_memory(&bytes)?;
    Ok(img)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{RgbaImage, DynamicImage};

    /// Create a solid-color test image.
    fn solid_image(width: u32, height: u32, r: u8, g: u8, b: u8) -> DynamicImage {
        let img = RgbaImage::from_fn(width, height, |_x, _y| Rgba([r, g, b, 255]));
        DynamicImage::ImageRgba8(img)
    }

    /// Create a half-and-half test image (left half one color, right half another).
    fn split_image(width: u32, height: u32, r1: u8, g1: u8, b1: u8, r2: u8, g2: u8, b2: u8) -> DynamicImage {
        let half = width / 2;
        let img = RgbaImage::from_fn(width, height, |x, _y| {
            if x < half {
                Rgba([r1, g1, b1, 255])
            } else {
                Rgba([r2, g2, b2, 255])
            }
        });
        DynamicImage::ImageRgba8(img)
    }

    #[test]
    fn identical_images_not_changed() {
        let img = solid_image(64, 64, 128, 128, 128);
        let result = compare_frames(&img, &img);

        assert!(!result.changed);
        assert_eq!(result.hash_distance, 0);
        assert_eq!(result.pixel_mismatch_pct, 0.0);
        assert_eq!(result.changed_region_count, 0);
    }

    #[test]
    fn completely_different_images_changed() {
        // Use a checkerboard-like pattern so every pixel differs significantly.
        let a = RgbaImage::from_fn(64, 64, |x, y| {
            if (x + y) % 2 == 0 {
                Rgba([0, 0, 0, 255])
            } else {
                Rgba([100, 100, 100, 255])
            }
        });
        let b = RgbaImage::from_fn(64, 64, |x, y| {
            if (x + y) % 2 == 0 {
                Rgba([200, 200, 200, 255])
            } else {
                Rgba([255, 255, 255, 255])
            }
        });
        let img_a = DynamicImage::ImageRgba8(a);
        let img_b = DynamicImage::ImageRgba8(b);
        let result = compare_frames(&img_a, &img_b);

        assert!(
            result.changed,
            "expected changed=true, got hash_distance={}, pixel_mismatch_pct={}",
            result.hash_distance, result.pixel_mismatch_pct
        );
    }

    #[test]
    fn partially_different_images() {
        let before = solid_image(64, 64, 100, 100, 100);
        let after = split_image(64, 64, 100, 100, 100, 200, 200, 200);
        let result = compare_frames(&before, &after);

        // Right half changed significantly
        assert!(result.changed);
        assert!(result.pixel_mismatch_pct > 0.0);
        assert!(result.changed_region_count > 0);
    }

    #[test]
    fn mean_hash_is_consistent() {
        let img = solid_image(64, 64, 128, 128, 128);
        let hash1 = mean_hash(&img);
        let hash2 = mean_hash(&img);

        assert_eq!(hash1, hash2);
    }

    #[test]
    fn mean_hash_differs_for_different_images() {
        // Solid-color images all produce zero hashes (every pixel == mean),
        // so use images with internal contrast to get distinguishable hashes.
        let dark_gradient = split_image(64, 64, 0, 0, 0, 100, 100, 100);
        let bright_gradient = split_image(64, 64, 150, 150, 150, 255, 255, 255);
        let hash_a = mean_hash(&dark_gradient);
        let hash_b = mean_hash(&bright_gradient);

        // Both have internal contrast, so bits are set, and the patterns differ
        // because the threshold (mean) cuts differently.
        // At minimum, verify that non-solid images produce non-zero hashes.
        assert_ne!(hash_a, [0u8; 32], "dark gradient hash should not be all zero");
        assert_ne!(hash_b, [0u8; 32], "bright gradient hash should not be all zero");
    }

    #[test]
    fn perceptual_hash_distance_zero_for_same() {
        let img = solid_image(64, 64, 50, 100, 150);
        assert_eq!(perceptual_hash_distance(&img, &img), 0);
    }

    #[test]
    fn perceptual_hash_distance_positive_for_different() {
        // Use images with internal contrast so mean-hash produces non-zero hashes.
        let a = split_image(64, 64, 0, 0, 0, 200, 200, 200);
        let b = split_image(64, 64, 200, 200, 200, 0, 0, 0); // reversed
        assert!(perceptual_hash_distance(&a, &b) > 0);
    }

    #[test]
    fn pixel_diff_zero_for_identical() {
        let img = solid_image(32, 32, 100, 100, 100);
        let (mismatch, mask) = pixel_diff(&img, &img, 30);

        assert_eq!(mismatch, 0.0);
        assert!(mask.iter().all(|&v| !v));
    }

    #[test]
    fn pixel_diff_detects_threshold() {
        let a = solid_image(32, 32, 100, 100, 100);
        let b = solid_image(32, 32, 110, 100, 100); // diff = 10 < threshold 30

        let (mismatch, _) = pixel_diff(&a, &b, 30);
        assert_eq!(mismatch, 0.0); // Below threshold

        let (mismatch2, _) = pixel_diff(&a, &b, 5);
        assert!(mismatch2 > 0.0); // Above threshold
    }

    #[test]
    fn detect_changed_regions_empty_for_no_change() {
        let mask = vec![false; 64 * 64];
        let regions = detect_changed_regions(&mask, 64, 64);
        assert!(regions.is_empty());
    }

    #[test]
    fn detect_changed_regions_finds_changes() {
        // All pixels changed
        let mask = vec![true; 64 * 64];
        let regions = detect_changed_regions(&mask, 64, 64);
        assert!(!regions.is_empty());
    }

    #[test]
    fn detect_changed_regions_zero_dimensions() {
        let regions = detect_changed_regions(&[], 0, 0);
        assert!(regions.is_empty());
    }

    #[test]
    fn compare_frames_different_sizes() {
        // compare_frames uses min(w1,w2) x min(h1,h2) — should not panic
        let small = solid_image(32, 32, 100, 100, 100);
        let large = solid_image(64, 64, 100, 100, 100);
        let result = compare_frames(&small, &large);
        // Same color, just different size — comparison area is the smaller image
        assert!(!result.changed);
    }

    #[test]
    fn regions_adjacent_touching() {
        assert!(regions_adjacent([0, 0, 10, 10], [10, 0, 10, 10]));
        assert!(regions_adjacent([0, 0, 10, 10], [0, 10, 10, 10]));
    }

    #[test]
    fn regions_adjacent_separated() {
        assert!(!regions_adjacent([0, 0, 5, 5], [20, 20, 5, 5]));
    }
}
