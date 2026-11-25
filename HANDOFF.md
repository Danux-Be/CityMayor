# Mémoire de passation — CityMayor

**Date :** 23 novembre 2026
**État :** itération 1 terminée + itération 2a (viewer Bevy) fonctionnelle.
**Prochaine étape :** itération 2b — rendre le viewer réellement utilisable (ville peuplée, caméra souris, overlays, outils).

> **Prompt de reprise pour Claude Code :**
> « Tu reprends le projet Rust CityMayor dans `/home/dany/HDD1/CityMayor`. Lis
> `HANDOFF.md` en entier, puis `README.md`. Objectif immédiat : itération 2b.
> Commence par `cargo test --workspace` (95 tests doivent passer) et
> `cargo clippy --workspace --all-targets` (muet). Ne casse aucun invariant de la
> section 4. Avance par petits commits logiques ; demande avant tout choix qui
> contredit la section 2. »

---

## 1. Résumé exécutable

City-builder 3D en Rust inspiré de *Cities: Skylines* / *SimCity*, cible
**1 M d'habitants**, routes **courbes**, rendu **Bevy 0.20.1**.

Ce qui marche aujourd'hui :

- un cœur de simulation **déterministe** (100 % point fixe, aucun flottant dans
  l'état), testé, sans aucune dépendance externe ;
- terrain, zonage R/C/I, réseau routier **courbe** (Bézier cubique), desserte,
  croissance, démographie, emploi, valeur foncière, budget ;
- une démo **headless** (`citymayor`) qui peuple une ville en 640 ticks ;
- un **viewer 3D Bevy** (`city-viewer`) qui affiche terrain, routes et bâtiments,
  avec caméra clavier et capture PNG (vérifié sur GTX 1080 Ti / Vulkan).

Ce qui ne marche pas encore : pas d'agents/trafic, pas d'UI, pas d'overlays
affichés, pas de parcelles multi-tuiles, pas de services publics, pas de
sauvegarde. Le viewer est un squelette visuel, pas encore un jeu.

Chiffres vérifiés au moment de la passation :

- **95 tests** : 71 unitaires `sim-core`, 19 d'intégration `sim-core`, 5 `city-render` ;
- `cargo clippy --workspace --all-targets` : **0 warning** ;
- démo headless à 640 ticks : 2 528 habitants (capacité saturée), 735 emplois,
  114 bâtiments, 2 segments, ~101 500 $ ;
- test d'échelle 256×256 (42 routes, zonage mixte) : ~210 000 habitants en
  3 000 ticks ; 1 M estimé vers ~4 000 ticks (≈ 3 min 20 de simulation à 20 Hz) ;
- viewer : capture 1400×860 valide (`citymayor-auto.png` après 4 s, `F12`).

## 2. Décisions verrouillées (ne pas rouvrir sans raison forte)

| Sujet | Décision | Pourquoi |
|---|---|---|
| Moteur | **Bevy 0.20.1**, derrière la feature `viewer` | vitesse de développement, écosystème ; la simulation n'en dépend pas |
| Routes | **Bézier cubique** en point fixe, une poignée sortante par nœud | G1 structurelle aux jonctions, édition locale, poignée nulle = droite exacte |
| Échelle | **1 M d'habitants** | impose croissance composée, budgets proportionnels, slab compact |
| Temps | **20 Hz fixe** (50 ms), rattrapage max 5 ticks/frame | déterminisme, coût stable, interpolation de rendu via `alpha()` |
| Données | **entiers + `Fixed` Q48.16** partout dans l'état | reproductibilité bit-à-bit, replay/multijoueur |
| Mutations | **`Command` uniquement**, FIFO en début de tick | replay, ghost preview, UI découplée |
| Rendu | lecture seule des vues ; dérivation dans `scene.rs` sans Bevy | testable sans GPU, frontière nette |
| Géométrie | rendu en **mètres, Y vers le haut** ; simulation en 2D (x, y plan) | une seule conversion, dans `scene.rs` |

Rappel : le rendu **ne doit jamais** être nécessaire à la simulation. Si un jour
Bevy disparaît, `sim-core` + `citymayor` doivent continuer à tourner.

## 3. Carte du code

### `crates/sim-core` (zéro dépendance, zéro `unsafe`)

| Fichier | Rôle | Points d'entrée |
|---|---|---|
| `fixed.rs` | Q48.16, `mul`/`div` via `i128`, `sqrt`, `lerp`, `clamp01` | `Fixed` |
| `rng.rs` | PCG32 déterministe, un flux par système | `Pcg32::new(seed, stream)` |
| `time.rs` | `Tick`, horloge à pas fixe, calendrier dérivé | `FixedStepClock`, `Calendar`, `TickBatch` |
| `grid.rs` | grille dense row-major, `TileId = y*width+x` | `Grid<T>` |
| `terrain.rs` | heightmap cm, eau à 0, terre par défaut à 500 cm, pente | `Terrain` |
| `zoning.rs` | une zone par tuile | `Zoning`, `ZoneType` |
| `spline.rs` | Bézier cubique : `eval`, `derivative`, `length`, courbure | `BezierCubic` |
| `road.rs` | nœuds + poignées + segments + CSR + rasterisation de desserte | `RoadNetwork` |
| `building.rs` | 1 bâtiment/tuile + slab compact des logements | `Buildings` |
| `population.rs` | census, emploi, vacance, bonheur EMA | `Census` |
| `demand.rs` | signaux RCI | `compute`, `DemandParams` |
| `land_value.rs` | valeur foncière, curseur borné | `LandValueLayer` |
| `economy.rs` | trésorerie, règlement mensuel | `Treasury` |
| `command.rs` | seule porte d'entrée des mutations | `Command`, `Rect` |
| `metrics.rs` | snapshot HUD + enum overlays | `Snapshot`, `Overlay` |
| `world.rs` | état complet + pipeline | `World`, `SimConfig`, `SimEvent`, `TickReport` |
| `tests/vertical_slice.rs` | scénarios causaux, déterminisme, échelle | 19 tests |

### `crates/render` (Bevy optionnel)

- `scene.rs` : transforme `&World` en `BuildingBlock`, `RoadStrip`,
  `TerrainMeshData` ; couleurs, hauteurs, centre de carte. **Aucun Bevy**,
  entièrement testé.
- `viewer.rs` : plugin/app Bevy, terrain, meshes fusionnés, lumière, caméra
  orbitale, captures. Compilé seulement avec `--features viewer`.

### `crates/app`

- `src/main.rs` : démo headless (horloge murale → ticks, commandes, snapshots).
- `src/bin/viewer.rs` : `city_render::viewer::run()`.

### Stubs documentés

- `sim-transport` : itération 3 (trafic/pathfinding). Le module explique déjà
  l'arbitrage agents individuels vs flot macroscopique et recommande l'hybride.
- `sim-economy` : itération 5.
- `ui` : itération 2b (HUD/outils).

## 4. Invariants critiques (à ne pas casser)

1. **Aucun `f32`/`f64` dans l'état simulé.** Conversions uniquement aux
   frontières (`scene.rs`, config d'hôte, affichage). `SimConfig` est 100 %
   entier + `Fixed`.
2. **Aucun accès au temps mural dans `sim-core`.** Seul `World::tick()` avance
   l'état. Ne jamais ajouter `Instant::now`, `SystemTime`, `thread::sleep`.
3. **Aucune itération de `HashMap`** dans un système. Les `HashMap` servent aux
   lookups (paire de nœuds → segment) ; toute boucle doit passer par `Vec`/`Grid`
   dans l'ordre des indices.
4. **Toute mutation passe par `Command`.** L'UI/le viewer ne touchent jamais
   `&mut World` en dehors de la file de commandes (le viewer est un cas
   particulier : il ne fait que lire).
5. **Un flux RNG par système.** Si un système consomme `World::rng_mut()`, il
   doit documenter quand et pourquoi ; sinon le test
   `different_seeds_do_not_change_a_deterministic_pipeline` échouera.
6. **`BuildingId` = index de tuile** (MVP). `Buildings::spawn` renvoie
   `BuildingId::new(tile.raw())`, `at(tile)` est O(1). Changer cela impose de
   revoir `built_tile_to_id` dans les tests, `Buildings::household_pos`, et
   `SimEvent::BuildingBuilt`.
7. **Un seul bâtiment par tuile.** `spawn` sur une tuile occupée renvoie `None`.
8. **`occupants <= capacity` toujours.** Le census et les tests d'intégration le
   vérifient.
9. **Les ids ne sont jamais réutilisés** : segments en tombstone (`Option`),
   bâtiments libérés par slot mais ids stables par tuile.
10. **`neighbors()` exige `ensure_topology()` à jour** (debug_assert). Le
    `World` le fait une fois par tick, pas par requête.
11. **Les poignées de nœud** : une seule poignée sortante par nœud. Une poignée
    n'est appliquée qu'à un nœud **créé** par la commande (sinon on déformerait
    les routes voisines).
12. **La desserte suit la courbe**, pas la corde (test dédié).
13. **`World::new` peut modifier `config`** : les budgets foncier/population sont
    dérivés de la surface (`tuiles/400`, bornés 4 096..65 536). `world.config()`
    est la vérité, pas la config passée.
14. **La demande est calculée en fin de tick** et consommée au tick suivant.
    C'est volontaire (évite un double census) ; ne pas « corriger » sans mesurer.
15. **Pas de démolition automatique.** Le joueur seul démolit. C'est le garde-fou
    anti death-wave avec le plancher de bonheur et l'émigration bornée.

## 5. Pipeline de tick, formules, défauts

### Pipeline (`World::tick`)

```
apply_commands          FIFO, validation, événements de refus
refresh_road_access     seulement si topologie modifiée (Bresenham sur courbe)
land_value              budget de tuiles, curseur persistant
move_population         slab compact des logements, flux entrant/sortant
grow                    quota composé par zone, tuiles desservies + pente
census                  un passage, bonheur EMA
demand                  RCI (déterministe, sans bruit)
budget                  si frontière de mois
```

Le **coût d'un tick ne dépend pas de la taille de la carte** : budgets bornés,
curseurs. C'est l'invariant de performance à préserver.

### Défauts `SimConfig`

| Champ | Valeur |
|---|---|
| `tick_hz` | 20 |
| `ticks_per_day` / `days_per_month` | 1200 / 30 |
| `tile_size_m` | 8 |
| `max_slope_cm` | 400 |
| `road_access_radius_tiles` | 2 |
| `road_snap_radius_m` | 1 m |
| `land_value_budget_per_tick` | 4096 (auto : `max(4096, tuiles/400)`, ≤ 65 536) |
| `population_budget_per_tick` | 4096 (même logique, ≤ 32 768) |
| `max_inflow_per_tick` | 6 (plancher) |
| `migration_fill_rate` | 5 % des logements vides / tick |
| `max_outflow_per_tick` | 4 |
| `growth_threshold` | 0,55 |
| `growth_rate_of_stock` | 1 % / tick |
| `growth_min_per_tick_per_zone` | 1 |
| `growth_max_per_tick_per_zone` | 64 |
| `vacate_happiness` | 0,45 |
| `labor_participation` | 0,60 |
| `happiness_smoothing` | 0,02 |

### Formules

```
demande_R = 0.25 + 0.50·emplois_ouverts + 0.35·(1 − 2·vacance) + 0.20·(bonheur − 0.5)
demande_C = (0.30 + 1.50·(0.30 − emplois_C/habitants)) · min(1, habitants/12)
demande_I = (0.30 + 1.20·(0.45 − emplois_I/habitants) + 0.30·chômage) · min(1, habitants/8)
bonheur   = 0.30 + 0.40·(1 − chômage) + 0.30·min(1, 2·vacance)   (EMA 2 %/tick)
valeur    = 0.30 + 0.25·desserte + bonus_de_zone                   (borné [0,1])
quota     = min(64, ceil(excès · max(1, ceil(1 % · stock))))
flux_in   = demande_R · max(6, ceil(5 % · logements vides))
flux_out  = (0.45 − bonheur)⁺ · 4
```

Le plancher de bonheur à 0,30 et le plafond d'émigration sous l'immigration de
base empêchent les vagues de départ. **Toute modification de ces constantes doit
passer par `jobless_city_does_not_collapse` et `long_run_keeps_every_invariant`.**

### Routes

- Géométrie : `c0 = a.pos + a.handle`, `c1 = b.pos − b.handle`.
- Longueur : somme de cordes sur `suggested_steps()` (pas cible ~2 m, borné 8..256).
- Courbure : cercle circonscrit à trois échantillons ; `None` si colinéaire.
- Desserte : échantillons → DDA tuile par tuile → disque de rayon `r`.
- `BuildRoad` = `BuildCurve` avec poignées nulles.
- Snap : un nœud existant à moins de 1 m est réutilisé (et sa poignée n'est pas
  touchée).

## 6. État vérifié

### Démo headless (`cargo run -p citymayor-app`)

Configuration : carte 48×48 (tuiles de 8 m), 200 ticks/jour, 1 jour/mois,
2 routes, 4 rectangles de zonage. Résultat à 640 ticks :

```
population 2528 / capacité 2528, emplois 735, chômage 781,
bonheur 0.496, bâtiments 114, segments 2, fonds 101 492 $,
valeur foncière moyenne 0.550
```

La ville sature son zonage : c'est attendu, le scénario ne zone que 117 tuiles.

### Test d'échelle (`growth_scales_to_a_million_inhabitant_map`)

Carte 256×256, 42 avenues tous les 6 rangs, 4 bandes zonées par avenue
(2 R, 1 C, 1 I). À 3 000 ticks : **> 150 000 habitants** (mesuré ~210 000),
> 9 000 bâtiments, 42 segments, vacance < 10 %. Aucune assertion de temps
(dépendante de la machine) ; le test vérifie les invariants et les ordres de
grandeur.

### Viewer (`cargo run -p citymayor-app --features viewer --bin city-viewer`)

- Carte 128×128, 10 avenues, 1 boulevard courbe, zonage mixte.
- Caméra : W/S/A/D ou flèches = déplacer, Q/E = tourner, R/F = incliner,
  Z/X (ou +/-) = zoom. Pas encore de souris.
- Lumière directionnelle, `GlobalAmbientLight`, matériaux Standard.
- Meshes reconstruits seulement quand `(bâtiments, segments, mois)` change.
- Capture automatique à ~4 s (`citymayor-auto.png`), F12 pour une capture
  horodatée. Les PNG sont gitignorés.
- Vérifié sur NVIDIA GTX 1080 Ti, Vulkan, driver 580.178.04.

## 7. Commandes

```bash
# Tests (sans Bevy, ~3 s la première fois)
cargo test --workspace
cargo test -p sim-core --lib
cargo test -p sim-core --test vertical_slice
cargo test -p city-render

# Qualité
cargo clippy --workspace --all-targets
cargo build --release -p citymayor-app

# Démo headless
cargo run -p citymayor-app

# Viewer 3D (compilation longue la première fois : ~1 min 15 en debug)
cargo run -p citymayor-app --features viewer --bin city-viewer

# Binaire déjà compilé
./target/debug/city-viewer
```

## 8. Pièges et bugs déjà rencontrés (ne pas réintroduire)

### Simulation

- **`Terrain::flat` à 0 = de l'eau.** Toute la carte était inconstructible.
  `flat` vaut désormais 500 cm ; l'eau est à 0 (`WATER_LEVEL_CM`).
- **Itérer la grille pour trouver des logements** : sur 256², le curseur ne
  recroisait le seul bâtiment que tous les 16 ticks, la ville ne démarrait
  jamais. Corrigé par le slab compact `Buildings::households` +
  `household_pos` (swap_remove O(1)).
- **Quota de croissance fixe** : remplir 60 000 bâtiments à 2/tick prenait des
  heures. Corrigé par la croissance composée (`growth_quota`).
- **Plafond d'immigration fixe** : la vacance restait haute, la demande R
  s'effondrait, la ville se figeait. Corrigé par le flux piloté par la vacance.
- **Zonage 100 % résidentiel** : la ville stagne, car sans emplois la demande R
  plafonne. Ce n'est **pas un bug**, c'est la boucle RCI. Les tests d'échelle
  zonent donc mixte.
- **La demande n'est pas un stock** : elle est recalculée chaque tick, jamais
  accumulée. Ne pas ajouter de « demande en attente » sans revoir la stabilité.

### Bevy 0.20.1

- `DefaultPlugins` inclut déjà `ScreenshotPlugin` : l'ajouter provoque un panic
  « plugin was already added ».
- Pour les captures PNG : features `png` **et** `tonemapping_luts` (sinon erreur
  de tonemapping) **et** un backend zstd (`zstd_rust`), sinon `bevy_image` refuse
  de compiler.
- `WindowResolution` se construit depuis un tuple d'**entiers** (`u32`), pas de
  flottants.
- `AmbientLight` est une **composante de caméra** en 0.20 ; la ressource globale
  est `GlobalAmbientLight`.
- `DirectionalLight::shadows_enabled` n'existe plus : c'est
  `shadow_maps_enabled`.
- `Assets::get_mut` exige `let Some(mut mesh) = ...` (le guard doit être `mut`).
- Les `ERROR` Vulkan `liblsfg-vk-layer.so` sont du bruit d'environnement
  (layer de frame-generation mal installée), pas une erreur de rendu.
- Le binaire debug avec Bevy pèse ~546 Mo ; prévoir l'espace disque.

### Cargo / workspace

- Les stubs `sim-transport`, `sim-economy`, `render`, `ui` n'ont **pas** de
  dépendance `sim-core` pour éviter les warnings `unused_dependencies` de Cargo
  1.100. Il faudra la rajouter au moment de les implémenter (et remettre les
  entrées dans `[workspace.dependencies]`, retirées pour la même raison).
- `city-render` est une dépendance **optionnelle** de `citymayor-app`, activée
  par la feature `viewer`.

## 9. Prochaines étapes (par ordre de priorité)

### Itération 2b — viewer utilisable

1. **Pré-simulation de la démo** : dans `viewer.rs::demo_city`, lancer
   `for _ in 0..1500 { world.tick(); }` avant de retourner le monde, pour ouvrir
   sur une ville peuplée. (Tentative non appliquée lors de la passation : le
   fichier est resté avec la ville vide au démarrage.)
2. **Vérifier le boulevard courbe** : le déplacer vers le centre de l'écran et
   augmenter la courbure (poignées latérales de même signe) ; contrôler que la
   desserte suit bien la courbe.
3. **Caméra souris** : orbite au clic-droit, pan au clic-milieu, zoom molette,
   amortissement (`lerp`), bornes de pitch et de distance.
4. **Visuel** : ciel clair, brouillard léger, normales de terrain calculées
   depuis la heightmap (actuellement `[0,1,0]` partout), ombres optionnelles.
5. **Overlays** : colorer les bâtiments/le sol via
   `World::overlay_cell(Overlay::Zone | LandValue | RoadAccess, tile)` ; ajouter
   un sélecteur clavier (1/2/3).
6. **HUD minimal** : date, population, emplois, chômage, trésorerie, demande RCI
   (lire `World::snapshot()` ; utiliser `bevy_ui` ou `egui` — recommandation :
   `egui` pour la vitesse de dev).
7. **Outils de construction** : machine à états dans `ui`, ghost avec validation
   (zonage, route, démolition), snapping, affichage des `CommandError`.
8. **Performance** : découper le terrain en chunks (32×32 tuiles par mesh),
   instancing des bâtiments au-delà de ~5 000, vérifier 60 fps sur 128².

**Critère de sortie 2b** : la fenêtre s'ouvre sur une ville peuplée, la caméra
se pilote à la souris, on peut zoner et tracer une route courbe à la main, les
overlays se basculent, et les 95 tests restent verts.

### Itération 3 — transport

- Ajouter `sim-core` comme dépendance de `sim-transport` (et au workspace).
- Ajouter un **compteur de version de topologie** (`topology_version: u64`)
  incrémenté à chaque `connect`/`disconnect`/mutation de poignée : le cache de
  chemins s'invalide dessus, pas sur un booléen.
- A* déterministe sur la CSR, tie-break par `(coût, NodeId)`.
- Cache de chemins étalé sur plusieurs ticks (curseur), jamais recalculé par
  frame.
- Hybride : flot macroscopique pour les trajets de fond, agents pour les
  véhicules proches de la caméra (LOD de simulation).
- Overlay trafic, budget de voirie, feux/priorités plus tard.

### Itération 4 — parcelles et services

- Parcelles multi-tuiles dérivées du front de rue (tuiles adjacentes à la
  desserte, groupées par segment) ; les splines donnent la courbure du front.
- Niveaux de bâtiment 1..3 (densité, capacité, revenus).
- Services : police, santé, éducation, couverture par BFS/flood sur la grille.
- Pollution/bruit : diffusion par cellule, termes dans `happiness_target`.

### Itération 5 — économie

- Entreprises, chaînes d'approvisionnement, marché foncier, budget des ménages.
- Fréquence réduite (1 tick sur N), système branché sur `tick.raw() % N == 0`.

### Itération 6 — production

- Sérialisation binaire versionnée (postcard/bincode) dans une crate dédiée.
- Replay `(seed, commandes)`, tests de non-régression sur 10 000 ticks.
- Benchmarks CI : tick < 5 ms sur 100 k bâtiments, balayage foncier borné.

## 10. Tests : ce que chacun protège

### Unitaires `sim-core`

- `fixed` : sémantique Q48.16, permille, floor/ceil négatifs, sqrt, lerp, clamp.
- `grid` : row-major, tailles, hors bornes → `None`.
- `rng` : même seed → même séquence, flux différents, `[0,1]`.
- `time` : 20 Hz, accumulation sous le pas, plafond de rattrapage + dette jetée,
  calendrier (jour/mois/heure, `day_progress`).
- `terrain` : plat constructible, eau non constructible, pente, taille validée.
- `zoning` : défaut non zoné, set renvoie l'ancienne valeur, index dense.
- `spline` : droite, quart de cercle (longueur 157,08 m, rayon ~100 m), courbe
  plus longue que la corde, extrémités exactes, dégénéré sans panic,
  déterminisme, tangentes aux extrémités.
- `road` : longueur, doublons/boucles, CSR bidirectionnel, sens unique,
  déconnexion, rachat de nœuds, G1 aux jonctions, poignées → longueur, desserte
  qui suit la courbe, polyligne, nœud inconnu.
- `building` : 1/tuile, remove, comptes par type, capacités, swap_remove du slab
  logements.
- `population` : emploi/vacance/chômage, cible de bonheur.
- `demand` : ville vide, logements pleins, pénurie de commerces, chômage,
  bornes `[0,1]`.
- `land_value` : non zoné = 0, route = valeur, budget étalé, dézonage.
- `economy` : impôts, déficit.
- `command` : normalisation, clipping, carte vide.
- `metrics` : seuls les overlays réels sont marqués implémentés.
- `world` : quota composé/plafonné/monotone, budgets des grandes cartes.

### Intégration (`vertical_slice.rs`)

Commandes au boundary, pas de route → pas de ville, route hors bornes refusée
sans effet de bord, doublon refusé, snapping, croissance + emplois, commerce/
industrie sans habitants, déterminisme sur 1 200 ticks, seeds indifférentes,
bulldoze, zonage qui ne remplace pas un bâtiment, budget mensuel, ville sans
logement, échelle 128², échelle 1 M (256²), route courbe qui dessert le
bombement, ville sans emplois qui ne s'effondre pas, invariants sur 5 000 ticks.

### `city-render`

Correspondance blocs↔bâtiments, hauteurs bornées, rubans courbes, maillage
terrain bien formé, déterminisme de la dérivation.

## 11. Dette technique / limites assumées

- Zonage par tuile : bâtiments alignés sur la grille, pas de front de rue.
- Pas de niveaux de bâtiment ni d'évolution.
- Pas de services publics, pollution, bruit, réseaux eau/électricité.
- Pas de sérialisation. L'état est POD, mais aucune crate ne le sérialise encore.
- Desserte rasterisée naïve : `O(segments × longueur × r²)` ; index spatial
  nécessaire au-delà de quelques milliers de segments.
- `set_node_handle` balaie les segments pour trouver les incidents (O(segments)).
  Un index nœud→segments sera nécessaire quand l'édition deviendra continue.
- `pair_index` est un `HashMap` : lookup uniquement, jamais itéré. Le garder
  ainsi.
- Le viewer fusionne tous les bâtiments en un seul mesh : simple, mais à
  remplacer par de l'instancing + chunks.
- Le terrain du viewer est plat (la heightmap existe mais n'est pas générée).
- Le budget est structurellement excédentaire (aucune charge de service).
- Les tests d'échelle n'ont pas d'assertion de temps : les benchmarks dédiés
  viendront en itération 6.

## 12. Questions encore ouvertes

1. **UI** : `bevy_ui` (intégré, mais verbeux) ou `egui` (rapide, très adapté aux
   panneaux de simulation) ? Recommandation : `egui` pour 2b.
2. **Parcelles** : découpage par front de rue à partir de la desserte, ou
   polygones explicites créés par l'outil ? Recommandation : front de rue dérivé
   de la desserte, polygones plus tard.
3. **Trafic** : seuil de bascule flot → agents (distance caméra ? nombre
   d'usagers ? budget CPU ?). À trancher avec un benchmark.
4. **Sauvegarde** : format binaire versionné + migrations, ou snapshots
   complets ? Recommandation : postcard + numéro de version + tests de
   round-trip.
5. **Terrain** : génération procédurale, eau, ponts/tunnels (les splines
   devront-elles devenir 3D ?). Non traité pour l'instant.
6. **Économie régionale** : une seule ville ou un marché inter-villes ? Hors
   périmètre MVP.

---

## Annexe A — conventions de code

- Commentaires et docs en **français**, identifiants en **anglais**.
- `#[must_use]` sur les accesseurs ; `Option` plutôt que panic hors bornes.
- Erreurs : enums locaux (`RoadError`, `CommandError`, `GridError`), `Display`
  + `std::error::Error`. Pas d'`anyhow` dans `sim-core`.
- Tests unitaires dans le module, tests d'intégration dans `tests/`.
- Toute constante de tuning va dans `SimConfig`/`*Params`, pas en dur dans un
  système.
- Tout nouveau système documente : entrée, sortie, coût par tick, invariant
  préservé, et son test de causalité.

## Annexe B — arborescence au moment de la passation

```
Cargo.toml
README.md
HANDOFF.md
crates/
  app/         main.rs (headless) + bin/viewer.rs
  render/      scene.rs, viewer.rs, Cargo.toml (Bevy optionnel)
  sim-core/    16 modules + tests/vertical_slice.rs
  sim-economy/ stub
  sim-transport/ stub
  ui/          stub
```

Pas de dépôt git initialisé au moment de la passation. Recommandation :
`git init && git add -A && git commit -m "Vertical slice: deterministic sim-core + curved roads + Bevy viewer"`,
puis travailler par branches/commits courts.
