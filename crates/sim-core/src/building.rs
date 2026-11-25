//! Bâtiments : slab contigu, **un bâtiment par tuile maximum**.
//!
//! Invariant MVP : `slots[tile.index()]` — le slab est indexé par la tuile.
//! Conséquences (toutes voulues) :
//! - `at(tile)` est un accès O(1) sans table secondaire ;
//! - pas de chevauchement possible (validation gratuite) ;
//! - la croissance peut scanner la grille et muter le slab sans hash map ;
//! - quand les parcelles multi-tuiles arriveront, `Buildings` passera à un slab
//!   compact + `Vec<Option<BuildingId>>` par tuile (même API, changement interne).

use crate::ids::{BuildingId, TileId};
use crate::zoning::ZoneType;

/// Nature d'un bâtiment, dérivée de la zone mais stockée (les zones peuvent
/// changer : le bâtiment, lui, ne se transforme pas dans cette itération).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BuildingKind {
    /// Logement : `capacity` = résidents.
    Household,
    /// Commerce : `capacity` = emplois.
    Shop,
    /// Industrie : `capacity` = emplois.
    Factory,
}

impl BuildingKind {
    #[must_use]
    pub const fn from_zone(zone: ZoneType) -> Option<Self> {
        match zone {
            ZoneType::Residential => Some(Self::Household),
            ZoneType::Commercial => Some(Self::Shop),
            ZoneType::Industrial => Some(Self::Factory),
            ZoneType::Unzoned => None,
        }
    }

