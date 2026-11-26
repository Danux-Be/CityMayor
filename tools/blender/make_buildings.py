"""Textures urbaines de CityMayor : façades, toits, routes, trottoirs (Blender, sans interface).

    python3 tools/blender/fetch_urban_sources.py
    blender -b --factory-startup --python tools/blender/make_buildings.py -- crates/render/assets/urban

Chaque couche est une texture **répétable** de 1024², rendue en vue
orthographique à partir de vraie géométrie (fenêtres en retrait, appuis,
encadrements, volets, balcons, vitrines, marquages routiers) posée sur des
photos CC0 de Poly Haven (voir `sources/README.md`). Le relief est donc cuit
dans la texture : ombres sous les appuis, tableaux des fenêtres, etc.

Le jeu assemble ensuite les bâtiments (crates/render/src/urban.rs) : murs,
toits et trottoirs prennent ces couches dans un tableau de textures, avec des
coordonnées en mètres divisées par la taille de répétition de la couche
(`LAYERS` ci-dessous ↔ `urban::LAYERS`, même ordre).

Repère d'une cellule : X vers la droite, Y vers le haut de l'image (le haut
du mur pour une façade), Z vers la caméra. Les éléments qui débordent d'un
bord (poteaux d'usine) sont dupliqués sur le bord opposé : la texture se
raccorde sans couture.
"""

import math
import os
import random
import subprocess
import sys

import bpy

HERE = os.path.dirname(os.path.abspath(__file__))
SOURCES = os.path.join(HERE, "sources", "urban")
ARGS = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
OUT_DIR = os.path.abspath(ARGS[0] if ARGS else "urban")
SIZE = 1024
# Débord de la géométrie hors cadre : sans lui, le filtre de pixel mélange le
# fond aux pixels du bord et chaque répétition montre une couture claire.
M = 0.3

# (fichier, largeur m, hauteur m) — même ordre que `urban::LAYERS` en Rust.
LAYERS = [
    ("00_brique_fenetres", 6.0, 6.0),
    ("01_brique", 6.0, 6.0),
    ("02_enduit_fenetres", 6.0, 6.0),
    ("03_enduit", 6.0, 6.0),
    ("04_bardage_fenetres", 6.0, 6.0),
    ("05_bardage", 6.0, 6.0),
    ("06_immeuble", 6.0, 6.0),
    ("07_vitrine", 6.0, 4.5),
    ("08_bureaux", 6.0, 6.0),
    ("09_industrie", 8.0, 8.0),
    ("10_tuiles", 4.0, 4.0),
    ("11_ardoises", 4.0, 4.0),
    ("12_toit_tole", 4.0, 4.0),
    ("13_toit_plat", 8.0, 8.0),
    ("14_asphalte", 8.0, 8.0),
    ("15_marquage", 2.0, 2.0),
    ("16_trottoir", 4.0, 4.0),
    ("17_beton", 4.0, 4.0),
    ("18_pelouse", 6.0, 6.0),
    ("19_porte", 1.6, 2.8),
    ("20_chemin", 6.0, 6.0),
    ("21_chantier", 6.0, 6.0),
    ("22_fleche", 3.5, 7.0),
    ("23_glissiere", 4.0, 0.32),
    ("24_balise", 2.0, 1.0),
]

# Taille réelle (m) des photos Poly Haven : on les répète un nombre entier de
# fois par cellule pour que la couche reste raccordable.
SOURCE_SIZE = {
    "brick_wall_001": 3.0, "painted_plaster_wall": 2.0, "brown_planks_07": 1.0,
    "plaster_grey_04": 1.5, "rectangular_facade_tiles": 2.0, "corrugated_iron_02": 2.7,
    "clay_roof_tiles_02": 2.5, "roof_slates_02": 3.0, "box_profile_metal_sheet": 2.0,
    "gravel_concrete": 2.12, "asphalt_02": 3.0, "square_concrete_pavers": 1.8,
    "concrete_wall_008": 2.71, "leafy_grass": 2.0, "gravel_road": 2.0, "muddy_tracks": 2.25, "asphalt_04": 4.0,
}


# ------------------------------------------------------------------ scène

