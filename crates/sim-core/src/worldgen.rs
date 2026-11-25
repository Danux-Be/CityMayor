//! Génération procédurale du monde : **une** fonction d'altitude continue,
//! définie sur un plan infini, dont la carte jouable n'est qu'une fenêtre.
//!
//! # Pourquoi une fonction continue plutôt qu'une heightmap bornée
//! La zone jouable (`Terrain`) est un échantillonnage de cette fonction aux
//! centres de tuiles. Le rendu échantillonne **la même fonction** au-delà des
//! limites pour dessiner le décor jusqu'à l'horizon : la jonction entre la
//! ville et le paysage est donc invisible par construction (pas de couture,
//! pas de mur), comme dans *Cities: Skylines*.
//!
//! # Déterminisme
//! 100 % entier (`i64`, coordonnées en centimètres, bruit en Q16). Aucune
//! fonction transcendante, aucun flottant : deux machines génèrent exactement
//! le même relief, donc la même constructibilité. Le rendu convertit en `f32`
//! à sa frontière, jamais l'inverse.
//!
//! # Composition du relief
//! ```text
//! plaine + collines (fBm)              atténuées dans la zone jouable
//!        + montagnes (fBm « ridged »)  masquées par la distance à la zone jouable
//!        − lacs (bassins de basse fréquence, seulement en plaine)
//!        − fleuve (vallée creusée le long d'une ligne méandreuse)
//! ```
//! Le niveau de l'eau est unique (`WATER_LEVEL_CM`) : tout point creusé sous ce
//! niveau est de l'eau, ce que `Terrain::is_buildable` sait déjà refuser.

use crate::terrain::{Terrain, TerrainError, WATER_LEVEL_CM};

/// Unité Q16.
const ONE: i64 = 1 << 16;

/// Paramètres du fleuve (mètres, sauf mention).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RiverParams {
    /// Demi-largeur du lit en eau.
    pub half_width_m: i64,
    /// Largeur de la berge (du bord de l'eau au haut de la berge).
    pub bank_m: i64,
    /// Largeur de la vallée qui raccorde la berge au relief naturel.
    pub valley_m: i64,
    /// Profondeur du lit sous le niveau de l'eau (cm).
    pub depth_cm: i64,
    /// Amplitude latérale des méandres.
    pub meander_amplitude_m: i64,
    /// Longueur d'onde des méandres.
    pub meander_wavelength_m: i64,
}

impl Default for RiverParams {
    fn default() -> Self {
        Self {
            half_width_m: 42,
            bank_m: 26,
            valley_m: 300,
            depth_cm: 450,
            meander_amplitude_m: 900,
            meander_wavelength_m: 3_400,
        }
    }
}

/// Paramètres de génération. Toutes les longueurs en mètres, altitudes en cm.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WorldGenParams {
    pub seed: u64,
    /// Altitude de base de la plaine.
    pub plain_altitude_cm: i64,
    /// Amplitude des collines **dans** la zone jouable (reste constructible).
    pub play_hill_amplitude_cm: i64,
    /// Amplitude des collines hors de la zone jouable.
    pub hill_amplitude_cm: i64,
    pub hill_wavelength_m: i64,
    /// Altitude maximale ajoutée par les montagnes.
    pub mountain_amplitude_cm: i64,
    pub mountain_wavelength_m: i64,
    /// Distance au bord de la zone jouable où les montagnes commencent…
    pub mountain_start_m: i64,
    /// … et où elles atteignent leur pleine amplitude.
    pub mountain_full_m: i64,
    pub lake_wavelength_m: i64,
    /// Seuil (Q16, `0..65536`) du bruit de lac au-delà duquel on creuse.
    pub lake_threshold_q16: i64,
    pub lake_depth_cm: i64,
    pub river: Option<RiverParams>,
}

impl Default for WorldGenParams {
    fn default() -> Self {
        Self {
            seed: 0x5EED_C17E,
            plain_altitude_cm: 600,
            play_hill_amplitude_cm: 1_400,
            hill_amplitude_cm: 6_000,
            hill_wavelength_m: 1_100,
            mountain_amplitude_cm: 185_000,
            mountain_wavelength_m: 5_200,
            mountain_start_m: 900,
            mountain_full_m: 6_500,
            lake_wavelength_m: 2_300,
            lake_threshold_q16: 43_000,
            lake_depth_cm: 600,
            river: Some(RiverParams::default()),
        }
    }
}

