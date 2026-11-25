# CityMayor

City-builder 3D en Rust, inspiré de *Cities: Skylines* / *SimCity*.
Cible : **1 M d'habitants**, routes **courbes**, rendu **Bevy**.

État : itération 1 terminée + itération 2a (viewer Bevy fonctionnel).
**95 tests**, clippy muet, démo headless et viewer 3D opérationnels.

```bash
cargo test --workspace                # 95 tests, ~0,5 s (sans Bevy)
cargo run -p citymayor-app            # ville de démo headless, 640 ticks
cargo run -p citymayor-app --features viewer --bin city-viewer   # fenêtre 3D
```

> Passation / reprise : voir **[HANDOFF.md](HANDOFF.md)** (mémoire complète du projet).

## Décisions verrouillées

| Sujet | Choix | Conséquence |
|---|---|---|
| Moteur | **Bevy 0.20.1**, isolé derrière la feature `viewer` | `cargo test` ne compile pas Bevy (3 s) |
| Routes | **Bézier cubique** en point fixe, une poignée sortante par nœud | continuité G1 structurelle, courbes éditables |
| Échelle | **1 M d'habitants** | croissance composée, budgets proportionnels à la carte, slab compact |
| Simulation | **20 Hz fixe**, 100 % entiers (`Fixed` Q48.16) | déterminisme bit-à-bit, replay possible |

## Arborescence

```
Cargo.toml                    workspace, lints partagés, profils
crates/
  sim-core/                   simulation déterministe, zéro dépendance, zéro unsafe
    fixed.rs                  Q48.16
    rng.rs                    PCG32 (contrat de reproductibilité)
    time.rs                   Tick, FixedStepClock, Calendar
    grid.rs                   grille dense row-major (toutes les couches partagent TileId)
    terrain.rs                heightmap cm, constructibilité par pente
    zoning.rs                 R/C/I/Unzoned par tuile
    spline.rs                 Bézier cubique : longueur d'arc, courbure, échantillonnage
    road.rs                   graphe + poignées + adjacence CSR + desserte rasterisée
    building.rs               slab 1 bâtiment/tuile + slab compact des logements
    population.rs             census, emploi, vacance, bonheur (EMA)
    demand.rs                 signaux RCI
    land_value.rs             valeur foncière, recalcul borné par curseur
    economy.rs                trésorerie, règlement mensuel
    command.rs                Command (seule porte d'entrée), Rect
    metrics.rs                Snapshot + enum Overlay
    world.rs                  état complet + pipeline de tick
    tests/vertical_slice.rs   scénarios causaux, déterminisme, échelle 256²
  render/                     présentation
    scene.rs                  World -> blocs/ribans/maillage (testé sans GPU)
    viewer.rs                 viewer Bevy (feature `viewer`)
  app/                        binaire headless + bin `city-viewer`
  sim-transport/              stub it. 3 (trafic, pathfinding, agents)
  sim-economy/                stub it. 5
  ui/                         stub it. 2 (HUD, outils)
```

## Frontières

```
app ──► sim-core, render(optionnel)
render ──► sim-core          (lecture seule)
ui ──► sim-core              (Command -> World, Snapshot <- World)
sim-transport ──► sim-core
sim-economy ──► sim-core
sim-core ──► std
```

`sim-core` n'a **aucune** dépendance externe. Le rendu ne possède aucun état de
simulation : il dérive (`scene.rs`) puis jette.

## Déterminisme

1. Aucun flottant dans l'état simulé (`f32` uniquement à l'affichage).
2. La simulation ne voit jamais `dt`, seulement `Tick`.
3. Itérations dans l'ordre des indices (`Vec`, `Grid`) — jamais un `HashMap`
   (réservé aux lookups).