def reset():
    bpy.ops.wm.read_factory_settings(use_empty=True)
    scene = bpy.context.scene
    scene.render.engine = "CYCLES"
    scene.cycles.device = "CPU"
    scene.cycles.samples = 48
    try:
        scene.cycles.use_denoising = True
    except AttributeError:
        pass
    scene.render.film_transparent = False
    scene.view_settings.view_transform = "Standard"
    scene.render.image_settings.file_format = "PNG"
    scene.render.image_settings.color_mode = "RGB"

    # Éclairage : ciel uniforme (rendu ≈ albédo) + soleil faible venant d'en
    # haut à gauche, pour cuire de petites ombres portées sous les reliefs.
    world = bpy.data.worlds.new("ciel")
    world.use_nodes = True
    bg = next(n for n in world.node_tree.nodes if n.type == "BACKGROUND")
    bg.inputs["Color"].default_value = (1, 1, 1, 1)
    bg.inputs["Strength"].default_value = 0.82
    scene.world = world
    sun = bpy.data.lights.new("soleil", "SUN")
    sun.energy = 1.1
    sun.angle = math.radians(6)
    sun_obj = bpy.data.objects.new("soleil", sun)
    sun_obj.rotation_euler = (math.radians(-38), math.radians(-14), 0)
    scene.collection.objects.link(sun_obj)
    return scene


def camera(scene, w, h):
    cam = bpy.data.cameras.new("cam")
    cam.type = "ORTHO"
    cam.ortho_scale = max(w, h)
    obj = bpy.data.objects.new("cam", cam)
    obj.location = (w / 2, h / 2, 20)
    scene.collection.objects.link(obj)
    scene.camera = obj
    # Pixels carrés en mètres ; on redimensionne en 1024² après le rendu.
    scale = SIZE / max(w, h)
    scene.render.resolution_x = round(w * scale)
    scene.render.resolution_y = round(h * scale)
    scene.render.resolution_percentage = 100


# -------------------------------------------------------------- matériaux

def photo(name, cell_w, cell_h, tint=(1, 1, 1), saturation=1.0, value=1.0, roughness=0.8,
          normal_strength=1.0):
    """Matériau photo répété un nombre entier de fois dans la cellule."""
    mat = bpy.data.materials.new(name)
    mat.use_nodes = True
    nt = mat.node_tree
    bsdf = next(n for n in nt.nodes if n.type == "BSDF_PRINCIPLED")
    bsdf.inputs["Roughness"].default_value = roughness
    coords = nt.nodes.new("ShaderNodeTexCoord")
    mapping = nt.nodes.new("ShaderNodeMapping")
    size = SOURCE_SIZE[name]
    mapping.inputs["Scale"].default_value = (
        max(1, round(cell_w / size)) / cell_w,
        max(1, round(cell_h / size)) / cell_h,
        1,
    )
    nt.links.new(coords.outputs["Object"], mapping.inputs["Vector"])

    diff = nt.nodes.new("ShaderNodeTexImage")
    diff.image = bpy.data.images.load(os.path.join(SOURCES, f"{name}_diff.jpg"))
    nt.links.new(mapping.outputs["Vector"], diff.inputs["Vector"])
    hsv = nt.nodes.new("ShaderNodeHueSaturation")
    hsv.inputs["Saturation"].default_value = saturation
    hsv.inputs["Value"].default_value = value
    nt.links.new(diff.outputs["Color"], hsv.inputs["Color"])
    mix = nt.nodes.new("ShaderNodeMix")
    mix.data_type = "RGBA"
    mix.blend_type = "MULTIPLY"
    mix.inputs["Factor"].default_value = 1.0
    nt.links.new(hsv.outputs["Color"], mix.inputs["A"])
    mix.inputs["B"].default_value = (*tint, 1)
    nt.links.new(mix.outputs["Result"], bsdf.inputs["Base Color"])

    nor = nt.nodes.new("ShaderNodeTexImage")
    nor.image = bpy.data.images.load(os.path.join(SOURCES, f"{name}_nor.jpg"))
    nor.image.colorspace_settings.name = "Non-Color"
    nt.links.new(mapping.outputs["Vector"], nor.inputs["Vector"])
    nmap = nt.nodes.new("ShaderNodeNormalMap")
    nmap.inputs["Strength"].default_value = normal_strength
    nt.links.new(nor.outputs["Color"], nmap.inputs["Color"])
    nt.links.new(nmap.outputs["Normal"], bsdf.inputs["Normal"])
    return mat


def flat(name, color, roughness=0.6, metallic=0.0):
    mat = bpy.data.materials.new(name)
    mat.use_nodes = True
    bsdf = next(n for n in mat.node_tree.nodes if n.type == "BSDF_PRINCIPLED")
    bsdf.inputs["Base Color"].default_value = (*color, 1)
    bsdf.inputs["Roughness"].default_value = roughness
    bsdf.inputs["Metallic"].default_value = metallic
    return mat


