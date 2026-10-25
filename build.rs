//! Build script.
//!
//! The crate supports two deployment models that share the same sources:
//!
//! * portable (default): `windows-reactor-setup` copies the Windows App SDK
//!   runtime next to the executable and embeds a registration-free WinRT
//!   manifest, so `target/<profile>` is a self-contained folder.
//! * MSIX (`--features msix`): nothing is copied. WinRT activation and WinUI
//!   come from the `Microsoft.WindowsAppRuntime.2` framework package that
//!   `packaging/AppxManifest.xml` declares, so the executable only runs with
//!   package identity.
//!
//! Both models write `snapline.ico` next to the executable; the tray icon and
//! the WinUI windows load it from there (`platform::icon_path`). The MSIX model
//! also writes the logo PNGs that `packaging/AppxManifest.xml` references.

use std::path::{Path, PathBuf};

/// `(file name in the package `Assets` folder, edge length in pixels)`.
const MSIX_LOGOS: &[(&str, u32)] = &[
    ("Square44x44Logo.png", 44),
    ("Square71x71Logo.png", 71),
    ("Square150x150Logo.png", 150),
    ("StoreLogo.png", 50),
];

/// Frame of the clip-shaped icon. Original artwork, not the upstream app icon.
const FRAME: [u8; 4] = [105, 119, 44, 255];
const CLIP: [u8; 4] = [241, 251, 248, 255];
/// Sub-samples per axis, for anti-aliased edges at every generated size.
const SAMPLES: u32 = 4;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let packaged = std::env::var_os("CARGO_FEATURE_MSIX").is_some();
    if !packaged {
        windows_reactor_setup::as_self_contained();
    }
    let out = PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    let profile = profile_dir(&out);
    write_icon(&profile.join("snapline.ico"));
    if packaged {
        write_msix_logos(&profile.join("msix-assets"));
    }
}

/// Directory that holds the linked executable (`target/<profile>`). `OUT_DIR` is
/// `target/<profile>/build/<package>-<hash>/out`, but the profile segment can move
/// when `--target-dir` or a split package cache is used, so look for it first.
fn profile_dir(out: &Path) -> PathBuf {
    let profile = std::env::var_os("PROFILE").unwrap_or_default();
    out.ancestors()
        .find(|path| path.file_name() == Some(profile.as_os_str()))
        .or_else(|| out.ancestors().nth(3))
        .map(Path::to_path_buf)
        .unwrap_or_else(|| out.to_path_buf())
}

/// Writes the 32x32 ICO that `platform::icon_path` loads from next to the
/// executable: a BMP with an AND mask, bottom-up BGRA.
fn write_icon(path: &Path) {
    const EDGE: usize = 32;
    let pixels = render(EDGE);
    let mut bitmap = vec![0u8; 40 + EDGE * EDGE * 4 + EDGE * 4];
    bitmap[0..4].copy_from_slice(&40u32.to_le_bytes());
    bitmap[4..8].copy_from_slice(&(EDGE as i32).to_le_bytes());
    bitmap[8..12].copy_from_slice(&((EDGE * 2) as i32).to_le_bytes());
    bitmap[12..14].copy_from_slice(&1u16.to_le_bytes());
    bitmap[14..16].copy_from_slice(&32u16.to_le_bytes());
    for y in 0..EDGE {
        for x in 0..EDGE {
            let at = (y * EDGE + x) * 4;
            let (r, g, b, a) = (pixels[at], pixels[at + 1], pixels[at + 2], pixels[at + 3]);
            let to = 40 + ((EDGE - 1 - y) * EDGE + x) * 4;
            bitmap[to..to + 4].copy_from_slice(&[b, g, r, a]);
            if a == 0 {
                bitmap[40 + EDGE * EDGE * 4 + (EDGE - 1 - y) * 4 + x / 8] |= 0x80 >> (x % 8);
            }
        }
    }
    let mut ico = vec![0, 0, 1, 0, 1, 0, EDGE as u8, EDGE as u8, 0, 0, 1, 0, 32, 0];
    ico.extend_from_slice(&(bitmap.len() as u32).to_le_bytes());
    ico.extend_from_slice(&22u32.to_le_bytes());
    ico.extend_from_slice(&bitmap);
    std::fs::write(path, ico).unwrap();
}

/// Writes the MSIX tile logos. `packaging/AppxManifest.xml` refers to these by
/// unqualified name, so the package needs no resource index (`resources.pri`).
fn write_msix_logos(dir: &Path) {
    // Rebuild the folder so a removed entry never lingers in the staged package.
    let _ = std::fs::remove_dir_all(dir);
    std::fs::create_dir_all(dir).unwrap();
    for (name, size) in MSIX_LOGOS {
        let pixels = render(*size as usize);
        let file = std::fs::File::create(dir.join(name)).unwrap();
        let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), *size, *size);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(&pixels).unwrap();
        writer.finish().unwrap();
    }
}

/// Renders the glyph into a square RGBA buffer with `size` pixels per edge.
fn render(size: usize) -> Vec<u8> {
    let mut pixels = vec![0u8; size * size * 4];
    let edge = size as f32;
    let sample = SAMPLES as f32;
    for y in 0..size {
        for x in 0..size {
            let mut alpha = 0u32;
            let mut weighted = [0u32; 3];
            for sy in 0..SAMPLES {
                for sx in 0..SAMPLES {
                    let nx = (x as f32 + (sx as f32 + 0.5) / sample) / edge;
                    let ny = (y as f32 + (sy as f32 + 0.5) / sample) / edge;
                    let [r, g, b, a] = glyph(nx, ny);
                    let a = a as u32;
                    weighted[0] += r as u32 * a;
                    weighted[1] += g as u32 * a;
                    weighted[2] += b as u32 * a;
                    alpha += a;
                }
            }
            let at = (y * size + x) * 4;
            // A fully transparent pixel has a zero alpha sum; the color is unused
            // then, so divide by one instead of branching around the division.
            let divisor = alpha.max(1);
            pixels[at] = (weighted[0] / divisor) as u8;
            pixels[at + 1] = (weighted[1] / divisor) as u8;
            pixels[at + 2] = (weighted[2] / divisor) as u8;
            pixels[at + 3] = (alpha / (SAMPLES * SAMPLES)) as u8;
        }
    }
    pixels
}

/// Normalized glyph, sampled at `(x, y)` in `0.0..1.0`. The frame is the shelf
/// card, the clip is the clothes peg that hangs the photo on the rope.
fn glyph(x: f32, y: f32) -> [u8; 4] {
    let inside = (3.0 / 32.0..29.0 / 32.0).contains(&x) && (3.0 / 32.0..29.0 / 32.0).contains(&y);
    if !inside {
        return [0, 0, 0, 0];
    }
    let clip = (10.0 / 32.0..14.0 / 32.0).contains(&x) && (8.0 / 32.0..24.0 / 32.0).contains(&y)
        || (13.0 / 32.0..23.0 / 32.0).contains(&x)
            && ((8.0 / 32.0..12.0 / 32.0).contains(&y) || (20.0 / 32.0..24.0 / 32.0).contains(&y))
        || (20.0 / 32.0..24.0 / 32.0).contains(&x) && (11.0 / 32.0..21.0 / 32.0).contains(&y);
    if clip { CLIP } else { FRAME }
}
