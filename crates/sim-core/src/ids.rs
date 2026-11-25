//! Identifiants typés.
//!
//! Tous les ids sont des **indices de slab** (`u32`), pas des pointeurs : ils
//! restent valides après un `serde`, tiennent dans 4 octets, et se prêtent au
//! stockage contigu. Un id périmé (tombstone) doit être traité comme un
//! `Option::None` : les accès renvoient toujours `Option<&T>`.

macro_rules! index_type {
    ($name:ident, $what:literal) => {
        #[doc = concat!("Indice d'un ", $what, ".")]
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
        pub struct $name(u32);

        impl $name {
            #[must_use]
            pub const fn new(raw: u32) -> Self {
                Self(raw)
            }

            #[must_use]
            pub const fn raw(self) -> u32 {
                self.0
            }

            #[must_use]
            pub const fn index(self) -> usize {
                self.0 as usize
            }
        }

        impl core::fmt::Display for $name {
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                write!(f, concat!(stringify!($name), "({})"), self.0)
            }
        }
    };
}

index_type!(TileId, "tuile de terrain");
index_type!(NodeId, "nœud du graphe routier");
index_type!(SegmentId, "segment routier");
index_type!(BuildingId, "bâtiment");