def glass(name="vitre", top=(0.32, 0.38, 0.44), bottom=(0.05, 0.065, 0.08)):
    """Vitre vue de face : sombre en bas, reflet du ciel en haut (dégradé)."""
    mat = bpy.data.materials.new(name)
    mat.use_nodes = True
    nt = mat.node_tree
    bsdf = next(n for n in nt.nodes if n.type == "BSDF_PRINCIPLED")
    bsdf.inputs["Roughness"].default_value = 0.12
    coords = nt.nodes.new("ShaderNodeTexCoord")
    sep = nt.nodes.new("ShaderNodeSeparateXYZ")
    nt.links.new(coords.outputs["Generated"], sep.inputs["Vector"])
    ramp = nt.nodes.new("ShaderNodeValToRGB")
    ramp.color_ramp.elements[0].position = 0.25
    ramp.color_ramp.elements[0].color = (*bottom, 1)
    ramp.color_ramp.elements[1].position = 1.0
    ramp.color_ramp.elements[1].color = (*top, 1)
    nt.links.new(sep.outputs["Y"], ramp.inputs["Fac"])
    nt.links.new(ramp.outputs["Color"], bsdf.inputs["Base Color"])
    return mat


# ------------------------------------------------------------- géométrie

def box(name, x0, y0, z0, x1, y1, z1, mat):
    """Pavé aligné sur les axes."""
    mesh = bpy.data.meshes.new(name)
    v = [(x0, y0, z0), (x1, y0, z0), (x1, y1, z0), (x0, y1, z0),
         (x0, y0, z1), (x1, y0, z1), (x1, y1, z1), (x0, y1, z1)]
    f = [(0, 3, 2, 1), (4, 5, 6, 7), (0, 1, 5, 4), (1, 2, 6, 5), (2, 3, 7, 6), (3, 0, 4, 7)]
    mesh.from_pydata(v, [], f)
    obj = bpy.data.objects.new(name, mesh)
    obj.data.materials.append(mat)
    bpy.context.scene.collection.objects.link(obj)
    return obj


def quad(name, pts, mat):
    mesh = bpy.data.meshes.new(name)
    mesh.from_pydata(pts, [], [(0, 1, 2, 3)])
    obj = bpy.data.objects.new(name, mesh)
    obj.data.materials.append(mat)
    bpy.context.scene.collection.objects.link(obj)
    return obj


def wall_with_holes(w, h, holes, mat, depth=0.18, reveal_mat=None):
    """Mur plan (z = 0) percé de trous rectangulaires, avec tableaux en retrait.

    Le mur est découpé en grille selon les bords des trous ; les cases qui ne
    tombent dans aucun trou deviennent des faces. Les tableaux (côtés du
    retrait) reçoivent l'ombre du soleil rasant : c'est ce qui donne de la
    profondeur aux fenêtres vues de face.
    """
    xs = sorted({-M, w + M, *[x for hx0, _, hx1, _ in holes for x in (hx0, hx1)]})
    ys = sorted({-M, h + M, *[y for _, hy0, _, hy1 in holes for y in (hy0, hy1)]})
    verts, faces = [], []
    for i in range(len(xs) - 1):
        for j in range(len(ys) - 1):
            cx, cy = (xs[i] + xs[i + 1]) / 2, (ys[j] + ys[j + 1]) / 2
            if any(hx0 < cx < hx1 and hy0 < cy < hy1 for hx0, hy0, hx1, hy1 in holes):
                continue
            b = len(verts)
            verts += [(xs[i], ys[j], 0), (xs[i + 1], ys[j], 0), (xs[i + 1], ys[j + 1], 0), (xs[i], ys[j + 1], 0)]
            faces.append((b, b + 1, b + 2, b + 3))
    for hx0, hy0, hx1, hy1 in holes:
        for (ax, ay), (bx, by) in (((hx0, hy0), (hx1, hy0)), ((hx1, hy0), (hx1, hy1)),
                                   ((hx1, hy1), (hx0, hy1)), ((hx0, hy1), (hx0, hy0))):
            b = len(verts)
            verts += [(ax, ay, 0), (bx, by, 0), (bx, by, -depth), (ax, ay, -depth)]
            faces.append((b, b + 1, b + 2, b + 3))
    mesh = bpy.data.meshes.new("mur")
    mesh.from_pydata(verts, [], faces)
    obj = bpy.data.objects.new("mur", mesh)
    obj.data.materials.append(mat)
    bpy.context.scene.collection.objects.link(obj)
    return obj


