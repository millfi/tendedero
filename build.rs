fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    windows_reactor_setup::as_self_contained();
    // Original clip-shaped icon. This does not reuse the upstream name or artwork.
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    let destination = out.ancestors().nth(3).unwrap().join("snapline.ico");
    let mut bitmap = vec![0u8; 40 + 32 * 32 * 4 + 32 * 4];
    bitmap[0..4].copy_from_slice(&40u32.to_le_bytes());
    bitmap[4..8].copy_from_slice(&32i32.to_le_bytes());
    bitmap[8..12].copy_from_slice(&64i32.to_le_bytes());
    bitmap[12..14].copy_from_slice(&1u16.to_le_bytes());
    bitmap[14..16].copy_from_slice(&32u16.to_le_bytes());
    for y in 0..32 {
        for x in 0..32 {
            let inside = (3..29).contains(&x) && (3..29).contains(&y);
            let clip = (10..14).contains(&x) && (8..24).contains(&y)
                || (13..23).contains(&x) && ((8..12).contains(&y) || (20..24).contains(&y))
                || (20..24).contains(&x) && (11..21).contains(&y);
            let px = if clip {
                [241, 251, 248, 255]
            } else if inside {
                [105, 119, 44, 255]
            } else {
                [0, 0, 0, 0]
            };
            let at = 40 + ((31 - y) * 32 + x) * 4;
            bitmap[at..at + 4].copy_from_slice(&px);
            if !inside {
                bitmap[40 + 32 * 32 * 4 + (31 - y) * 4 + x / 8] |= 0x80 >> (x % 8);
            }
        }
    }
    let mut ico = vec![0, 0, 1, 0, 1, 0, 32, 32, 0, 0, 1, 0, 32, 0];
    ico.extend_from_slice(&(bitmap.len() as u32).to_le_bytes());
    ico.extend_from_slice(&22u32.to_le_bytes());
    ico.extend_from_slice(&bitmap);
    std::fs::write(destination, ico).unwrap();
}
