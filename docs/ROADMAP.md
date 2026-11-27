# Feuille de route produit

## Exigences (fixées le 2026-11-26)

1. Jeu prévu pour l'i18n, avec des fichiers de langue séparés (`locales/`).
2. Anglais uniquement dans un premier temps.
3. Jeu le plus complet possible.
4. Bâtiments avec plusieurs stades de construction.
5. Blender pour modéliser les bâtiments quand il le faut.
6. Routes de divers types : chantiers, une et deux voies, sens unique ou double sens, avenues, autoroutes…
7. Ronds-points.
8. Textures les plus détaillées possible.
9. Niveau d'un jeu commercial actuel (référence : Cities: Skylines 2).

## Étapes

| # | Étape | Contenu | État |
|---|---|---|---|
| 1 | i18n | `city-i18n` (Fluent), `locales/en`, clés de message dans `sim-core`, tests de complétude | fait |
| 2 | Routes | catalogue de types (chemin, rue 2 voies, sens unique 1 et 2 voies, avenue 4 voies, boulevard à terre-plein, autoroute 2×2 et 2×3 avec bretelles), chantier de route (route en construction, coût, durée) | fait (sauf coût de construction et échangeurs dénivelés) |
| 3 | Ronds-points | nœud giratoire dans le graphe (sens de circulation, priorité), géométrie et marquages, outil de pose | fait (sauf outil de pose, qui vient avec le HUD) |
| 4 | Construction des bâtiments | stades dans `sim-core` (terrain préparé → fondations → gros œuvre → finitions → occupé), progression déterministe, rendu de chaque stade (palissade, fondations, échafaudages, grue) modélisé dans Blender | fait |
| 5 | HUD et outils | barre d'outils, zonage au pinceau, tracé de routes courbes, démolition, panneaux (budget, demande, infos), vitesse du temps ; tous les textes traduits | en partie : barre d'infos, vitesse, outils route / rond-point / zonage / démolition faits ; panneaux budget et infos, vues d'information à venir |
| 6 | Rendu commercial | textures 2K avec normales et rugosité en jeu, plus de modèles de bâtiments par niveau de richesse, nuit et fenêtres éclairées, eau réfléchissante, occlusion ambiante | à faire |
| 7 | Trafic | A* déterministe, véhicules animés, feux et priorités, overlay de trafic | à faire |
| 8 | Services et économie | électricité, eau, police, santé, éducation, pollution, niveaux de bâtiments, budget détaillé | à faire |
| 9 | Production | sauvegarde/chargement, options (langue, graphismes), menus, sons | à faire |
