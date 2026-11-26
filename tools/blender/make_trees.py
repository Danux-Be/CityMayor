"""Génère les arbres de CityMayor dans Blender (sans interface).

    blender -b --factory-startup --python tools/blender/make_trees.py -- crates/render/assets/trees

Quatre essences, chacune en deux niveaux de détail :
  <essence>_lod0.glb   ~300-600 triangles (vu de près, porte des ombres)
  <essence>_lod1.glb   ~40-60 triangles   (au-delà de quelques centaines de mètres)

Le jeu fusionne des milliers d'instances par morceau de carte : pas de
texture, pas de matériau à part, tout est dans les **couleurs de sommets**
(albédo + occlusion ambiante cuite) et dans les **normales du feuillage**,
orientées depuis le centre du houppier pour un ombrage doux de canopée.

Déterministe : graine fixe par essence, aucune dépendance à l'état de Blender.
Unités : mètres, pied de l'arbre à l'origine, Z vers le haut (l'export glTF
convertit en Y vers le haut).
"""

import math
import os
import random
import sys

import bmesh
import bpy
from mathutils import Vector, noise

ARGS = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
OUT_DIR = os.path.abspath(ARGS[0] if ARGS else "trees")
LOD1_TRIANGLES = 48


def reset_scene():
    bpy.ops.wm.read_factory_settings(use_empty=True)


# ------------------------------------------------------------------ géométrie

class Builder:
    """Accumule des sommets colorés dans un bmesh, avec normales personnalisées."""

    def __init__(self):
        self.bm = bmesh.new()
        # Couleur **par sommet** (et non par coin de face) : sinon l'export
        # glTF duplique chaque sommet par face (×3 sommets, mesuré).
        self.color = self.bm.verts.layers.float_color.new("Col")
        # Normale voulue par sommet (None : normale géométrique).
        self.custom_normals = {}

    def vert(self, co, normal=None):
        v = self.bm.verts.new(co)
        if normal is not None:
            self.custom_normals[v] = Vector(normal).normalized()
        return v

    def face(self, verts, colors):
        try:
            f = self.bm.faces.new(verts)
        except ValueError:
            return None
        for vert, color in zip(verts, colors):
            vert[self.color] = (*color, 1.0)
        return f


def lerp3(a, b, t):
    return tuple(a[i] + (b[i] - a[i]) * t for i in range(3))


def shade(color, factor):
    return tuple(max(0.0, min(1.0, c * factor)) for c in color)


def tapered_cylinder(b, start, end, r0, r1, sides, color_bottom, color_top, rng, bark_marks=None):
    """Tronc ou branche : cylindre effilé, faces latérales seulement."""
    axis = (end - start)
    length = axis.length
    axis.normalize()
    helper = Vector((0, 0, 1)) if abs(axis.z) < 0.9 else Vector((1, 0, 0))
    u = axis.cross(helper).normalized()
    v = axis.cross(u).normalized()
    rings = max(2, int(length / 2.5) + 2)
    ring_verts = []
    for k in range(rings):
        t = k / (rings - 1)
        center = start + (end - start) * t
        radius = r0 + (r1 - r0) * t
        ring = []
        for s in range(sides):
            a = s / sides * math.tau + rng.uniform(-0.1, 0.1)
            n = u * math.cos(a) + v * math.sin(a)
            wobble = 1.0 + 0.12 * noise.noise(center * 0.9 + n)
            ring.append((b.vert(center + n * radius * wobble), t, s))
        ring_verts.append(ring)
    for k in range(rings - 1):
        for s in range(sides):
            a0, ta, sa = ring_verts[k][s]
            a1, _, _ = ring_verts[k][(s + 1) % sides]
            b1, tb, _ = ring_verts[k + 1][(s + 1) % sides]
            b0, _, _ = ring_verts[k + 1][s]
            cols = []
            for t in (ta, ta, tb, tb):
                c = lerp3(color_bottom, color_top, t)
                if bark_marks and rng.random() < bark_marks:
                    c = shade(c, 0.35)
                cols.append(c)
            b.face([a0, a1, b1, b0], cols)


