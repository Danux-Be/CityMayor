"""Arbres de CityMayor : cartes de feuillage texturées + imposteurs (Blender, sans interface).

    blender -b --factory-startup --python tools/blender/make_trees.py -- crates/render/assets/trees

Produit :
  atlas.png              2048² RGBA : cartes de feuillage, écorces, imposteurs
  <essence>_lod0.glb     tronc + branches + ~100-150 cartes de feuillage
  <essence>_lod1.glb     même arbre, ~20-30 grandes cartes (mi-distance)
  <essence>_lod2.glb     imposteur : deux plans croisés portant le rendu de l'arbre

Principe (celui des arbres de jeux comme Cities: Skylines 2) : le volume du
houppier est fait de **cartes** (quads détourés par l'alpha de l'atlas) et non
de géométrie pleine. Les cartes sont elles-mêmes composées ici à partir de
photos CC0 (Poly Haven, voir `sources/README.md`) : bout de branche de sapin,
bouquet de feuilles, etc., rendus vus de dessus avec transparence.

Couleur finale dans le jeu = texture de l'atlas × couleur de sommet ; la
couleur de sommet porte l'occlusion ambiante cuite et la variation de teinte.
Les normales du feuillage partent du centre du houppier (ombrage de canopée).

Déterministe (graines fixes). Mètres, pied de l'arbre à l'origine, Z vers le
haut (l'export glTF convertit en Y vers le haut).
"""

import math
import os
import random
import sys

import bmesh
import bpy
import numpy as np
from mathutils import Matrix, Vector

HERE = os.path.dirname(os.path.abspath(__file__))
SOURCES = os.path.join(HERE, "sources")
ARGS = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
OUT_DIR = os.path.abspath(ARGS[0] if ARGS else "trees")
TMP = os.path.join(OUT_DIR, ".tmp")

ATLAS = 2048
CELL = 512

# Cellules de l'atlas (colonne, rangée), rangée 0 en BAS (convention UV Blender).
CARD_CELLS = {"conifere": (0, 3), "chene": (1, 3), "bouleau": (2, 3), "pin": (3, 3)}
BARK_CELLS = {"epicea": (0, 2), "pin": (1, 2), "chene": (2, 2), "bouleau": (3, 2)}
# Imposteurs : une colonne × deux rangées (512 × 1024).
IMPOSTOR_CELLS = {"epicea": (0, 0), "pin": (1, 0), "chene": (2, 0), "bouleau": (3, 0)}


def cell_uv(col, row, rows=1):
    """Rectangle UV (u0, v0, u1, v1) d'une cellule, avec une marge d'un texel et demi."""
    pad = 1.5 / ATLAS
    return (col * CELL / ATLAS + pad, row * CELL / ATLAS + pad,
            (col + 1) * CELL / ATLAS - pad, (row + rows) * CELL / ATLAS - pad)


# ------------------------------------------------------------------ outils

def reset():
    bpy.ops.wm.read_factory_settings(use_empty=True)
    scene = bpy.context.scene
    scene.render.engine = "CYCLES"
    scene.cycles.device = "CPU"
    scene.cycles.samples = 12
    scene.cycles.use_denoising = False
    scene.render.film_transparent = True
    # « Standard » : sans AgX/Filmic, les couleurs rendues sont celles des photos.
    scene.view_settings.view_transform = "Standard"
    scene.render.image_settings.file_format = "PNG"
    scene.render.image_settings.color_mode = "RGBA"
    return scene


def load_pixels(path):
    img = bpy.data.images.load(path)
    w, h = img.size
    px = np.array(img.pixels[:], dtype=np.float32).reshape(h, w, 4)
    bpy.data.images.remove(img)
    return px  # rangées de bas en haut