/// Seize directions unitaires (Q16), tous les 22,5° : évite toute fonction
/// trigonométrique pour orienter le fleuve.
const DIRECTIONS: [(i64, i64); 16] = [
    (65_536, 0),
    (60_547, 25_080),
    (46_341, 46_341),
    (25_080, 60_547),
    (0, 65_536),
    (-25_080, 60_547),
    (-46_341, 46_341),
    (-60_547, 25_080),
    (-65_536, 0),
    (-60_547, -25_080),
    (-46_341, -46_341),
    (-25_080, -60_547),
    (0, -65_536),
    (25_080, -60_547),
    (46_341, -46_341),
    (60_547, -25_080),
];

/// Pas d'échantillonnage de l'axe du fleuve (cm).
const RIVER_STEP_CM: i64 = 2_500;
/// Demi-étendue de l'axe du fleuve le long de sa direction (cm) : couvre
/// largement le décor visible (le brouillard tombe bien avant).
const RIVER_HALF_SPAN_CM: i64 = 6_000_000;
/// Au-delà de cette distance, le fleuve n'influence plus le relief : inutile
/// de chercher le segment le plus proche.
const RIVER_INFLUENCE_CM: i64 = 400_000;

/// Générateur prêt à échantillonner. Données entières, sérialisables.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorldGen {
    params: WorldGenParams,
    /// Taille de la zone jouable (cm), coin `(0, 0)`.
    play_width_cm: i64,
    play_height_cm: i64,
    /// Direction du fleuve (Q16) et point d'ancrage (cm).
    river_dir: (i64, i64),
    river_origin: (i64, i64),
    /// Décalage latéral de l'axe (cm), un point tous les `RIVER_STEP_CM`
    /// depuis `-RIVER_HALF_SPAN_CM`, dans le repère du fleuve. Vide sans fleuve.
    ///
    /// Précalculé : la distance « verticale » `|v − off(u)|` est fausse loin du
    /// lit et dessinait des stries ; la distance exacte à la polyligne non.
    river_axis: Vec<i64>,
}

impl WorldGen {
    /// Prépare le générateur pour une zone jouable de `width_m × height_m`.
    #[must_use]
    pub fn new(params: WorldGenParams, width_m: i64, height_m: i64) -> Self {
        let (w, h) = (width_m * 100, height_m * 100);
        let pick = hash(params.seed, 17, 29, 0xA11CE);
        // Le fleuve traverse la carte (jamais parallèle à un bord à 0° près,
        // pour éviter qu'il longe une limite) près de son centre.
        let river_dir = DIRECTIONS[1 + (pick as usize % 7) * 2 % 15];
        let jitter_x = (hash(params.seed, 3, 5, 0xB0B) - ONE / 2) * w / (5 * ONE);
        let jitter_y = (hash(params.seed, 7, 11, 0xB0B) - ONE / 2) * h / (5 * ONE);
        let river_axis = params.river.map_or_else(Vec::new, |river| {
            (-RIVER_HALF_SPAN_CM..=RIVER_HALF_SPAN_CM)
                .step_by(RIVER_STEP_CM as usize)
                .map(|u| meander_offset_cm(params.seed, &river, u))
                .collect()
        });
        Self {
            params,
            play_width_cm: w,
            play_height_cm: h,
            river_dir,
            river_origin: (w / 2 + jitter_x, h / 2 + jitter_y),
            river_axis,
        }
    }

    #[must_use]
    pub const fn params(&self) -> &WorldGenParams {
        &self.params
    }

    /// Distance (cm) d'un point au rectangle de la zone jouable (0 dedans).
    #[must_use]
    pub fn distance_to_play_area_cm(&self, x_cm: i64, y_cm: i64) -> i64 {
        let dx = (-x_cm).max(x_cm - self.play_width_cm).max(0);
        let dy = (-y_cm).max(y_cm - self.play_height_cm).max(0);
        isqrt(dx * dx + dy * dy)
    }