def foliage_blob(b, center, radius, squash, color, rng, crown_center, subdivisions=2, roughness=0.30):
    """Grappe de feuillage : icosphère bosselée, normales depuis le centre du houppier."""
    tmp = bmesh.new()
    bmesh.ops.create_icosphere(tmp, subdivisions=subdivisions, radius=1.0)
    seed = Vector((rng.uniform(0, 100), rng.uniform(0, 100), rng.uniform(0, 100)))
    mapping = {}
    for tv in tmp.verts:
        d = tv.co.normalized()
        # Trois octaves : grosses touffes, rameaux, feuillage.
        bump = (1.0 + roughness * noise.noise(d * 1.9 + seed)
                + roughness * 0.55 * noise.noise(d * 4.3 + seed)
                + roughness * 0.3 * noise.noise(d * 9.7 + seed))
        # Bornes : un pic de bruit ferait une aiguille isolée hors du houppier.
        bump = max(0.7, min(1.35, bump))
        p = Vector((d.x * radius, d.y * radius, d.z * radius * squash)) * bump + center
        # Normale « canopée » : mélange de la normale locale et de la direction
        # depuis le centre du houppier (lumière douce, pas de facettes).
        outward = (p - crown_center).normalized()
        n = (outward * 0.75 + d * 0.25).normalized()
        mapping[tv] = (b.vert(p, n), p)
    for tf in tmp.faces:
        verts = [mapping[v][0] for v in tf.verts]
        cols = []
        for v in tf.verts:
            p = mapping[v][1]
            # Occlusion cuite : plus sombre vers le bas et vers l'intérieur.
            height = (p.z - (crown_center.z - radius * 1.6)) / (radius * 3.2)
            inner = (p - crown_center).length / max(radius * 1.8, 1e-3)
            ao = 0.42 + 0.45 * max(0.0, min(1.0, height)) + 0.25 * max(0.0, min(1.0, inner))
            # Taches de lumière : des rameaux plus clairs ou plus sombres.
            patch = 1.0 + 0.22 * noise.noise(p * 0.8 + seed)
            jitter = 1.0 + rng.uniform(-0.05, 0.05)
            cols.append(shade(color, ao * patch * jitter))
        b.face(verts, cols)
    tmp.free()


def conifer_tier(b, center, radius, height, points, color, rng, droop=0.35):
    """Étage de ramure : jupe en étoile déchiquetée, pointes tombantes."""
    apex = b.vert(center + Vector((0, 0, height)), (0, 0, 1))
    rim = []
    phase = rng.uniform(0, math.tau)
    for k in range(points * 2):
        a = phase + k / (points * 2) * math.tau
        tip = k % 2 == 0
        r = radius * (1.0 if tip else 0.62) * rng.uniform(0.85, 1.12)
        z = -droop * radius * (1.0 if tip else 0.45) + rng.uniform(-0.15, 0.15)
        p = center + Vector((math.cos(a) * r, math.sin(a) * r, z))
        n = Vector((math.cos(a), math.sin(a), 0.9)).normalized()
        rim.append((b.vert(p, n), tip))
    dark = shade(color, 0.55)
    light = shade(color, 1.18)
    for k in range(len(rim)):
        v0, tip0 = rim[k]
        v1, tip1 = rim[(k + 1) % len(rim)]
        b.face([apex, v0, v1], [light, light if tip0 else dark, light if tip1 else dark])
    # Dessous sombre (vu en contre-plongée et dans les ombres).
    under = b.vert(center + Vector((0, 0, -droop * radius * 0.2)), (0, 0, -1))
    for k in range(len(rim)):
        v0, _ = rim[k]
        v1, _ = rim[(k + 1) % len(rim)]
        b.face([under, v1, v0], [shade(color, 0.3)] * 3)


# ------------------------------------------------------------------ essences