def components(alpha, min_frac=0.0006, grid=128):
    """Boîtes englobantes (u0, v0, u1, v1, aire) des taches opaques d'un masque."""
    h, w = alpha.shape
    small = alpha[:: max(1, h // grid), :: max(1, w // grid)] > 0.5
    gh, gw = small.shape
    seen = np.zeros_like(small, dtype=bool)
    boxes = []
    for y in range(gh):
        for x in range(gw):
            if not small[y, x] or seen[y, x]:
                continue
            stack = [(y, x)]
            seen[y, x] = True
            ys, xs = [], []
            while stack:
                cy, cx = stack.pop()
                ys.append(cy)
                xs.append(cx)
                for ny, nx in ((cy + 1, cx), (cy - 1, cx), (cy, cx + 1), (cy, cx - 1)):
                    if 0 <= ny < gh and 0 <= nx < gw and small[ny, nx] and not seen[ny, nx]:
                        seen[ny, nx] = True
                        stack.append((ny, nx))
            if len(ys) >= min_frac * gh * gw:
                boxes.append((min(xs) / gw, min(ys) / gh, (max(xs) + 1) / gw, (max(ys) + 1) / gh, len(ys)))
    return boxes


def textured_material(name, color_path, alpha_path, tint=(1, 1, 1)):
    """Émission = couleur de la photo × teinte × Object Color.r (profondeur)."""
    mat = bpy.data.materials.new(name)
    mat.use_nodes = True
    nt = mat.node_tree
    nt.nodes.clear()
    out = nt.nodes.new("ShaderNodeOutputMaterial")
    tex = nt.nodes.new("ShaderNodeTexImage")
    tex.image = bpy.data.images.load(color_path)
    tex.interpolation = "Cubic"
    atex = nt.nodes.new("ShaderNodeTexImage")
    atex.image = bpy.data.images.load(alpha_path)
    atex.image.colorspace_settings.name = "Non-Color"
    info = nt.nodes.new("ShaderNodeObjectInfo")
    mul = nt.nodes.new("ShaderNodeMix")
    mul.data_type = "RGBA"
    mul.blend_type = "MULTIPLY"
    mul.inputs["Factor"].default_value = 1.0
    mul.inputs["B"].default_value = (*tint, 1.0)
    nt.links.new(tex.outputs["Color"], mul.inputs["A"])
    sep = nt.nodes.new("ShaderNodeSeparateColor")
    nt.links.new(info.outputs["Color"], sep.inputs["Color"])
    emit = nt.nodes.new("ShaderNodeEmission")
    nt.links.new(mul.outputs["Result"], emit.inputs["Color"])
    nt.links.new(sep.outputs["Red"], emit.inputs["Strength"])
    transparent = nt.nodes.new("ShaderNodeBsdfTransparent")
    mix = nt.nodes.new("ShaderNodeMixShader")
    # Seuil net : les gris du masque laissaient voir le cadre flou des brins.
    cut = nt.nodes.new("ShaderNodeMath")
    cut.operation = "GREATER_THAN"
    cut.inputs[1].default_value = 0.5
    nt.links.new(atex.outputs["Color"], cut.inputs[0])
    nt.links.new(cut.outputs[0], mix.inputs["Fac"])
    nt.links.new(transparent.outputs[0], mix.inputs[1])
    nt.links.new(emit.outputs[0], mix.inputs[2])
    nt.links.new(mix.outputs[0], out.inputs["Surface"])
    return mat


def quad(name, uv, center, along, across, length, width, mat, shade, z_base=False):
    """Plan texturé : `uv` = rectangle source, axe v le long de `along`."""
    bm = bmesh.new()
    a = Vector(along).normalized()
    b = Vector(across).normalized()
    c = Vector(center)
    start = c if z_base else c - a * (length / 2)
    corners = [start - b * (width / 2), start + b * (width / 2),
               start + b * (width / 2) + a * length, start - b * (width / 2) + a * length]
    verts = [bm.verts.new(p) for p in corners]
    face = bm.faces.new(verts)
    layer = bm.loops.layers.uv.new("UVMap")
    u0, v0, u1, v1 = uv
    for loop, (u, v) in zip(face.loops, ((u0, v0), (u1, v0), (u1, v1), (u0, v1))):
        loop[layer].uv = (u, v)
    mesh = bpy.data.meshes.new(name)
    bm.to_mesh(mesh)
    bm.free()
    obj = bpy.data.objects.new(name, mesh)
    obj.color = (shade, shade, shade, 1)
    mesh.materials.append(mat)
    bpy.context.scene.collection.objects.link(obj)
    return obj


def render_top(path, size=CELL, scale=1.0):
    scene = bpy.context.scene
    cam = bpy.data.objects.new("cam", bpy.data.cameras.new("cam"))
    cam.data.type = "ORTHO"
    cam.data.ortho_scale = scale
    cam.location = (0, 0, 10)
    scene.collection.objects.link(cam)
    scene.camera = cam
    scene.render.resolution_x = scene.render.resolution_y = size
    scene.render.filepath = path
    bpy.ops.render.render(write_still=True)


# ------------------------------------------------------------------ cartes

def make_cards():
    """Rend les quatre cartes de feuillage (512², vues de dessus, alpha)."""
    twig = os.path.join(SOURCES, "fir_tree_01_twig_diff.jpg")
    twig_a = os.path.join(SOURCES, "fir_tree_01_twig_alpha.png")
    leaf = os.path.join(SOURCES, "island_tree_02_leaves_diff.jpg")
    leaf_a = os.path.join(SOURCES, "island_tree_02_leaves_alpha.png")

    # Brins de sapin : taches de taille moyenne (pas la bande blanche à gauche
    # du masque ni la tige du bas, trop allongées ou trop grandes).
    sprigs = [b for b in components(load_pixels(twig_a)[:, :, 0])
              if 0.06 < (b[2] - b[0]) < 0.45 and 0.08 < (b[3] - b[1]) < 0.5 and b[0] > 0.12 and b[1] > 0.12]
    leaves = [b for b in components(load_pixels(leaf_a)[:, :, 0]) if (b[3] - b[1]) > 0.15]
    print(f"cartes : {len(sprigs)} brins de sapin, {len(leaves)} feuilles")
    assert sprigs and leaves, "sources de feuillage introuvables"

    paths = {}
    for kind in CARD_CELLS:
        reset()
        rng = random.Random(sum(map(ord, kind)))
        if kind in ("conifere", "pin"):
            tint = (0.85, 0.95, 0.8) if kind == "conifere" else (1.05, 1.08, 0.82)
            mat = textured_material(kind, twig, twig_a, tint)
            if kind == "conifere":
                # Bout de branche vu de dessus : axe de bas en haut, brins
                # alternés de part et d'autre, plus petits vers la pointe.
                steps = 9
                for i in range(steps):
                    t = i / steps
                    y = -0.45 + t * 0.82
                    size = 0.55 - 0.3 * t
                    for side in (-1, 1):
                        ang = side * math.radians(rng.uniform(38, 58))
                        along = Vector((math.sin(ang), math.cos(ang), 0))
                        s = rng.choice(sprigs)
                        quad("s", s[:4], Vector((0, y, i * 0.01 + 0.005 * side)), along, Vector((along.y, -along.x, 0)),
                             size, size * (s[2] - s[0]) / max(s[3] - s[1], 1e-3) * 1.3, mat, 0.7 + 0.35 * t, z_base=True)
                s = rng.choice(sprigs)
                quad("tip", s[:4], Vector((0, 0.3, 0.2)), (0, 1, 0), (1, 0, 0), 0.25, 0.2, mat, 1.05, z_base=True)
            else:
                # Touffe de pin : brins en étoile depuis le centre.
                for k in range(11):
                    ang = k / 11 * math.tau + rng.uniform(-0.2, 0.2)
                    along = Vector((math.cos(ang), math.sin(ang), 0))
                    s = rng.choice(sprigs)
                    quad("t", s[:4], Vector((0, 0, k * 0.01)), along, Vector((along.y, -along.x, 0)),
                         rng.uniform(0.36, 0.48), 0.3, mat, rng.uniform(0.75, 1.1), z_base=True)
        else:
            tint = (0.82, 0.95, 0.72) if kind == "chene" else (1.08, 1.15, 0.85)
            mat = textured_material(kind, leaf, leaf_a, tint)
            count, size = (40, 0.24) if kind == "chene" else (60, 0.15)
            # Bouquet : feuilles réparties dans un disque, pointées vers
            # l'extérieur, empilées (les plus hautes plus claires).
            for _ in range(count):
                r = 0.42 * math.sqrt(rng.random())
                ang = rng.uniform(0, math.tau)
                pos = Vector((math.cos(ang) * r, math.sin(ang) * r * 0.95, rng.random()))
                out = Vector((math.cos(ang), math.sin(ang), 0)) if r > 0.05 else Vector((0, 1, 0))
                out.rotate(Matrix.Rotation(rng.uniform(-0.7, 0.7), 3, "Z"))
                s = rng.choice(leaves)
                length = size * rng.uniform(0.8, 1.2)
                quad("l", s[:4], pos, out, Vector((out.y, -out.x, 0)), length,
                     length * (s[2] - s[0]) / max(s[3] - s[1], 1e-3), mat, 0.6 + 0.5 * pos.z)
        path = os.path.join(TMP, f"card_{kind}.png")
        render_top(path)
        paths[kind] = path
    return paths


def make_barks():
    """Écorces 512² (rangées de bas en haut, RGBA)."""
    def resized(path):
        img = bpy.data.images.load(path)
        img.scale(CELL, CELL)
        px = np.array(img.pixels[:], dtype=np.float32).reshape(CELL, CELL, 4)
        bpy.data.images.remove(img)
        px[:, :, 3] = 1.0
        return px

    fir = resized(os.path.join(SOURCES, "fir_tree_01_bark_diff.jpg"))
    oak = resized(os.path.join(SOURCES, "jacaranda_tree_trunk_diff.jpg"))
    pine = fir.copy()
    # Écorce de pin sylvestre : plaques orangées.
    pine[:, :, 0] = np.clip(pine[:, :, 0] * 1.55 + 0.08, 0, 1)
    pine[:, :, 1] = np.clip(pine[:, :, 1] * 1.05 + 0.02, 0, 1)
    pine[:, :, 2] = np.clip(pine[:, :, 2] * 0.75, 0, 1)
    # Bouleau : blanc crème, lenticelles sombres horizontales.
    rng = np.random.default_rng(7)
    birch = np.ones((CELL, CELL, 4), dtype=np.float32)
    birch[:, :, :3] = np.array([0.86, 0.84, 0.79], dtype=np.float32)
    shade = 0.92 + 0.08 * rng.random((CELL, 1))
    birch[:, :, :3] *= shade[:, :, None]
    for _ in range(260):
        y = rng.integers(0, CELL)
        x = rng.integers(0, CELL)
        length = rng.integers(10, 70)
        thick = rng.integers(1, 4)
        dark = rng.uniform(0.08, 0.3)
        xs = np.arange(x, x + length) % CELL
        birch[y:y + thick, xs, :3] = dark
    return {"epicea": fir, "pin": pine, "chene": oak, "bouleau": birch}


# ------------------------------------------------------------------ arbres

class Tree:
    """Géométrie d'arbre : bmesh avec UV (coins) et couleur (sommets)."""

    def __init__(self):
        self.bm = bmesh.new()
        self.uv = self.bm.loops.layers.uv.new("UVMap")
        self.color = self.bm.verts.layers.float_color.new("Col")
        self.normals = {}

    def face(self, points, uvs, normals, colors):
        verts = []
        for p, n, c in zip(points, normals, colors):
            v = self.bm.verts.new(p)
            v[self.color] = (*c, 1.0)
            self.normals[v] = Vector(n).normalized()
            verts.append(v)
        f = self.bm.faces.new(verts)
        for loop, uv in zip(f.loops, uvs):
            loop[self.uv].uv = uv
        return f

    def card(self, center, along, across, length, width, uv, normal, shade, base=False):
        a, b = Vector(along).normalized(), Vector(across).normalized()
        start = Vector(center) if base else Vector(center) - a * (length / 2)
        p = [start - b * width / 2, start + b * width / 2,
             start + b * width / 2 + a * length, start - b * width / 2 + a * length]
        u0, v0, u1, v1 = uv
        c = (shade, shade, shade)
        self.face(p, [(u0, v0), (u1, v0), (u1, v1), (u0, v1)], [normal] * 4, [c] * 4)

    def cylinder(self, start, end, r0, r1, sides, uv, shade=1.0):
        axis = end - start
        length = axis.length
        axis.normalize()
        helper = Vector((0, 0, 1)) if abs(axis.z) < 0.9 else Vector((1, 0, 0))
        u = axis.cross(helper).normalized()
        w = axis.cross(u).normalized()
        u0, v0, u1, v1 = uv
        for s in range(sides):
            a0 = s / sides * math.tau
            a1 = (s + 1) / sides * math.tau
            n0 = u * math.cos(a0) + w * math.sin(a0)
            n1 = u * math.cos(a1) + w * math.sin(a1)
            pts = [start + n0 * r0, start + n1 * r0, end + n1 * r1, end + n0 * r1]
            su0 = u0 + (u1 - u0) * s / sides
            su1 = u0 + (u1 - u0) * (s + 1) / sides
            vv1 = v0 + (v1 - v0) * min(1.0, length / 8.0)
            self.face(pts, [(su0, v0), (su1, v0), (su1, vv1), (su0, vv1)], [n0, n1, n1, n0],
                      [(shade * 0.8,) * 3, (shade * 0.8,) * 3, (shade,) * 3, (shade,) * 3])

    def to_object(self, name):
        mesh = bpy.data.meshes.new(name)
        self.bm.to_mesh(mesh)
        mesh.update()
        by_index = {v.index: n for v, n in self.normals.items()}
        mesh.shade_smooth()
        mesh.normals_split_custom_set_from_vertices([tuple(by_index.get(v.index, v.normal)) for v in mesh.vertices])
        obj = bpy.data.objects.new(name, mesh)
        bpy.context.scene.collection.objects.link(obj)
        return obj


def canopy_normal(p, crown):
    out = (Vector(p) - Vector(crown))
    out.z *= 0.6
    if out.length < 1e-4:
        out = Vector((0, 0, 1))
    return (out.normalized() * 0.65 + Vector((0, 0, 1)) * 0.35).normalized()


def ao(p, crown, radius, height_min, height_max):
    """Occlusion cuite : sombre à l'intérieur et en bas du houppier."""
    inner = min(1.0, (Vector(p) - Vector(crown)).length / max(radius, 1e-3))
    h = (p[2] - height_min) / max(height_max - height_min, 1e-3)
    return 0.5 + 0.3 * inner + 0.25 * max(0.0, min(1.0, h))


def random_across(along, rng):
    a = Vector(along).normalized()
    helper = Vector((0, 0, 1)) if abs(a.z) < 0.8 else Vector((1, 0, 0))
    b = a.cross(helper).normalized()
    b.rotate(Matrix.Rotation(rng.uniform(-0.6, 0.6), 3, a))
    return b


def build_spruce(t, rng, detail):
    h = 21.0
    t.cylinder(Vector((0, 0, -0.5)), Vector((0, 0, h * 0.95)), 0.4, 0.05, 6 if detail else 4, cell_uv(*BARK_CELLS["epicea"]))
    card = cell_uv(*CARD_CELLS["conifere"])
    whorls = 18 if detail else 7
    per = 6 if detail else 5
    for i in range(whorls):
        f = i / (whorls - 1)
        z = 1.5 + f * (h - 2.5)
        reach = (4.0 * (1 - f) ** 0.9 + 0.5) * (1.0 if detail else 1.15)
        phase = rng.uniform(0, math.tau)
        for k in range(per):
            ang = phase + k / per * math.tau + rng.uniform(-0.25, 0.25)
            droop = -0.35 - 0.25 * (1 - f)
            along = Vector((math.cos(ang), math.sin(ang), droop))
            across = Vector((-math.sin(ang), math.cos(ang), 0))
            base = Vector((0, 0, z))
            mid = base + along.normalized() * reach * 0.5
            shade = ao(mid, (0, 0, h * 0.45), 4.0, 0, h)
            t.card(base, along, across, reach * 1.1, reach * 0.85, card, canopy_normal(mid, (0, 0, h * 0.45)), shade, base=True)
    # Flèche.
    t.card(Vector((0, 0, h - 1.5)), (0, 0, 1), (1, 0, 0), 2.2, 1.0, card, (0, 0, 1), 1.0, base=True)
    t.card(Vector((0, 0, h - 1.5)), (0, 0, 1), (0, 1, 0), 2.2, 1.0, card, (0, 0, 1), 1.0, base=True)


def build_pine(t, rng, detail):
    h = 17.0
    top = Vector((0.4, 0.2, h * 0.84))
    t.cylinder(Vector((0, 0, -0.5)), top, 0.38, 0.12, 6 if detail else 4, cell_uv(*BARK_CELLS["pin"]))
    card = cell_uv(*CARD_CELLS["pin"])
    crown = Vector((0.4, 0.2, h * 0.8))
    branches = 7 if detail else 5
    for k in range(branches):
        ang = k / branches * math.tau + rng.uniform(-0.4, 0.4)
        start = Vector((0.3, 0.15, h * (0.58 + 0.035 * k)))
        tip = start + Vector((math.cos(ang) * 2.8, math.sin(ang) * 2.8, rng.uniform(0.6, 2.0)))
        if detail:
            t.cylinder(start, tip, 0.12, 0.05, 4, cell_uv(*BARK_CELLS["pin"]))
        tufts = 6 if detail else 2
        for _ in range(tufts):
            c = tip + Vector((rng.uniform(-1.2, 1.2), rng.uniform(-1.2, 1.2), rng.uniform(-0.3, 0.8)))
            size = rng.uniform(2.0, 2.8) * (1.0 if detail else 1.8)
            normal = canopy_normal(c, crown)
            along = random_across(normal, rng)
            t.card(c, along, normal.cross(along), size, size, card, normal, ao(c, crown, 3.5, h * 0.5, h))
    for _ in range(8 if detail else 2):
        c = top + Vector((rng.uniform(-1.4, 1.4), rng.uniform(-1.4, 1.4), rng.uniform(0, 1.0)))
        size = 2.6 * (1.0 if detail else 1.6)
        normal = canopy_normal(c, crown)
        along = random_across(normal, rng)
        t.card(c, along, normal.cross(along), size, size, card, normal, ao(c, crown, 3.5, h * 0.5, h))


def broadleaf(t, rng, detail, trunk_top, crown, clusters, cluster_radius, card_kind, card_size, count, h):
    card = cell_uv(*CARD_CELLS[card_kind])
    bark = cell_uv(*BARK_CELLS["chene" if card_kind == "chene" else "bouleau"])
    if detail:
        for c in clusters:
            t.cylinder(trunk_top, c, 0.28 if card_kind == "chene" else 0.1, 0.08, 4, bark)
    per_cluster = max(1, count // len(clusters))
    for c in clusters:
        for _ in range(per_cluster):
            d = Vector((rng.gauss(0, 1), rng.gauss(0, 1), rng.gauss(0, 0.8))).normalized()
            p = c + d * cluster_radius * rng.uniform(0.45, 1.0)
            size = card_size * rng.uniform(0.8, 1.2) * (1.0 if detail else 2.2)
            normal = canopy_normal(p, crown)
            along = random_across(normal, rng)
            t.card(p, along, normal.cross(along), size, size * 0.95, card, normal, ao(p, crown, cluster_radius * 2.2, crown.z - 5, h))


def build_oak(t, rng, detail):
    h = 14.0
    trunk_top = Vector((0, 0, 4.3))
    t.cylinder(Vector((0, 0, -0.5)), trunk_top, 0.65, 0.45, 8 if detail else 4, cell_uv(*BARK_CELLS["chene"]))
    crown = Vector((0, 0, 9.0))
    clusters = []
    # Houppier ample et rond : deux couronnes de grappes + un sommet.
    for k in range(6):
        a = k / 6 * math.tau + rng.uniform(-0.3, 0.3)
        clusters.append(Vector((math.cos(a) * 3.8, math.sin(a) * 3.8, rng.uniform(6.8, 8.6))))
    for k in range(4):
        a = (k + 0.5) / 4 * math.tau + rng.uniform(-0.3, 0.3)
        clusters.append(Vector((math.cos(a) * 2.2, math.sin(a) * 2.2, rng.uniform(10.0, 11.5))))
    clusters.append(Vector((0, 0, 12.3)))
    broadleaf(t, rng, detail, trunk_top, crown, clusters, 2.9, "chene", 2.8, 165 if detail else 30, h)


def build_birch(t, rng, detail):
    h = 16.0
    top = Vector((0.2, -0.1, h * 0.9))
    t.cylinder(Vector((0, 0, -0.5)), top, 0.25, 0.06, 6 if detail else 4, cell_uv(*BARK_CELLS["bouleau"]))
    crown = Vector((0.2, -0.1, 10.5))
    clusters = []
    for k in range(5):
        a = k / 5 * math.tau + rng.uniform(-0.5, 0.5)
        clusters.append(crown + Vector((math.cos(a) * 1.4, math.sin(a) * 1.4, rng.uniform(-2.8, 3.2))))
    broadleaf(t, rng, detail, Vector((0.15, -0.05, 6.0)), crown, clusters, 2.0, "bouleau", 1.7, 120 if detail else 20, h)


SPECIES = {"epicea": build_spruce, "pin": build_pine, "chene": build_oak, "bouleau": build_birch}


# ------------------------------------------------------------------ export, imposteurs

def triangulated(obj):
    bm = bmesh.new()
    bm.from_mesh(obj.data)
    bmesh.ops.triangulate(bm, faces=bm.faces[:])
    bm.to_mesh(obj.data)
    bm.free()
    return sum(len(p.vertices) - 2 for p in obj.data.polygons)


def export(obj, path):
    bpy.ops.object.select_all(action="DESELECT")
    obj.select_set(True)
    bpy.context.view_layer.objects.active = obj
    bpy.ops.export_scene.gltf(
        filepath=path, export_format="GLB", use_selection=True, export_apply=True, export_yup=True,
        export_normals=True, export_vertex_color="ACTIVE", export_materials="NONE", export_texcoords=True,
    )


def atlas_material(atlas_path):
    """Matériau de rendu des imposteurs : albédo × couleur de sommet, émissif."""
    mat = bpy.data.materials.new("atlas")
    mat.use_nodes = True
    nt = mat.node_tree
    nt.nodes.clear()
    out = nt.nodes.new("ShaderNodeOutputMaterial")
    tex = nt.nodes.new("ShaderNodeTexImage")
    tex.image = bpy.data.images.load(atlas_path)
    tex.interpolation = "Closest"
    col = nt.nodes.new("ShaderNodeVertexColor")
    col.layer_name = "Col"
    mul = nt.nodes.new("ShaderNodeMix")
    mul.data_type = "RGBA"
    mul.blend_type = "MULTIPLY"
    mul.inputs["Factor"].default_value = 1.0
    nt.links.new(tex.outputs["Color"], mul.inputs["A"])
    nt.links.new(col.outputs["Color"], mul.inputs["B"])
    emit = nt.nodes.new("ShaderNodeEmission")
    nt.links.new(mul.outputs["Result"], emit.inputs["Color"])
    clip = nt.nodes.new("ShaderNodeMath")
    clip.operation = "GREATER_THAN"
    clip.inputs[1].default_value = 0.45
    nt.links.new(tex.outputs["Alpha"], clip.inputs[0])
    transparent = nt.nodes.new("ShaderNodeBsdfTransparent")
    mix = nt.nodes.new("ShaderNodeMixShader")
    nt.links.new(clip.outputs[0], mix.inputs["Fac"])
    nt.links.new(transparent.outputs[0], mix.inputs[1])
    nt.links.new(emit.outputs[0], mix.inputs[2])
    nt.links.new(mix.outputs[0], out.inputs["Surface"])
    return mat


def render_impostor(obj, path):
    """Rendu de profil (512 × 1024), pied en bas. Renvoie (largeur, hauteur) en m."""
    scene = bpy.context.scene
    zs = [v.co.z for v in obj.data.vertices]
    rs = [math.hypot(v.co.x, v.co.y) for v in obj.data.vertices]
    height = max(zs) + 0.3
    scale = max(height, 4 * max(rs) + 0.5)  # image 1:2, l'échelle porte sur la hauteur
    cam = bpy.data.objects.new("icam", bpy.data.cameras.new("icam"))
    cam.data.type = "ORTHO"
    cam.data.ortho_scale = scale
    cam.location = (0, -60, scale / 2)
    cam.rotation_euler = (math.radians(90), 0, 0)
    scene.collection.objects.link(cam)
    scene.camera = cam
    scene.render.resolution_x, scene.render.resolution_y = CELL, CELL * 2
    scene.render.filepath = path
    bpy.ops.render.render(write_still=True)
    return scale / 2, scale


def impostor_object(name, width, height, uv):
    t = Tree()
    u0, v0, u1, v1 = uv
    for along in ((1, 0, 0), (0, 1, 0)):
        a = Vector(along)
        p = [-a * width / 2, a * width / 2, a * width / 2 + Vector((0, 0, height)), -a * width / 2 + Vector((0, 0, height))]
        n_low = (a.cross(Vector((0, 0, 1))) * 0.4 + Vector((0, 0, 1)) * 0.6).normalized()
        n_high = Vector((0, 0, 1))
        t.face(p, [(u0, v0), (u1, v0), (u1, v1), (u0, v1)], [n_low, n_low, n_high, n_high], [(1, 1, 1)] * 4)
    return t.to_object(name)


def write_atlas(cells, path):
    atlas = np.zeros((ATLAS, ATLAS, 4), dtype=np.float32)
    for (col, row, rows), px in cells:
        atlas[row * CELL:(row + rows) * CELL, col * CELL:(col + 1) * CELL] = px
    img = bpy.data.images.new("atlas", ATLAS, ATLAS, alpha=True)
    img.pixels = atlas.ravel()
    img.filepath_raw = path
    img.file_format = "PNG"
    img.save()
    bpy.data.images.remove(img)


def main():
    os.makedirs(TMP, exist_ok=True)
    cards = make_cards()
    reset()
    barks = make_barks()
    cells = []
    for kind, (col, row) in CARD_CELLS.items():
        cells.append(((col, row, 1), load_pixels(cards[kind])))
    for kind, (col, row) in BARK_CELLS.items():
        cells.append(((col, row, 1), barks[kind]))
    atlas_path = os.path.join(OUT_DIR, "atlas.png")
    write_atlas(cells, atlas_path)

    for index, (name, build) in enumerate(SPECIES.items()):
        for lod, detail in ((0, True), (1, False)):
            reset()
            t = Tree()
            build(t, random.Random(100 + index), detail)
            obj = t.to_object(f"{name}_lod{lod}")
            tris = triangulated(obj)
            export(obj, os.path.join(OUT_DIR, f"{name}_lod{lod}.glb"))
            print(f"{name} lod{lod} : {tris} triangles, {len(obj.data.vertices)} sommets")
            if lod == 0:
                obj.data.materials.append(atlas_material(atlas_path))
                path = os.path.join(TMP, f"impostor_{name}.png")
                width, height = render_impostor(obj, path)
                col, row = IMPOSTOR_CELLS[name]
                cells.append(((col, row, 2), load_pixels(path)))
                imp = impostor_object(f"{name}_lod2", width, height, cell_uv(col, row, rows=2))
                triangulated(imp)
                export(imp, os.path.join(OUT_DIR, f"{name}_lod2.glb"))
    write_atlas(cells, atlas_path)
    # Recompression sans perte (−40 %) si ImageMagick est disponible.
    import shutil
    import subprocess
    if shutil.which("magick"):
        subprocess.run(["magick", atlas_path, "-define", "png:compression-level=9",
                        "-define", "png:compression-filter=5", atlas_path], check=False)
    shutil.rmtree(TMP, ignore_errors=True)
    print("atlas :", atlas_path)


main()