    /// Altitude (cm) en un point quelconque du plan, dedans ou hors carte.
    #[must_use]
    pub fn altitude_cm(&self, x_cm: i64, y_cm: i64) -> i32 {
        let p = &self.params;
        let seed = p.seed;
        let outside = self.distance_to_play_area_cm(x_cm, y_cm);

        // Déformation du domaine : les formes s'étirent et se tordent comme un
        // relief érodé, au lieu de suivre la grille du bruit.
        // Basse fréquence et amplitude modérée : au-delà, le relief s'étire en
        // stries (mesuré : pentes > 150 % et lacs en étoile).
        let warp_wl = p.hill_wavelength_m * 400;
        let wx = x_cm + mul(fbm(seed, x_cm, y_cm, warp_wl, 2, 8) - ONE / 2, warp_wl / 3);
        let wy = y_cm + mul(fbm(seed, x_cm, y_cm, warp_wl, 2, 9) - ONE / 2, warp_wl / 3);

        // Collines : douces dans la zone jouable, franches au-delà.
        let hills = fbm(seed, wx, wy, p.hill_wavelength_m * 100, 5, 1);
        let hill_amp = lerp(
            p.play_hill_amplitude_cm,
            p.hill_amplitude_cm,
            smoothstep(0, 120_000, outside),
        );
        let mut h = p.plain_altitude_cm + mul(hills, hill_amp);

        // Montagnes : crêtes (bruit « ridged ») + socle large, masquées par la
        // distance à la zone jouable. La ville est dans une vallée.
        let mask = smoothstep(p.mountain_start_m * 100, p.mountain_full_m * 100, outside);
        if mask > 0 {
            let wl = p.mountain_wavelength_m * 100;
            // 5 octaves : chaque octave de fBm ajoute autant de pente que la
            // précédente ; à 7, les versants dépassaient 150 % (mesuré).
            let ridges = ridged(seed, wx, wy, wl, 5, 2);
            let massif = fbm(seed, wx, wy, wl * 2, 3, 3);
            let shape = (ridges * 3 + massif) / 4;
            // Accentue les sommets (courbe convexe) pour des pics, pas des dômes.
            let shape = mul(shape, shape / 2 + ONE / 2);
            h += mul(mul(shape, mask), p.mountain_amplitude_cm);
        }

        // Lacs : bassins de basse fréquence, seulement en plaine (un lac ne doit
        // pas creuser un puits au sommet d'une montagne).
        let lake_noise = fbm(seed, (x_cm + wx) / 2, (y_cm + wy) / 2, p.lake_wavelength_m * 100, 3, 4);
        if lake_noise > p.lake_threshold_q16 {
            let t = smoothstep(p.lake_threshold_q16, p.lake_threshold_q16 + 5_000, lake_noise);
            let lowland = ONE - smoothstep(p.plain_altitude_cm + 1_500, p.plain_altitude_cm + 4_500, h);
            let t = mul(t, lowland);
            let bed = i64::from(WATER_LEVEL_CM) - p.lake_depth_cm;
            if bed < h {
                h += mul(bed - h, t);
            }
        }

        // Fleuve : on creuse une vallée (min avec le relief naturel).
        if let Some(river) = p.river {
            let distance = self.river_distance_cm(x_cm, y_cm, &river);
            let carved = river_profile_cm(&river, distance, h, mask);
            h = h.min(carved);
        }

        h.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
    }

    /// Distance (cm) à l'axe du fleuve, `None` s'il n'y a pas de fleuve.
    #[must_use]
    pub fn river_distance(&self, x_cm: i64, y_cm: i64) -> Option<i64> {
        self.params
            .river
            .map(|river| self.river_distance_cm(x_cm, y_cm, &river))
    }