def make_spruce(b, rng):
    """Épicéa : flèche étroite, étages serrés jusqu'au sol."""
    h = 21.0
    tapered_cylinder(b, Vector((0, 0, -0.5)), Vector((0, 0, h * 0.92)), 0.38, 0.06, 6,
                     (0.20, 0.13, 0.08), (0.26, 0.18, 0.11), rng)
    color = (0.06, 0.15, 0.08)
    tiers = 8
    for i in range(tiers):
        t = i / (tiers - 1)
        z = 1.6 + t * (h - 4.5)
        radius = 3.6 * (1.0 - t) ** 0.85 + 0.55
        tier_color = lerp3(color, (0.09, 0.21, 0.11), t)
        conifer_tier(b, Vector((0, 0, z)), radius, 3.2 + 0.4 * (1 - t), 8, tier_color, rng, droop=0.42)
    conifer_tier(b, Vector((0, 0, h - 2.6)), 0.7, 2.6, 5, (0.10, 0.23, 0.12), rng, droop=0.2)


def make_pine(b, rng):
    """Pin sylvestre : long fût nu orangé, couronne irrégulière en touffes."""
    h = 17.0
    top = Vector((0.4, 0.2, h * 0.86))
    tapered_cylinder(b, Vector((0, 0, -0.5)), top, 0.36, 0.12, 6,
                     (0.30, 0.17, 0.10), (0.62, 0.36, 0.20), rng, bark_marks=0.08)
    crown_center = Vector((0.4, 0.2, h * 0.78))
    color = (0.08, 0.17, 0.07)
    for k in range(5):
        a = k / 5 * math.tau + rng.uniform(-0.4, 0.4)
        start = Vector((0.3, 0.15, h * (0.58 + 0.06 * k)))
        tip = start + Vector((math.cos(a) * 2.4, math.sin(a) * 2.4, rng.uniform(0.8, 2.0)))
        tapered_cylinder(b, start, tip, 0.12, 0.05, 4, (0.45, 0.27, 0.15), (0.50, 0.30, 0.16), rng)
        foliage_blob(b, tip + Vector((0, 0, 0.4)), rng.uniform(1.5, 1.9), 0.55, color, rng, crown_center, roughness=0.38)
    foliage_blob(b, top + Vector((0, 0, 0.3)), 1.9, 0.65, (0.09, 0.19, 0.08), rng, crown_center, roughness=0.38)


def make_oak(b, rng):
    """Chêne : tronc trapu, branches maîtresses, houppier large en grappes."""
    h = 13.0
    base_top = Vector((0, 0, 4.2))
    tapered_cylinder(b, Vector((0, 0, -0.5)), base_top, 0.62, 0.45, 7,
                     (0.18, 0.13, 0.09), (0.26, 0.19, 0.13), rng, bark_marks=0.05)
    crown_center = Vector((0, 0, 8.6))
    clusters = []
    for k in range(6):
        a = k / 6 * math.tau + rng.uniform(-0.3, 0.3)
        tip = Vector((math.cos(a) * 3.7, math.sin(a) * 3.7, rng.uniform(7.0, 9.8)))
        tapered_cylinder(b, base_top, tip, 0.32, 0.12, 5, (0.22, 0.16, 0.11), (0.25, 0.18, 0.12), rng)
        clusters.append(tip)
    clusters.append(Vector((0, 0, 11.0)))
    palette = [(0.13, 0.25, 0.07), (0.16, 0.29, 0.08), (0.11, 0.22, 0.06)]
    for k, c in enumerate(clusters):
        radius = rng.uniform(2.5, 3.0) if k < 6 else 3.4
        foliage_blob(b, c, radius, 0.78, palette[k % 3], rng, crown_center)