4. Mutations uniquement via `Command`, FIFO en début de tick.
5. Un flux RNG par système (aucun système n'en consomme encore).
6. Tests de garde : `simulation_is_deterministic_across_runs`,
   `different_seeds_do_not_change_a_deterministic_pipeline`.

## Temps et pipeline

| Concept | Défaut |
|---|---|
| tick | 50 ms (20 Hz) |
| rattrapage max | 5 ticks/frame (au-delà, temps jeté) |
| jour | 1 200 ticks (60 s) |
| mois | 30 jours |

```
apply_commands → refresh_road_access → land_value → move_population
→ grow → census → demand → budget (1×/mois)
```

Le coût d'un tick ne dépend pas de la taille de la carte : budgets de tuiles et
de bâtiments par tick, curseurs persistants. Les budgets foncier/population sont
dérivés de la surface (`tuiles/400`, bornés 4 096..65 536).

## Modèle de simulation (MVP)

```
demande_R = 0.25 + 0.50·emplois_ouverts + 0.35·(1 − 2·vacance) + 0.20·(bonheur − 0.5)
demande_C = (0.30 + 1.50·(0.30 − emplois_C/habitants)) · min(1, habitants/12)
demande_I = (0.30 + 1.20·(0.45 − emplois_I/habitants) + 0.30·chômage) · min(1, habitants/8)
bonheur   = 0.30 + 0.40·(1 − chômage) + 0.30·logement_disponible   (EMA 2 %/tick)
valeur    = 0.30 + 0.25·desserte + bonus_de_zone                   (borné [0,1])
```

Croissance **composée** : `quota = excès · max(1, 1 % du stock)`, plafonné à
64/tick/zone. Immigration pilotée par la vacance : `5 % × logements vides`,
plancher 6/tick. Sortie plafonnée sous l'entrée (anti death wave), aucune
démolition automatique.

## Routes courbes

- Un nœud a **une** poignée sortante (`handle`, mètres). Segment `a→b` :
  `c0 = a + handle_a`, `c1 = b − handle_b`. Poignée nulle = droite exacte.
- Deux segments voisins partagent la tangente : **G1 par construction**.
- `BezierCubic` fournit `eval`, `derivative`, `length` (somme de cordes),
  `min_curvature_radius`, `sample_into`.
- La desserte (`rasterize_access`) suit la courbe échantillonnée, pas la corde.
- `Command::BuildCurve` porte les poignées ; les poignées ne s'appliquent qu'aux
  nœuds **créés** par la commande.

## Rendu (itération 2a)

- `scene.rs` : aucun Bevy, testé (blocs sains, rubans courbes, maillage valide,
  déterminisme).
- `viewer.rs` : terrain (grille), bâtiments fusionnés en un mesh coloré, routes
  en rubans, caméra orbitale (WASD/QE/RF/ZX), lumière directionnelle.
- Reconstruction des meshes seulement quand `(bâtiments, segments, mois)` change.
- Capture : `citymayor-auto.png` après 4 s, `F12` pour une capture manuelle.

## Limites connues

- Viewer : pas de LOD ni d'instancing GPU, pas de ciel/ombres, caméra clavier
  seulement, ville sparse au démarrage (pas de pré-simulation), overlays non
  branchés.
- Simulation : pas d'agents (pas de trafic), zonage par tuile, pas de niveaux de
  bâtiment, pas de services/pollution/réseaux, pas de sérialisation.
- Budget volontairement excédentaire (aucune charge de service).
- Desserte rasterisée naïve : index spatial nécessaire au-delà de quelques
  milliers de segments.

## Roadmap

| It. | Contenu | Critère de sortie |
|---|---|---|
| 2b | pré-sim, caméra souris, overlays, outils de construction, UI | ville lisible et pilotable à 60 fps |
| 3 | trafic (hybride agents/flot), A* déterministe, cache de chemins | 50 k trajets sans casser le tick |
| 4 | parcelles/front de rue, niveaux, services, pollution/bruit | densité émergente stable |
| 5 | entreprises, chaînes, marché foncier | économie lisible |
| 6 | sérialisation, replay, benchmarks CI | sauvegarde sans perte, tick < 5 ms |