    fn river_distance_cm(&self, x_cm: i64, y_cm: i64, _river: &RiverParams) -> i64 {
        let (dx, dy) = (x_cm - self.river_origin.0, y_cm - self.river_origin.1);
        let (cx, cy) = self.river_dir;
        // Repère du fleuve : `u` le long, `v` en travers (cm).
        let u = (dx * cx + dy * cy) / ONE;
        let v = (-dx * cy + dy * cx) / ONE;
        let last = self.river_axis.len() as i64 - 1;
        if last < 1 {
            return i64::MAX / 4;
        }
        let index = |u: i64| ((u + RIVER_HALF_SPAN_CM) / RIVER_STEP_CM).clamp(0, last - 1);
        let here = index(u);
        // Majorant : distance verticale à l'axe. Le segment le plus proche est
        // forcément à moins de cette distance le long de `u`.
        let vertical = (v - self.river_axis[here as usize]).abs();
        if vertical > RIVER_INFLUENCE_CM + 200_000 {
            return vertical;
        }
        let reach = vertical.min(RIVER_INFLUENCE_CM) + RIVER_STEP_CM;
        let (first, end) = (index(u - reach), index(u + reach));
        let mut best = i64::MAX;
        for i in first..=end {
            let ua = i * RIVER_STEP_CM - RIVER_HALF_SPAN_CM;
            let va = self.river_axis[i as usize];
            let vb = self.river_axis[i as usize + 1];
            best = best.min(segment_distance_sq(u, v, ua, va, ua + RIVER_STEP_CM, vb));
        }
        isqrt(best)
    }

    /// Densité de forêt (Q16, `0..=65536`). Nulle sur l'eau, les berges et
    /// au-dessus de la limite des arbres. La pente est laissée au rendu.
    #[must_use]
    pub fn forest_density_q16(&self, x_cm: i64, y_cm: i64, altitude_cm: i32) -> i64 {
        let altitude = i64::from(altitude_cm);
        if altitude <= i64::from(WATER_LEVEL_CM) + 150 {
            return 0;
        }
        let n = fbm(self.params.seed, x_cm, y_cm, 90_000, 4, 6);
        let patches = smoothstep(29_000, 39_000, n);
        let treeline = ONE - smoothstep(170_000, 210_000, altitude);
        mul(patches, treeline)
    }

    /// Échantillonne la zone jouable aux centres des tuiles.
    ///
    /// # Errors
    /// Dimensions nulles.
    pub fn terrain(&self, width: u32, height: u32, tile_size_m: i32) -> Result<Terrain, TerrainError> {
        let tile_cm = i64::from(tile_size_m) * 100;
        let mut heights = Vec::with_capacity(width as usize * height as usize);
        for y in 0..i64::from(height) {
            for x in 0..i64::from(width) {
                heights.push(self.altitude_cm(x * tile_cm + tile_cm / 2, y * tile_cm + tile_cm / 2));
            }
        }
        Terrain::from_heights(width, height, heights)
    }
}

/// Décalage latéral de l'axe du fleuve en `u` (cm) : méandres lisses.
fn meander_offset_cm(seed: u64, river: &RiverParams, u: i64) -> i64 {
    let n = fbm(seed, u, 0, river.meander_wavelength_m * 100, 3, 5);
    mul(2 * n - ONE, river.meander_amplitude_m * 100)
}

/// Carré de la distance du point `(px, py)` au segment `a→b` (cm²).
fn segment_distance_sq(px: i64, py: i64, ax: i64, ay: i64, bx: i64, by: i64) -> i64 {
    let (abx, aby) = (bx - ax, by - ay);
    let (apx, apy) = (px - ax, py - ay);
    let len_sq = abx * abx + aby * aby;
    let (qx, qy) = if len_sq == 0 {
        (ax, ay)
    } else {
        let t = (apx * abx + apy * aby).clamp(0, len_sq);
        (ax + abx * t / len_sq, ay + aby * t / len_sq)
    };
    (px - qx) * (px - qx) + (py - qy) * (py - qy)
}

/// Profil en travers du fleuve : lit, berge, puis vallée qui rejoint `natural`.
///
/// La vallée s'élargit en montagne (`mountain_mask`, Q16) pour former une
/// gorge praticable plutôt qu'un canyon vertical. Ce facteur doit varier
/// **lentement** : l'indexer sur l'altitude locale faisait bouger le bord de la
/// vallée avec chaque colline et dessinait des stries parallèles au fleuve.
fn river_profile_cm(river: &RiverParams, distance_cm: i64, natural_cm: i64, mountain_mask: i64) -> i64 {
    let water = i64::from(WATER_LEVEL_CM);
    let half = river.half_width_m * 100;
    let bank_end = half + river.bank_m * 100;
    let bank_top = water + 140;
    let valley_end = bank_end + river.valley_m * 100 + mul(mountain_mask, 250_000);
    if distance_cm <= half {
        // Lit concave : plus profond au centre.
        let t = (distance_cm * ONE) / half.max(1);
        water - river.depth_cm + mul(mul(t, t), river.depth_cm * 6 / 10)
    } else if distance_cm <= bank_end {
        let t = smoothstep(half, bank_end, distance_cm);
        lerp(water - river.depth_cm * 4 / 10, bank_top, t)
    } else {
        let t = smoothstep(bank_end, valley_end, distance_cm);
        lerp(bank_top, natural_cm.max(bank_top), t)
    }
}