def make_birch(b, rng):
    """Bouleau : tronc fin blanc marqué de noir, houppier ovale clair."""
    h = 15.0
    tapered_cylinder(b, Vector((0, 0, -0.5)), Vector((0.2, -0.1, h * 0.86)), 0.24, 0.07, 6,
                     (0.80, 0.78, 0.72), (0.86, 0.84, 0.80), rng, bark_marks=0.22)
    crown_center = Vector((0.2, -0.1, 10.0))
    for k in range(4):
        a = k / 4 * math.tau + rng.uniform(-0.5, 0.5)
        c = crown_center + Vector((math.cos(a) * 1.3, math.sin(a) * 1.3, rng.uniform(-2.2, 2.6)))
        foliage_blob(b, c, 1.9, 1.35, (0.24, 0.38, 0.11), rng, crown_center, roughness=0.3)
    foliage_blob(b, crown_center + Vector((0, 0, 2.6)), 1.6, 1.4, (0.27, 0.41, 0.12), rng, crown_center)


SPECIES = {
    "epicea": make_spruce,
    "pin": make_pine,
    "chene": make_oak,
    "bouleau": make_birch,
}


# ------------------------------------------------------------------ export

def build_object(name, builder):
    mesh = bpy.data.meshes.new(name)
    builder.bm.to_mesh(mesh)
    # Normales personnalisées : canopée douce, troncs lissés.
    normals = []
    by_index = {v.index: n for v, n in builder.custom_normals.items()}
    mesh.update()
    for v in mesh.vertices:
        n = by_index.get(v.index)
        normals.append(tuple(n) if n is not None else tuple(v.normal))
    # Ombrage lisse : avec l'ombrage plat, chaque face impose sa normale et
    # l'export duplique les sommets ; nos normales par sommet suffisent.
    mesh.shade_smooth()
    mesh.normals_split_custom_set_from_vertices(normals)
    obj = bpy.data.objects.new(name, mesh)
    bpy.context.scene.collection.objects.link(obj)
    # Matériau minimal qui lit la couleur de sommet (l'exporteur la garde).
    mat = bpy.data.materials.new(name + "_mat")
    mat.use_nodes = True
    nodes = mat.node_tree.nodes
    bsdf = next(n for n in nodes if n.type == "BSDF_PRINCIPLED")
    attr = nodes.new("ShaderNodeVertexColor")
    attr.layer_name = "Col"
    mat.node_tree.links.new(attr.outputs[0], bsdf.inputs[0])
    mesh.materials.append(mat)
    return obj


def triangle_count(obj):
    return sum(len(p.vertices) - 2 for p in obj.data.polygons)


def export(obj, path):
    bpy.ops.object.select_all(action="DESELECT")
    obj.select_set(True)
    bpy.context.view_layer.objects.active = obj
    bpy.ops.export_scene.gltf(
        filepath=path,
        export_format="GLB",
        use_selection=True,
        export_apply=True,
        export_yup=True,
        export_normals=True,
        export_vertex_color="ACTIVE",
        export_materials="NONE",
        export_texcoords=False,
    )


def main():
    os.makedirs(OUT_DIR, exist_ok=True)
    for index, (name, make) in enumerate(SPECIES.items()):
        reset_scene()
        rng = random.Random(1_000 + index)
        builder = Builder()
        make(builder, rng)
        bmesh.ops.triangulate(builder.bm, faces=builder.bm.faces[:])
        lod0 = build_object(f"{name}_lod0", builder)
        export(lod0, os.path.join(OUT_DIR, f"{name}_lod0.glb"))
        t0 = triangle_count(lod0)

        # LOD1 : décimation du même arbre (même silhouette, ~48 triangles).
        lod1 = lod0.copy()
        lod1.data = lod0.data.copy()
        lod1.name = f"{name}_lod1"
        bpy.context.scene.collection.objects.link(lod1)
        dec = lod1.modifiers.new("decimate", "DECIMATE")
        dec.decimate_type = "COLLAPSE"
        dec.ratio = min(1.0, LOD1_TRIANGLES / max(t0, 1))
        dec.use_collapse_triangulate = True
        bpy.context.view_layer.objects.active = lod1
        bpy.ops.object.modifier_apply(modifier=dec.name)
        export(lod1, os.path.join(OUT_DIR, f"{name}_lod1.glb"))
        print(f"{name}: lod0 {t0} triangles, lod1 {triangle_count(lod1)} triangles")
        builder.bm.free()


main()
