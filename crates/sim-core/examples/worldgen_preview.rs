//! Aperçu vu du dessus du générateur de monde (relief ombré + eau + forêts).
//!
//! ```bash
//! cargo run -p sim-core --example worldgen_preview -- [seed] [taille_km] [sortie.ppm]
//! ```
//! Le cadre blanc est la zone jouable (2 048 m). Les flottants ne servent ici
//! qu'à l'image : la génération elle-même est entière.

use std::io::Write;

use sim_core::{WorldGen, WorldGenParams};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let seed = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(WorldGenParams::default().seed);
    let size_km: i64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(24);
    let out = args.get(3).cloned().unwrap_or_else(|| "worldgen.ppm".into());

    let play_m = 2_048i64;
    let river = if std::env::var_os("NO_RIVER").is_some() { None } else { WorldGenParams::default().river };
    let generator = WorldGen::new(WorldGenParams { seed, river, ..WorldGenParams::default() }, play_m, play_m);
    let pixels = 900i64;
    let span_cm = size_km * 100_000;
    let origin = play_m * 50 - span_cm / 2; // centre de la zone jouable
    let step = span_cm / pixels;

    let mut heights = vec![0i32; (pixels * pixels) as usize];
    for py in 0..pixels {
        for px in 0..pixels {
            heights[(py * pixels + px) as usize] = generator.altitude_cm(origin + px * step, origin + py * step);
        }
    }

    let mut image = Vec::with_capacity((pixels * pixels * 3) as usize);
    for py in 0..pixels {
        for px in 0..pixels {
            let h = heights[(py * pixels + px) as usize] as f32 / 100.0;
            let hx = heights[(py * pixels + (px + 1).min(pixels - 1)) as usize] as f32 / 100.0;
            let hy = heights[((py + 1).min(pixels - 1) * pixels + px) as usize] as f32 / 100.0;
            let s = step as f32 / 100.0;
            // Ombrage de Lambert, soleil au nord-ouest.
            let (nx, ny) = ((h - hx) / s, (h - hy) / s);
            let len = (nx * nx + ny * ny + 1.0).sqrt();
            let light = ((nx * -0.6 + ny * -0.6 + 1.0 * 0.55) / len).clamp(0.15, 1.0);
            let slope = (nx * nx + ny * ny).sqrt();

            let (x, y) = (origin + px * step, origin + py * step);
            let forest = generator.forest_density_q16(x, y, (h * 100.0) as i32) as f32 / 65_536.0;
            let mut color = if h <= 0.0 {
                let depth = (-h / 6.0).clamp(0.0, 1.0);
                [0.20 - depth * 0.1, 0.42 - depth * 0.15, 0.55 - depth * 0.1]
            } else if h < 1.6 {
                [0.76, 0.70, 0.52]
            } else if slope > 0.9 || h > 1_450.0 {
                [0.52, 0.50, 0.47]
            } else {
                let grass = [0.36, 0.50, 0.25];
                let trees = [0.16, 0.30, 0.15];
                let f = if slope < 0.9 { forest } else { 0.0 };
                [
                    grass[0] + (trees[0] - grass[0]) * f,
                    grass[1] + (trees[1] - grass[1]) * f,
                    grass[2] + (trees[2] - grass[2]) * f,
                ]
            };
            if h > 1_650.0 {
                color = [0.93, 0.94, 0.96];
            }
            let shade = if h <= 0.0 { 1.0 } else { light };
            let on_border = (x.abs() < step || (x - play_m * 100).abs() < step) && (0..=play_m * 100).contains(&y)
                || (y.abs() < step || (y - play_m * 100).abs() < step) && (0..=play_m * 100).contains(&x);
            for c in color {
                let v = if on_border { 1.0 } else { c * shade };
                image.push((v.clamp(0.0, 1.0) * 255.0) as u8);
            }
        }
    }

    let mut file = std::fs::File::create(&out).expect("création du fichier");
    write!(file, "P6\n{pixels} {pixels}\n255\n").expect("en-tête");
    file.write_all(&image).expect("écriture");
    let max = heights.iter().max().copied().unwrap_or(0);
    let water = heights.iter().filter(|&&h| h <= 0).count();
    println!(
        "{out} : {size_km} km, altitude max {:.0} m, eau {:.1} %",
        max as f32 / 100.0,
        water as f32 * 100.0 / heights.len() as f32
    );
}