// ------------------------------------------------------------ bruit entier

/// Hachage 2D → `0..65536` (finaliseur SplitMix64).
fn hash(seed: u64, ix: i64, iy: i64, salt: u64) -> i64 {
    let mut h = seed
        ^ (ix as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (iy as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F)
        ^ salt.wrapping_mul(0x1656_67B1_9E37_79F9);
    h ^= h >> 30;
    h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h ^= h >> 27;
    h = h.wrapping_mul(0x94D0_49BB_1331_11EB);
    h ^= h >> 31;
    (h >> 48) as i64
}

/// Produit Q16.
const fn mul(a: i64, b: i64) -> i64 {
    (a * b) >> 16
}

const fn lerp(a: i64, b: i64, t: i64) -> i64 {
    a + (((b - a) * t) >> 16)
}

/// `smoothstep` Q16 : 0 sous `e0`, `ONE` au-dessus de `e1`.
fn smoothstep(e0: i64, e1: i64, v: i64) -> i64 {
    if e1 <= e0 {
        return if v >= e1 { ONE } else { 0 };
    }
    let t = (((v - e0) * ONE) / (e1 - e0)).clamp(0, ONE);
    mul(mul(t, t), 3 * ONE - 2 * t)
}

/// Courbe d'atténuation quintique de Perlin (`6t⁵ − 15t⁴ + 10t³`), Q16.
const fn fade(t: i64) -> i64 {
    // t³ · (t · (6t − 15) + 10)
    let inner = mul(t, 6 * t - 15 * ONE) + 10 * ONE;
    mul(mul(mul(t, t), t), inner)
}

/// Bruit de gradient (Perlin) 2D **signé**, environ `−46341..46341` (±√2/2).
///
/// Les gradients sont tirés parmi les 16 directions de `DIRECTIONS` : aucune
/// trigonométrie, aucun flottant. Contrairement au bruit de valeur, il ne
/// laisse pas de motifs carrés alignés sur la grille.
fn gradient_noise(seed: u64, x: i64, y: i64, wavelength: i64, salt: u64) -> i64 {
    let wl = wavelength.max(1);
    let (ix, iy) = (x.div_euclid(wl), y.div_euclid(wl));
    let fx = (x.rem_euclid(wl) * ONE) / wl;
    let fy = (y.rem_euclid(wl) * ONE) / wl;
    let corner = |cx: i64, cy: i64, dx: i64, dy: i64| -> i64 {
        let (gx, gy) = DIRECTIONS[(hash(seed, cx, cy, salt) & 15) as usize];
        mul(gx, dx) + mul(gy, dy)
    };
    let a = corner(ix, iy, fx, fy);
    let b = corner(ix + 1, iy, fx - ONE, fy);
    let c = corner(ix, iy + 1, fx, fy - ONE);
    let d = corner(ix + 1, iy + 1, fx - ONE, fy - ONE);
    let (sx, sy) = (fade(fx), fade(fy));
    lerp(lerp(a, b, sx), lerp(c, d, sx), sy)
}

/// Bruit de gradient ramené dans `0..65536`.
fn value_noise(seed: u64, x: i64, y: i64, wavelength: i64, salt: u64) -> i64 {
    // ±0,707 → 0..1 : `0,5 + n / √2`.
    (ONE / 2 + mul(gradient_noise(seed, x, y, wavelength, salt), 92_682)).clamp(0, ONE)
}

/// Bruit fractal (fBm) normalisé dans `0..65536`.
fn fbm(seed: u64, x: i64, y: i64, wavelength: i64, octaves: u32, salt: u64) -> i64 {
    let (mut sum, mut total, mut amp, mut wl) = (0i64, 0i64, ONE, wavelength);
    for octave in 0..octaves {
        // Décalage par octave : casse l'alignement des grilles de bruit.
        let shift = i64::from(octave) * 1_234_567;
        sum += mul(value_noise(seed, x + shift, y - shift, wl, salt * 31 + u64::from(octave)), amp);
        total += amp;
        amp /= 2;
        wl = (wl / 2).max(1);
    }
    (sum * ONE) / total.max(1)
}

/// Bruit « ridged » : crêtes nettes (`1 − |2n − 1|`, au carré), `0..65536`.
fn ridged(seed: u64, x: i64, y: i64, wavelength: i64, octaves: u32, salt: u64) -> i64 {
    let (mut sum, mut total, mut amp, mut wl) = (0i64, 0i64, ONE, wavelength);
    let mut weight = ONE;
    for octave in 0..octaves {
        let shift = i64::from(octave) * 2_345_678;
        let n = gradient_noise(seed, x - shift, y + shift, wl, salt * 37 + u64::from(octave));
        // |n| ≤ √2/2 : on le ramène à 0..1 avant de creuser la crête. La
        // valeur absolue est adoucie (`√(n² + ε²) − ε`) : une crête en lame de
        // couteau donnait des versants à 70° sur le premier octave.
        let soft_abs = isqrt(n * n + 4_000 * 4_000) - 4_000;
        let r = (ONE - mul(soft_abs, 92_682)).clamp(0, ONE);
        let r = mul(r, r);
        // Les petites crêtes n'apparaissent que sur les grandes : relief érodé.
        let contribution = mul(r, weight);
        weight = contribution.clamp(0, ONE);
        sum += mul(contribution, amp);
        total += amp;
        amp = amp * 45 / 100; // gain 0,45 : moins de pente cumulée par les petits octaves
        wl = (wl / 2).max(1);
    }
    (sum * ONE) / total.max(1)
}

/// Racine carrée entière (Newton), `isqrt(n)² <= n`.
fn isqrt(n: i64) -> i64 {
    if n <= 0 {
        return 0;
    }
    let mut x = n;
    let mut y = (x + 1) / 2;
    while y < x {
        x = y;
        y = (x + n / x) / 2;
    }
    x
}

#[cfg(test)]
mod tests {
    use super::*;

    fn generator() -> WorldGen {
        WorldGen::new(WorldGenParams::default(), 2_048, 2_048)
    }

    #[test]
    fn generation_is_deterministic() {
        let a = generator();
        let b = generator();
        for i in 0..500i64 {
            let (x, y) = (i * 7_919 - 400_000, i * 104_729 % 900_000 - 200_000);
            assert_eq!(a.altitude_cm(x, y), b.altitude_cm(x, y));
        }
    }

    #[test]
    fn seeds_change_the_world() {
        let a = generator();
        let b = WorldGen::new(
            WorldGenParams {
                seed: 99,
                ..WorldGenParams::default()
            },
            2_048,
            2_048,
        );
        let differs = (0..200i64).any(|i| a.altitude_cm(i * 1_000, i * 700) != b.altitude_cm(i * 1_000, i * 700));
        assert!(differs);
    }

    #[test]
    fn relief_is_continuous() {
        // Pas de couture ni de marche : 1 m de déplacement ne change jamais
        // l'altitude de plus de 3 m. Les versants les plus raides des massifs
        // approchent 2,5 m/m (falaises) ; une marche de génération ferait des
        // dizaines de mètres.
        let g = generator();
        let mut steepest = 0;
        for i in 0..6_000i64 {
            let x = -1_200_000 + i * 997;
            let y = -300_000 + (i * 7_717) % 1_400_000;
            let a = i64::from(g.altitude_cm(x, y));
            let b = i64::from(g.altitude_cm(x + 100, y));
            let c = i64::from(g.altitude_cm(x, y + 100));
            steepest = steepest.max((a - b).abs()).max((a - c).abs());
            assert!((a - b).abs() < 300, "saut {a} -> {b} en ({x}, {y})");
            assert!((a - c).abs() < 300, "saut {a} -> {c} en ({x}, {y})");
        }
        assert!(steepest > 30, "relief suspectement plat : {steepest} cm/m");
    }

    #[test]
    fn play_area_slopes_stay_gentle() {
        // Dans la zone jouable, aucune pente ne dépasse 30 % hors des berges.
        let g = generator();
        for i in 0..4_000i64 {
            let (x, y) = ((i * 4_973) % 204_000, (i * 7_919) % 204_000);
            let a = i64::from(g.altitude_cm(x, y));
            let b = i64::from(g.altitude_cm(x + 100, y));
            assert!((a - b).abs() <= 30 || a <= 200 || b <= 200, "pente {a} -> {b} en ({x}, {y})");
        }
    }

    #[test]
    fn play_area_is_mostly_buildable_land_with_some_water() {
        let g = generator();
        let terrain = g.terrain(256, 256, 8).unwrap();
        let tiles = terrain.tile_count();
        let mut water = 0;
        let mut buildable = 0;
        for i in 0..tiles {
            let tile = crate::ids::TileId::new(i as u32);
            if terrain.altitude_cm(tile).unwrap() <= WATER_LEVEL_CM {
                water += 1;
            }
            if terrain.is_buildable(tile, 400) {
                buildable += 1;
            }
        }
        assert!(water > tiles / 100, "pas d'eau : {water}/{tiles}");
        assert!(buildable > tiles * 6 / 10, "trop peu constructible : {buildable}/{tiles}");
    }

    #[test]
    fn mountains_rise_around_the_play_area() {
        let g = generator();
        let inside_max = (0..64i64)
            .flat_map(|i| (0..64i64).map(move |j| (i, j)))
            .map(|(i, j)| g.altitude_cm(i * 3_200, j * 3_200))
            .max()
            .unwrap();
        // Couronne à ~8 km : on y cherche de vrais sommets.
        let ring_max = (0..720i64)
            .map(|k| {
                let (dx, dy) = DIRECTIONS[(k % 16) as usize];
                let r = 900_000 + (k / 16) * 10_000;
                g.altitude_cm(102_400 + dx * r / ONE, 102_400 + dy * r / ONE)
            })
            .max()
            .unwrap();
        assert!(inside_max < 3_500, "zone jouable trop accidentée : {inside_max} cm");
        assert!(ring_max > 60_000, "pas de montagnes : {ring_max} cm");
    }

    #[test]
    fn river_bed_is_under_water_and_banks_are_dry() {
        let g = generator();
        let river = g.params().river.unwrap();
        // Échantillonne autour du centre et vérifie la cohérence lit/berge.
        let mut wet_bed = 0;
        let mut checked = 0;
        for i in 0..40_000i64 {
            let (x, y) = ((i * 4_973) % 204_800, (i * 7_919) % 204_800);
            let d = g.river_distance(x, y).unwrap();
            let alt = g.altitude_cm(x, y);
            if d < river.half_width_m * 100 / 2 {
                checked += 1;
                if alt < WATER_LEVEL_CM {
                    wet_bed += 1;
                }
            }
            if d > (river.half_width_m + river.bank_m) * 100 {
                // Hors du lit, le fleuve ne creuse jamais sous l'eau (seuls les
                // lacs le peuvent).
                let natural = WorldGen::new(
                    WorldGenParams {
                        river: None,
                        ..*g.params()
                    },
                    2_048,
                    2_048,
                )
                .altitude_cm(x, y);
                if natural > WATER_LEVEL_CM {
                    assert!(alt > WATER_LEVEL_CM, "berge noyée en ({x}, {y})");
                }
            }
        }
        assert!(checked > 20, "le fleuve ne traverse pas la carte");
        assert_eq!(wet_bed, checked);
    }

    #[test]
    fn forest_avoids_water_and_summits() {
        let g = generator();
        assert_eq!(g.forest_density_q16(0, 0, WATER_LEVEL_CM - 10), 0);
        assert_eq!(g.forest_density_q16(0, 0, 250_000), 0);
        let any_forest = (0..2_000i64).any(|i| g.forest_density_q16(i * 9_000, i * 5_000, 900) > ONE / 2);
        assert!(any_forest);
    }

    #[test]
    fn isqrt_is_exact_floor() {
        for n in [0i64, 1, 2, 3, 4, 15, 16, 17, 1 << 40, (1 << 40) + 12_345] {
            let r = isqrt(n);
            assert!(r * r <= n && (r + 1) * (r + 1) > n, "{n}");
        }
    }
}
