//! `city-viewer` — fenêtre 3D (Bevy) sur le même `World` que la démo headless.
//!
//! ```bash
//! cargo run -p citymayor-app --features viewer --bin city-viewer
//! ```
//!
//! Caméra : WASD/flèches pour se déplacer, Q/E pour tourner, R/F pour
//! l'inclinaison, Z/X (ou +/-) pour le zoom.

fn main() {
    city_render::viewer::run();
}