    #[must_use]
    pub const fn provides_jobs(self) -> bool {
        matches!(self, Self::Shop | Self::Factory)
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Household => "logement",
            Self::Shop => "commerce",
            Self::Factory => "usine",
        }
    }

    /// Index dense `0..3` pour les compteurs (stock par type).
    #[must_use]
    pub const fn index(self) -> usize {
        match self {
            Self::Household => 0,
            Self::Shop => 1,
            Self::Factory => 2,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Building {
    pub kind: BuildingKind,
    pub tile: TileId,
    /// Niveau de développement (1..=3 à terme : densité, emplois, revenus).
    pub level: u8,
    /// Résidents (logement) ou emplois (commerce/industrie).
    pub capacity: u32,
    /// Occupants effectifs (résidents). Toujours `<= capacity`.
    pub occupants: u32,
}

impl Building {
    #[must_use]
    pub const fn is_household(self) -> bool {
        matches!(self.kind, BuildingKind::Household)
    }

    #[must_use]
    pub const fn free_slots(self) -> u32 {
        self.capacity.saturating_sub(self.occupants)
    }
}

/// Slab de bâtiments indexé par tuile.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Buildings {
    slots: Vec<Option<Building>>,
    live: u32,
    /// Stock par type, maintenu incrémentalement : c'est l'entrée de la loi de
    /// croissance composée (`World::grow`) et un agrégat gratuit pour le HUD.
    by_kind: [u32; 3],
    /// Slab **compact** des logements, dans l'ordre d'apparition.
    ///
    /// Pourquoi une seconde liste ? Le flux migratoire doit toucher des
    /// logements, pas des tuiles. Itérer la grille revient à échantillonner
    /// 65 000 tuiles pour trouver 1 bâtiment sur une grande carte : le curseur
    /// ne recroise un logement que tous les 16 ticks et la ville ne démarre
    /// jamais (bug mesuré sur 256²). Ici, chaque tick touche un logement réel.
    ///
    /// Invariant : `household_pos[slot]` est la position du logement de cette
    /// tuile dans `households`, ou `u32::MAX` s'il n'y en a pas. Suppression par
    /// `swap_remove` + réparation de la position déplacée (O(1)).
    households: Vec<BuildingId>,
    household_pos: Vec<u32>,
}

impl Buildings {
    #[must_use]
    pub fn new(tile_count: usize) -> Self {
        Self {
            slots: vec![None; tile_count],
            live: 0,
            by_kind: [0; 3],
            households: Vec::new(),
            household_pos: vec![u32::MAX; tile_count],
        }
    }

    #[must_use]
    pub fn slot_len(&self) -> usize {
        self.slots.len()
    }

    #[must_use]
    pub const fn live_count(&self) -> u32 {
        self.live
    }

    #[must_use]
    pub fn id_at_slot(&self, slot: usize) -> Option<BuildingId> {
        self.slots
            .get(slot)
            .and_then(Option::as_ref)
            .map(|_| BuildingId::new(slot as u32))
    }

    #[must_use]
    pub fn get(&self, id: BuildingId) -> Option<&Building> {
        self.slots.get(id.index()).and_then(Option::as_ref)
    }

    #[must_use]
    pub fn get_mut(&mut self, id: BuildingId) -> Option<&mut Building> {
        self.slots.get_mut(id.index()).and_then(Option::as_mut)
    }

    #[must_use]
    pub fn at(&self, tile: TileId) -> Option<BuildingId> {
        self.id_at_slot(tile.index())
    }

    /// Construit un bâtiment. Renvoie `None` si la tuile est occupée ou hors grille.
    pub fn spawn(&mut self, tile: TileId, kind: BuildingKind, capacity: u32) -> Option<BuildingId> {
        debug_assert!(capacity > 0, "batiment a capacite nulle");
        let slot = self.slots.get_mut(tile.index())?;
        if slot.is_some() {
            return None;
        }
        *slot = Some(Building {
            kind,
            tile,
            level: 1,
            capacity,
            occupants: 0,
        });
        self.live += 1;
        self.by_kind[kind.index()] += 1;
        let id = BuildingId::new(tile.raw());
        if kind == BuildingKind::Household {
            self.household_pos[tile.index()] = self.households.len() as u32;
            self.households.push(id);
        }
        Some(id)
    }

    pub fn remove(&mut self, id: BuildingId) -> Option<Building> {
        let slot = self.slots.get_mut(id.index())?;
        let removed = slot.take();
        if let Some(building) = removed {
            self.live -= 1;
            self.by_kind[building.kind.index()] -= 1;
            if building.kind == BuildingKind::Household {
                self.unlink_household(id);
            }
        }
        removed
    }

    /// Stock d'un type de bâtiment (O(1)).
    #[must_use]
    pub fn count(&self, kind: BuildingKind) -> u32 {
        self.by_kind[kind.index()]
    }

    /// Nombre de logements (O(1)).
    #[must_use]
    pub fn household_count(&self) -> usize {
        self.households.len()
    }

    /// Logement à une position du slab compact (itération sans borrow tenu).
    #[must_use]
    pub fn household_at(&self, position: usize) -> Option<BuildingId> {
        self.households.get(position).copied()
    }

    fn unlink_household(&mut self, id: BuildingId) {
        let position = self.household_pos[id.index()] as usize;
        self.household_pos[id.index()] = u32::MAX;
        self.households.swap_remove(position);
        if let Some(moved) = self.households.get(position) {
            self.household_pos[moved.index()] = position as u32;
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = (BuildingId, &Building)> + '_ {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(i, b)| b.as_ref().map(|b| (BuildingId::new(i as u32), b)))
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = (BuildingId, &mut Building)> + '_ {
        self.slots
            .iter_mut()
            .enumerate()
            .filter_map(|(i, b)| b.as_mut().map(|b| (BuildingId::new(i as u32), b)))
    }

    /// Somme des capacités par catégorie (un seul passage, hot path du census).
    #[must_use]
    pub fn capacity_totals(&self) -> (u32, u32) {
        let mut households = 0u32;
        let mut job_slots = 0u32;
        for b in self.slots.iter().flatten() {
            if b.is_household() {
                households += b.capacity;
            } else {
                job_slots += b.capacity;
            }
        }
        (households, job_slots)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_building_per_tile() {
        let mut b = Buildings::new(9);
        let id = b.spawn(TileId::new(4), BuildingKind::Household, 10).unwrap();
        assert!(b.spawn(TileId::new(4), BuildingKind::Shop, 5).is_none());
        assert_eq!(b.live_count(), 1);
        assert_eq!(b.at(TileId::new(4)), Some(id));
    }

    #[test]
    fn remove_frees_the_tile() {
        let mut b = Buildings::new(4);
        let id = b.spawn(TileId::new(1), BuildingKind::Factory, 8).unwrap();
        assert!(b.remove(id).is_some());
        assert!(b.remove(id).is_none());
        assert_eq!(b.live_count(), 0);
        assert!(b.spawn(TileId::new(1), BuildingKind::Shop, 3).is_some());
    }

    #[test]
    fn household_slab_stays_consistent_through_swap_removes() {
        let mut b = Buildings::new(9);
        let ids: Vec<_> = (0..4)
            .map(|i| b.spawn(TileId::new(i), BuildingKind::Household, 4).unwrap())
            .collect();
        b.spawn(TileId::new(5), BuildingKind::Shop, 4).unwrap();
        assert_eq!(b.household_count(), 4);

        // Supprimer celui du milieu déplace le dernier à sa place.
        b.remove(ids[1]);
        assert_eq!(b.household_count(), 3);
        let mut present: Vec<u32> = (0..b.household_count())
            .filter_map(|i| b.household_at(i))
            .map(|id| id.raw())
            .collect();
        present.sort_unstable();
        assert_eq!(present, vec![ids[0].raw(), ids[2].raw(), ids[3].raw()]);
        for i in 0..b.household_count() {
            let id = b.household_at(i).unwrap();
            assert_eq!(b.get(id).unwrap().tile.index(), id.index());
        }
    }

    #[test]
    fn kind_counts_track_spawns_and_removals() {
        let mut b = Buildings::new(9);
        b.spawn(TileId::new(0), BuildingKind::Household, 4).unwrap();
        let shop = b.spawn(TileId::new(1), BuildingKind::Shop, 3).unwrap();
        b.spawn(TileId::new(2), BuildingKind::Factory, 5).unwrap();
        assert_eq!(b.count(BuildingKind::Household), 1);
        assert_eq!(b.count(BuildingKind::Shop), 1);
        assert_eq!(b.count(BuildingKind::Factory), 1);
        b.remove(shop);
        assert_eq!(b.count(BuildingKind::Shop), 0);
        assert_eq!(b.live_count(), 2);
    }

    #[test]
    fn capacity_totals_split_households_and_jobs() {
        let mut b = Buildings::new(9);
        b.spawn(TileId::new(0), BuildingKind::Household, 4).unwrap();
        b.spawn(TileId::new(1), BuildingKind::Household, 6).unwrap();
        b.spawn(TileId::new(2), BuildingKind::Shop, 3).unwrap();
        b.spawn(TileId::new(3), BuildingKind::Factory, 9).unwrap();
        assert_eq!(b.capacity_totals(), (10, 12));
    }
}