def window(x0, y0, x1, y1, frame, pane, depth=0.18, mullion=True, transom=False, curtain=None,
           rng=None):
    """Fenêtre : vitre au fond du retrait, dormant, meneau, rideau éventuel."""
    t = 0.07
    z = -depth + 0.04
    quad("vitre", [(x0, y0, -depth), (x1, y0, -depth), (x1, y1, -depth), (x0, y1, -depth)], pane)
    if curtain is not None and rng is not None and rng.random() < 0.7:
        drop = (y1 - y0) * rng.uniform(0.15, 0.55)
        quad("rideau", [(x0, y1 - drop, -depth + 0.02), (x1, y1 - drop, -depth + 0.02),
                        (x1, y1, -depth + 0.02), (x0, y1, -depth + 0.02)], curtain)
    # Pièces jointives sans chevauchement : un rayon qui part de l'intérieur
    # d'un autre pavé donne des points noirs aux angles.
    box("dormant_b", x0, y0, -depth, x1, y0 + t, z, frame)
    box("dormant_h", x0, y1 - t, -depth, x1, y1, z, frame)
    box("dormant_g", x0, y0 + t, -depth, x0 + t, y1 - t, z, frame)
    box("dormant_d", x1 - t, y0 + t, -depth, x1, y1 - t, z, frame)
    xm = (x0 + x1) / 2
    ym = y1 - (y1 - y0) * 0.28
    if mullion:
        if transom:
            box("meneau", xm - t / 2, y0 + t, -depth, xm + t / 2, ym - t / 2, z, frame)
            box("meneau", xm - t / 2, ym + t / 2, -depth, xm + t / 2, y1 - t, z, frame)
        else:
            box("meneau", xm - t / 2, y0 + t, -depth, xm + t / 2, y1 - t, z, frame)
    if transom:
        box("traverse", x0 + t, ym - t / 2, -depth, x1 - t, ym + t / 2, z, frame)


def sill(x0, x1, y, mat, depth=0.08):
    box("appui", x0 - 0.06, y - 0.07, -0.02, x1 + 0.06, y, depth, mat)


# ---------------------------------------------------------------- couches

def facade_house(scene, wall, w, h, with_windows, shutters=None, lintel=None, seed=1):
    rng = random.Random(seed)
    frame = flat("dormant", (0.86, 0.86, 0.84), 0.45)
    pane = glass()
    curtain = flat("rideau", (0.80, 0.76, 0.68), 0.9)
    stone = flat("pierre", (0.72, 0.70, 0.66), 0.8)
    holes = []
    if with_windows:
        for floor in range(2):
            for bay in range(2):
                cx, base = 1.5 + 3.0 * bay, 3.0 * floor
                holes.append((cx - 0.62, base + 0.9, cx + 0.62, base + 2.35))
    wall_with_holes(w, h, holes, wall)
    for hx0, hy0, hx1, hy1 in holes:
        window(hx0, hy0, hx1, hy1, frame, pane, transom=True, curtain=curtain, rng=rng)
        sill(hx0, hx1, hy0, stone)
        if lintel is not None:
            box("linteau", hx0 - 0.08, hy1, -0.01, hx1 + 0.08, hy1 + 0.2, 0.03, lintel)
        if shutters is not None:
            for sx0 in (hx0 - 0.6, hx1):
                box("volet", sx0, hy0 - 0.02, 0.0, sx0 + 0.58, hy1 + 0.02, 0.05, shutters)
                for k in range(1, 9):
                    yy = hy0 + (hy1 - hy0) * k / 9
                    box("lame", sx0 + 0.04, yy - 0.012, 0.05, sx0 + 0.54, yy + 0.012, 0.065, shutters)
    # Bandeau d'étage (cordon) : marque les niveaux, cache le raccord.
    if with_windows:
        for y in (0.0, 3.0):
            box("cordon", -M, y, 0, w + M, y + 0.12, 0.04, stone)


def layer_brick(scene, windows):
    wall = photo("brick_wall_001", 6, 6, saturation=0.9)
    lintel = flat("linteau", (0.62, 0.58, 0.52), 0.8)
    facade_house(scene, wall, 6, 6, windows, lintel=lintel, seed=11)


def layer_plaster(scene, windows):
    wall = photo("painted_plaster_wall", 6, 6, saturation=0.2, value=1.08, normal_strength=0.6)
    shutters = flat("volets", (0.36, 0.44, 0.40), 0.6)
    facade_house(scene, wall, 6, 6, windows, shutters=shutters, seed=12)


def layer_siding(scene, windows):
    wall = photo("brown_planks_07", 6, 6, saturation=0.15, value=1.35)
    trim = flat("chambranle", (0.9, 0.9, 0.88), 0.5)
    facade_house(scene, wall, 6, 6, windows, lintel=trim, seed=13)
    for x in (0.0, 6.0):  # planches d'angle (raccord : moitié de chaque côté)
        box("angle", x - 0.08, -M, 0.045, x + 0.08, 6 + M, 0.075, trim)


def layer_apartment(scene):
    rng = random.Random(21)
    wall = photo("plaster_grey_04", 6, 6, tint=(0.95, 0.93, 0.9), saturation=0.4, value=1.1)
    frame = flat("dormant", (0.25, 0.26, 0.27), 0.5)
    pane = glass()
    curtain = flat("rideau", (0.82, 0.78, 0.70), 0.9)
    slab = flat("dalle", (0.80, 0.79, 0.77), 0.7)
    rail = flat("garde-corps", (0.22, 0.23, 0.24), 0.4, 0.6)
    frosted = flat("verre_depoli", (0.62, 0.68, 0.70), 0.3)
    holes = []
    for floor in range(2):
        base = 3.0 * floor
        holes.append((0.65, base + 0.25, 2.35, base + 2.45))   # porte-fenêtre (balcon)
        holes.append((3.75, base + 0.95, 5.25, base + 2.45))   # fenêtre
    wall_with_holes(6, 6, holes, wall)
    for hx0, hy0, hx1, hy1 in holes:
        window(hx0, hy0, hx1, hy1, frame, pane, curtain=curtain, rng=rng)
        if hy0 > 0.5 + 3.0 * (hy0 // 3.0):
            sill(hx0, hx1, hy0, slab)
    for floor in range(2):
        base = 3.0 * floor
        box("balcon", 0.35, base + 0.05, 0, 2.65, base + 0.22, 1.2, slab)
        box("panneau", 0.4, base + 0.22, 1.1, 2.6, base + 1.15, 1.16, frosted)
        box("main_courante", 0.35, base + 1.12, 1.08, 2.65, base + 1.2, 1.2, rail)
        box("bandeau", -M, base, 0, 6 + M, base + 0.08, 0.05, slab)


def layer_shop(scene):
    rng = random.Random(31)
    wall = photo("concrete_wall_008", 6, 4.5, tint=(0.95, 0.94, 0.92), saturation=0.3, value=1.15)
    frame = flat("menuiserie", (0.12, 0.13, 0.14), 0.4, 0.7)
    pane = glass(top=(0.42, 0.46, 0.5), bottom=(0.10, 0.11, 0.12))
    goods = [flat(f"etal{i}", c, 0.8) for i, c in enumerate(
        [(0.7, 0.25, 0.2), (0.85, 0.7, 0.3), (0.3, 0.45, 0.6), (0.85, 0.85, 0.82), (0.35, 0.55, 0.3)])]
    sign = flat("enseigne", (0.16, 0.17, 0.19), 0.5)
    letters = flat("lettres", (0.92, 0.90, 0.85), 0.5)
    awning = flat("store", (0.55, 0.16, 0.14), 0.9)
    stripe = flat("store_raye", (0.92, 0.9, 0.86), 0.9)
    holes = [(0.35, 0.3, 3.65, 3.05), (4.0, 0.0, 5.65, 2.6)]
    wall_with_holes(6, 4.5, holes, wall, depth=0.25)
    # Vitrine : étals colorés derrière le verre (silhouettes de produits).
    for k in range(14):
        x = 0.5 + k * 0.22
        if x + 0.18 > 1.95 and x < 2.05:  # meneau
            continue
        box("produit", x, 0.38, -0.19, x + 0.18, 0.38 + rng.uniform(0.3, 1.2), -0.17, rng.choice(goods))
    window(0.35, 0.3, 3.65, 3.05, frame, pane, depth=0.2, mullion=True)
    # Porte vitrée.
    window(4.0, 0.0, 5.65, 2.6, frame, pane, depth=0.2, mullion=True, transom=True)
    box("seuil", 3.95, 0, 0, 5.7, 0.06, 0.15, frame)
    # Bandeau d'enseigne et lettres stylisées.
    box("bandeau", 0.2, 3.3, 0, 5.8, 4.05, 0.12, sign)
    x = 0.7
    while x < 5.1:
        lw = rng.uniform(0.18, 0.32)
        box("lettre", x, 3.48, 0.12, x + lw, 3.87, 0.14, letters)
        x += lw + rng.uniform(0.06, 0.25)
    # Store banne rayé au-dessus de la vitrine.
    for k in range(12):
        x0 = 0.3 + k * 0.29
        quad("store", [(x0, 3.12, 0.05), (x0 + 0.29, 3.12, 0.05), (x0 + 0.29, 2.75, 0.9), (x0, 2.75, 0.9)],
             awning if k % 2 == 0 else stripe)
    box("socle", -M, 0, 0, 6 + M, 0.3, 0.05, flat("soubassement", (0.35, 0.35, 0.36), 0.8))


def layer_office(scene):
    rng = random.Random(41)
    spandrel = photo("rectangular_facade_tiles", 6, 6, tint=(1.05, 1.05, 1.05), saturation=0.3)
    frame = flat("profil", (0.55, 0.57, 0.6), 0.35, 0.8)
    pane = glass(top=(0.45, 0.55, 0.62), bottom=(0.12, 0.16, 0.2))
    blind = flat("store", (0.75, 0.75, 0.72), 0.8)
    holes = [(-M, 0.85 + 3.0 * f, 6 + M, 2.75 + 3.0 * f) for f in range(2)]
    wall_with_holes(6, 6, holes, spandrel, depth=0.12)
    for hx0, hy0, hx1, hy1 in holes:
        quad("vitre", [(hx0, hy0, -0.12), (hx1, hy0, -0.12), (hx1, hy1, -0.12), (hx0, hy1, -0.12)], pane)
        for k in range(4):
            if rng.random() < 0.6:
                drop = (hy1 - hy0) * rng.uniform(0.1, 0.6)
                quad("store", [(k * 1.5, hy1 - drop, -0.11), (k * 1.5 + 1.5, hy1 - drop, -0.11),
                               (k * 1.5 + 1.5, hy1, -0.11), (k * 1.5, hy1, -0.11)], blind)
        for k in range(5):  # montants (k = 0 et 4 : raccord des deux bords)
            x = k * 1.5
            box("montant", x - 0.04, hy0 + 0.04, -0.12, x + 0.04, hy1 - 0.04, 0.02, frame)
        box("traverse", -M, hy0 - 0.04, -0.12, 6 + M, hy0 + 0.04, 0.02, frame)
        box("traverse", -M, hy1 - 0.04, -0.12, 6 + M, hy1 + 0.04, 0.02, frame)


def layer_industrial(scene):
    wall = photo("corrugated_iron_02", 8, 8, tint=(0.92, 0.95, 0.98), saturation=0.4, value=1.15)
    steel = flat("acier", (0.30, 0.34, 0.38), 0.5, 0.6)
    panel = flat("polycarbonate", (0.70, 0.74, 0.72), 0.3)
    base = photo("concrete_wall_008", 8, 8, saturation=0.4)
    holes = [(0.3, 5.2, 7.7, 6.6)]
    wall_with_holes(8, 8, holes, wall, depth=0.1)
    quad("bandeau", [(0.3, 5.2, -0.1), (7.7, 5.2, -0.1), (7.7, 6.6, -0.1), (0.3, 6.6, -0.1)], panel)
    for k in range(1, 8):
        box("raidisseur", 0.3 + k * (7.4 / 8) - 0.02, 5.2, -0.1, 0.3 + k * (7.4 / 8) + 0.02, 6.6, -0.05, steel)
    for x in (0.0, 8.0):  # poteaux sur les bords (raccord)
        box("poteau", x - 0.18, 0, 0, x + 0.18, 8, 0.2, steel)
    box("muret", -M, -M, 0, 8 + M, 1.2, 0.08, base)
    box("couvertine", -M, 1.2, 0, 8 + M, 1.28, 0.12, steel)


def layer_roof(scene, name, w, h, **kw):
    mat = photo(name, w, h, **kw)
    quad("toit", [(-M, -M, 0), (w + M, -M, 0), (w + M, h + M, 0), (-M, h + M, 0)], mat)


def layer_road(scene, lanes):
    w, h = 3.5 * lanes, 8.0
    asphalt = photo("asphalt_02", w, h, saturation=0.3, value=0.95)
    paint = flat("peinture", (0.86, 0.86, 0.82), 0.6)
    quad("chaussee", [(-M, -M, 0), (w + M, -M, 0), (w + M, h + M, 0), (-M, h + M, 0)], asphalt)
    for x in (0.35, w - 0.35):  # lignes de rive
        box("rive", x - 0.075, -M, 0, x + 0.075, h + M, 0.005, paint)
    mid = w / 2
    if lanes == 2:
        box("axe", mid - 0.07, 1.0, 0, mid + 0.07, 4.0, 0.005, paint)
    else:
        for x in (mid - 0.16, mid + 0.16):
            box("axe", x - 0.07, -M, 0, x + 0.07, h + M, 0.005, paint)
        for x in (3.5, w - 3.5):
            box("voie", x - 0.07, 1.0, 0, x + 0.07, 4.0, 0.005, paint)


def road_paint():
    """Peinture routière : blanc légèrement usé (bruit), mate."""
    mat = bpy.data.materials.new("peinture")
    mat.use_nodes = True
    nt = mat.node_tree
    bsdf = next(n for n in nt.nodes if n.type == "BSDF_PRINCIPLED")
    bsdf.inputs["Roughness"].default_value = 0.55
    noise = nt.nodes.new("ShaderNodeTexNoise")
    noise.inputs["Scale"].default_value = 18.0
    noise.inputs["Detail"].default_value = 8.0
    ramp = nt.nodes.new("ShaderNodeValToRGB")
    ramp.color_ramp.elements[0].position = 0.35
    ramp.color_ramp.elements[0].color = (0.78, 0.78, 0.76, 1)
    ramp.color_ramp.elements[1].position = 0.6
    ramp.color_ramp.elements[1].color = (0.9, 0.9, 0.87, 1)
    nt.links.new(noise.outputs["Fac"], ramp.inputs["Fac"])
    nt.links.new(ramp.outputs["Color"], bsdf.inputs["Base Color"])
    return mat


def layer_asphalt(scene):
    layer_roof(scene, "asphalt_04", 8, 8, saturation=0.2, value=0.42)


def layer_marking(scene):
    quad("peinture", [(-M, -M, 0), (2 + M, -M, 0), (2 + M, 2 + M, 0), (-M, 2 + M, 0)], road_paint())


def layer_arrow(scene):
    """Flèche directionnelle au sol (sens de circulation vers le haut de l'image)."""
    w, h = 3.5, 7.0
    asphalt = photo("asphalt_04", 8, 8, saturation=0.2, value=0.42)
    quad("chaussee", [(-M, -M, 0), (w + M, -M, 0), (w + M, h + M, 0), (-M, h + M, 0)], asphalt)
    paint = road_paint()
    mid = w / 2
    box("hampe", mid - 0.15, 1.2, 0, mid + 0.15, 4.6, 0.005, paint)
    mesh = bpy.data.meshes.new("pointe")
    mesh.from_pydata([(mid - 0.6, 4.6, 0.005), (mid + 0.6, 4.6, 0.005), (mid, 5.9, 0.005)], [], [(0, 1, 2)])
    obj = bpy.data.objects.new("pointe", mesh)
    obj.data.materials.append(paint)
    bpy.context.scene.collection.objects.link(obj)


def layer_guardrail(scene):
    """Glissière métallique vue de face : lisse en W galvanisée (32 cm)."""
    w, h = 4.0, 0.32
    steel = flat("acier_galva", (0.5, 0.52, 0.54), 0.45, 0.6)
    dark = flat("poteau", (0.32, 0.33, 0.35), 0.5, 0.8)
    back = flat("fond", (0.05, 0.05, 0.05), 1.0)
    # La couche entière est la lisse en W (le jeu pose les poteaux à part) :
    # trois bandes inclinées alternées sur toute la hauteur de la cellule.
    del back, dark
    bands = [(-M, 0.11, 0.0, 0.03), (0.11, 0.21, 0.03, 0.0), (0.21, h + M, 0.0, 0.03)]
    for k, (y0, y1, z0, z1) in enumerate(bands):
        quad(f"lisse{k}", [(-M, y0, z0), (w + M, y0, z0), (w + M, y1, z1), (-M, y1, z1)], steel)
    for x in (0.0, 4.0):  # boulons de jonction (raccord des deux bords)
        box("boulon", x - 0.05, 0.12, 0.03, x + 0.05, 0.2, 0.05, steel)


def layer_beacon(scene):
    """Balise de chantier rayée rouge et blanc (vue de face)."""
    w, h = 2.0, 1.0
    red = flat("rouge", (0.72, 0.06, 0.04), 0.5)
    white = flat("blanc", (0.92, 0.92, 0.9), 0.5)
    for k in range(-1, 9):
        x0 = k * 0.25
        mat = red if k % 2 == 0 else white
        quad("bande", [(x0, -M, 0), (x0 + 0.25, -M, 0), (x0 + 0.25 + 0.4, h + M, 0), (x0 + 0.4, h + M, 0)], mat)


def layer_door(scene):
    w, h = 1.6, 2.8
    surround = flat("encadrement", (0.78, 0.76, 0.72), 0.8)
    wood = flat("porte", (0.30, 0.18, 0.11), 0.55)
    knob = flat("poignee", (0.75, 0.62, 0.3), 0.3, 1.0)
    pane = glass()
    wall_with_holes(w, h, [(0.25, 0.1, 1.35, 2.35), (0.3, 2.42, 1.3, 2.68)], surround, depth=0.1)
    box("porte", 0.25, 0.1, -0.1, 1.35, 2.35, -0.08, wood)
    for y0, y1 in ((0.3, 1.1), (1.25, 2.15)):
        for x0, x1 in ((0.38, 0.75), (0.85, 1.22)):
            box("panneau", x0, y0, -0.08, x1, y1, -0.065, wood)
    quad("imposte", [(0.3, 2.42, -0.1), (1.3, 2.42, -0.1), (1.3, 2.68, -0.1), (0.3, 2.68, -0.1)], pane)
    box("poignee", 1.17, 1.15, -0.08, 1.24, 1.22, -0.03, knob)
    box("marche", 0.1, 0, 0, 1.5, 0.1, 0.25, surround)


def render(scene, out, w, h):
    tmp = out + ".png"
    scene.render.filepath = tmp
    bpy.ops.render.render(write_still=True)
    subprocess.run(["magick", tmp, "-resize", f"{SIZE}x{SIZE}!", "-quality", "90", out], check=True)
    os.remove(tmp)


def main():
    os.makedirs(OUT_DIR, exist_ok=True)
    builders = {
        "00_brique_fenetres": lambda s: layer_brick(s, True),
        "01_brique": lambda s: layer_brick(s, False),
        "02_enduit_fenetres": lambda s: layer_plaster(s, True),
        "03_enduit": lambda s: layer_plaster(s, False),
        "04_bardage_fenetres": lambda s: layer_siding(s, True),
        "05_bardage": lambda s: layer_siding(s, False),
        "06_immeuble": layer_apartment,
        "07_vitrine": layer_shop,
        "08_bureaux": layer_office,
        "09_industrie": layer_industrial,
        "10_tuiles": lambda s: layer_roof(s, "clay_roof_tiles_02", 4, 4, saturation=0.85),
        "11_ardoises": lambda s: layer_roof(s, "roof_slates_02", 4, 4, saturation=0.5, value=0.8),
        "12_toit_tole": lambda s: layer_roof(s, "box_profile_metal_sheet", 4, 4, saturation=0.15,
                                            value=1.6),
        "13_toit_plat": lambda s: layer_roof(s, "gravel_concrete", 8, 8, saturation=0.3),
        "14_asphalte": layer_asphalt,
        "15_marquage": layer_marking,
        "16_trottoir": lambda s: layer_roof(s, "square_concrete_pavers", 4, 4, saturation=0.12,
                                           value=1.25),
        "17_beton": lambda s: layer_roof(s, "concrete_wall_008", 4, 4, saturation=0.3),
        "18_pelouse": lambda s: layer_roof(s, "leafy_grass", 6, 6, tint=(0.8, 1.0, 0.7),
                                          saturation=1.2),
        "19_porte": layer_door,
        "20_chemin": lambda s: layer_roof(s, "gravel_road", 6, 6, saturation=0.8),
        "21_chantier": lambda s: layer_roof(s, "muddy_tracks", 6, 6, saturation=0.9),
        "22_fleche": layer_arrow,
        "23_glissiere": layer_guardrail,
        "24_balise": layer_beacon,
    }
    only = set(ARGS[1:])
    for name, w, h in LAYERS:
        if only and name not in only:
            continue
        scene = reset()
        camera(scene, w, h)
        builders[name](scene)
        render(scene, os.path.join(OUT_DIR, f"{name}.jpg"), w, h)
        print("couche", name)


main()
