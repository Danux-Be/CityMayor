"""Engins et installations de chantier de CityMayor (Blender, sans interface).

    blender -b --factory-startup --python tools/blender/make_site_props.py -- crates/render/assets/site

Produit (glTF binaire, couleur par sommet, sans texture) :
  grue_mat.glb      un élément de mât de grue à tour (treillis 1,6 m × 3 m),
                    empilé par le jeu jusqu'à la hauteur voulue
  grue_tete.glb     tête de grue : pivot, cabine, porte-flèche, flèche en
                    treillis de 30 m, contre-flèche lestée, tirants, chariot,
                    câble et crochet (origine = sommet du mât)
  pelleteuse.glb    pelle hydraulique sur chenilles (~ 9 m bras déployé)
  bungalow.glb      bungalow de chantier (conteneur 6 × 2,4 m, fenêtres, porte)

Le jeu multiplie la couleur de sommet par une couche de peinture unie
(`urban::Layer::Marking`) : la couleur porte donc la teinte réelle (jaune
de grue, verre sombre, béton des contrepoids).

Mètres, Z vers le haut (l'export glTF convertit en Y vers le haut), origine
au pied de l'objet. Déterministe.
"""

import math
import os
import sys

import bmesh
import bpy
from mathutils import Vector

ARGS = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
OUT_DIR = os.path.abspath(ARGS[0] if ARGS else "site")

YELLOW = (0.95, 0.68, 0.05)
DARK_YELLOW = (0.75, 0.5, 0.04)
STEEL = (0.35, 0.36, 0.38)
BLACK = (0.06, 0.06, 0.07)
GLASS = (0.12, 0.17, 0.22)
CONCRETE = (0.62, 0.61, 0.58)
WHITE = (0.9, 0.9, 0.88)
BLUE = (0.16, 0.32, 0.6)


class Prop:
    """Maillage à couleur par sommet et normales plates."""

    def __init__(self):
        self.bm = bmesh.new()
        self.color = self.bm.verts.layers.float_color.new("Col")

    def quad(self, pts, color):
        verts = []
        for p in pts:
            v = self.bm.verts.new(Vector(p))
            v[self.color] = (*color, 1.0)
            verts.append(v)
        self.bm.faces.new(verts)

    def box(self, center, size, color, top=None):
        """Pavé centré en `center` (x, y) et posé à z = center.z."""
        cx, cy, z0 = center
        sx, sy, sz = size
        x0, x1, y0, y1, z1 = cx - sx / 2, cx + sx / 2, cy - sy / 2, cy + sy / 2, z0 + sz
        c = color
        self.quad([(x0, y0, z1), (x1, y0, z1), (x1, y1, z1), (x0, y1, z1)], top or c)
        self.quad([(x0, y1, z0), (x1, y1, z0), (x1, y0, z0), (x0, y0, z0)], c)
        self.quad([(x0, y0, z0), (x1, y0, z0), (x1, y0, z1), (x0, y0, z1)], c)
        self.quad([(x1, y1, z0), (x0, y1, z0), (x0, y1, z1), (x1, y1, z1)], c)
        self.quad([(x0, y1, z0), (x0, y0, z0), (x0, y0, z1), (x0, y1, z1)], c)
        self.quad([(x1, y0, z0), (x1, y1, z0), (x1, y1, z1), (x1, y0, z1)], c)

    def strut(self, a, b, r, color):
        """Barre de section carrée `2r` de `a` à `b` (4 faces, extrémités ouvertes)."""
        a, b = Vector(a), Vector(b)
        axis = (b - a).normalized()
        helper = Vector((0, 0, 1)) if abs(axis.z) < 0.9 else Vector((1, 0, 0))
        u = axis.cross(helper).normalized() * r
        w = axis.cross(u).normalized() * r
        corners = [u + w, -u + w, -u - w, u - w]
        for i in range(4):
            c0, c1 = corners[i], corners[(i + 1) % 4]
            self.quad([a + c0, b + c0, b + c1, a + c1], color)

    def wedge(self, base, top, color):
        """Prisme : quadrilatère de base et quadrilatère de dessus (4 + 4 points)."""
        for q in ([base[0], base[1], base[2], base[3]][::-1], top):
            self.quad(q, color)
        for i in range(4):
            j = (i + 1) % 4
            self.quad([base[i], base[j], top[j], top[i]], color)

    def to_object(self, name):
        mesh = bpy.data.meshes.new(name)
        bmesh.ops.recalc_face_normals(self.bm, faces=self.bm.faces)
        self.bm.to_mesh(mesh)
        mesh.update()
        obj = bpy.data.objects.new(name, mesh)
        bpy.context.scene.collection.objects.link(obj)
        return obj


def truss_box(p, x0, x1, y0, y1, z0, z1, panels_axis, panels, r, color, faces="xyzw"):
    """Treillis parallélépipédique : 4 membrures, montants et diagonales.

    `panels_axis` : axe de la longueur ('x' ou 'z'), découpée en `panels`.
    """
    def at(t, i):
        # i : coin de la section (0..3), t : position le long de l'axe.
        if panels_axis == "z":
            xs, ys = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)][i]
            return Vector((xs, ys, z0 + (z1 - z0) * t))
        ys, zs = [(y0, z0), (y1, z0), (y1, z1), (y0, z1)][i]
        return Vector((x0 + (x1 - x0) * t, ys, zs))

    for i in range(4):
        p.strut(at(0, i), at(1, i), r * 1.6, color)
    for k in range(panels + 1):
        t = k / panels
        for i in range(4):
            p.strut(at(t, i), at(t, (i + 1) % 4), r, color)
    for k in range(panels):
        t0, t1 = k / panels, (k + 1) / panels
        for i in range(4):
            a, b = (t0, t1) if (k + i) % 2 == 0 else (t1, t0)
            p.strut(at(a, i), at(b, (i + 1) % 4), r, color)


def make_mast():
    p = Prop()
    truss_box(p, -0.8, 0.8, -0.8, 0.8, 0.0, 3.0, "z", 2, 0.04, YELLOW)
    return p


def make_crane_top():
    p = Prop()
    # Pivot (couronne d'orientation) et plateforme.
    p.box((0, 0, 0.0), (2.2, 2.2, 0.6), DARK_YELLOW)
    p.box((0, 0, 0.6), (2.6, 2.6, 0.25), STEEL)
    # Cabine à l'avant, vitrée.
    p.box((1.2, -1.6, 0.85), (1.8, 1.4, 2.0), WHITE)
    p.box((2.11, -1.6, 1.5), (0.02, 1.2, 1.1), GLASS)
    p.box((1.2, -2.31, 1.5), (1.5, 0.02, 1.1), GLASS)
    # Porte-flèche (pyramide en treillis), sommet à 7 m.
    apex = Vector((0, 0, 7.6))
    for x, y in [(-0.9, -0.9), (0.9, -0.9), (0.9, 0.9), (-0.9, 0.9)]:
        p.strut(Vector((x, y, 0.85)), apex, 0.07, YELLOW)
    for z in (2.6, 4.4, 6.0):
        k = (z - 0.85) / (apex.z - 0.85)
        s = 0.9 * (1 - k)
        pts = [Vector((sx * s, sy * s, z)) for sx, sy in [(-1, -1), (1, -1), (1, 1), (-1, 1)]]
        for i in range(4):
            p.strut(pts[i], pts[(i + 1) % 4], 0.04, YELLOW)
    # Flèche : treillis triangulaire de 30 m (vers +X), section 1,2 m.
    length, panels = 30.0, 15
    base_z = 0.85
    for k in range(panels + 1):
        x = 0.9 + length * k / panels
        a, b, c = Vector((x, -0.6, base_z)), Vector((x, 0.6, base_z)), Vector((x, 0.0, base_z + 1.3))
        p.strut(a, b, 0.035, YELLOW)
        p.strut(b, c, 0.035, YELLOW)
        p.strut(c, a, 0.035, YELLOW)
        if k < panels:
            x1 = 0.9 + length * (k + 1) / panels
            a1, b1, c1 = Vector((x1, -0.6, base_z)), Vector((x1, 0.6, base_z)), Vector((x1, 0.0, base_z + 1.3))
            for s0, s1 in ((a, a1), (b, b1), (c, c1)):
                p.strut(s0, s1, 0.06, YELLOW)
            p.strut(a, c1, 0.03, YELLOW)
            p.strut(b1, c, 0.03, YELLOW)
            p.strut(a1, b, 0.03, YELLOW)
    # Contre-flèche (vers -X), passerelle et contrepoids en béton.
    p.box((-5.5, 0, 0.6), (9.0, 1.6, 0.35), YELLOW)
    for i, x in enumerate((-7.8, -8.9)):
        p.box((x, 0, 0.95), (0.9, 1.8, 1.8 - 0.3 * i), CONCRETE)
    p.box((-4.0, 0, 0.95), (1.6, 1.4, 1.0), STEEL)  # treuil
    # Tirants depuis le porte-flèche.
    p.strut(apex, Vector((18.0, 0, base_z + 1.3)), 0.03, STEEL)
    p.strut(apex, Vector((-9.3, 0, 1.0)), 0.03, STEEL)
    # Chariot, câble et moufle.
    p.box((16.0, 0, base_z - 0.35), (1.2, 1.4, 0.35), STEEL)
    for y in (-0.15, 0.15):
        p.strut(Vector((16.0, y, base_z - 0.35)), Vector((16.0, y, base_z - 12.0)), 0.012, BLACK)
    p.box((16.0, 0, base_z - 12.8), (0.5, 0.5, 0.8), YELLOW)
    p.strut(Vector((16.0, 0, base_z - 12.8)), Vector((16.0, 0, base_z - 13.4)), 0.05, STEEL)
    return p


def make_excavator():
    p = Prop()
    # Train de chenilles.
    for y in (-1.15, 1.15):
        p.wedge(
            [(-2.2, y - 0.3, 0.0), (2.2, y - 0.3, 0.0), (2.2, y + 0.3, 0.0), (-2.2, y + 0.3, 0.0)],
            [(-2.6, y - 0.3, 0.85), (2.6, y - 0.3, 0.85), (2.6, y + 0.3, 0.85), (-2.6, y + 0.3, 0.85)],
            BLACK,
        )
    p.box((0, 0, 0.35), (3.4, 1.8, 0.5), STEEL)
    # Tourelle : caisse, contrepoids, cabine vitrée.
    p.box((-0.3, 0, 0.95), (3.4, 2.4, 1.1), YELLOW)
    p.box((-1.95, 0, 0.95), (0.4, 2.4, 1.2), DARK_YELLOW)
    p.box((0.9, -0.65, 2.05), (1.2, 1.0, 1.4), YELLOW)
    p.box((1.51, -0.65, 2.25), (0.02, 0.9, 1.05), GLASS)
    p.box((0.9, -1.16, 2.25), (1.0, 0.02, 1.05), GLASS)
    p.box((0.9, -0.14, 2.25), (1.0, 0.02, 1.05), GLASS)
    # Flèche, balancier, godet.
    root = Vector((1.3, 0.35, 1.8))
    elbow = Vector((4.2, 0.35, 4.0))
    wrist = Vector((6.0, 0.35, 1.0))
    for a, b, r in ((root, elbow, 0.22), (elbow, wrist, 0.17)):
        p.strut(a, b, r, YELLOW)
    p.strut(Vector((1.0, 0.35, 1.5)), Vector((3.0, 0.35, 3.2)), 0.07, STEEL)  # vérin
    p.wedge(
        [(5.6, -0.25, 0.1), (6.4, -0.25, 0.1), (6.4, 0.95, 0.1), (5.6, 0.95, 0.1)],
        [(5.7, -0.25, 1.0), (6.2, -0.25, 1.0), (6.2, 0.95, 1.0), (5.7, 0.95, 1.0)],
        DARK_YELLOW,
    )
    return p


def make_cabin():
    p = Prop()
    p.box((0, 0, 0.0), (6.0, 2.4, 2.6), WHITE, top=(0.75, 0.75, 0.74))
    for x in (-1.8, 0.4):
        p.box((x, -1.21, 1.0), (1.2, 0.02, 1.0), GLASS)
    p.box((2.0, -1.21, 0.1), (0.9, 0.02, 2.1), BLUE)
    p.box((0, 0, 2.6), (6.1, 2.5, 0.12), BLUE)
    p.box((0, 0, -0.3), (5.6, 2.0, 0.3), STEEL)
    return p


def export(obj, path):
    bpy.ops.object.select_all(action="DESELECT")
    obj.select_set(True)
    bpy.context.view_layer.objects.active = obj
    bpy.ops.export_scene.gltf(
        filepath=path, export_format="GLB", use_selection=True, export_apply=True, export_yup=True,
        export_normals=True, export_vertex_color="ACTIVE", export_materials="NONE", export_texcoords=False,
    )


def main():
    bpy.ops.wm.read_factory_settings(use_empty=True)
    os.makedirs(OUT_DIR, exist_ok=True)
    for name, builder in (
        ("grue_mat", make_mast),
        ("grue_tete", make_crane_top),
        ("pelleteuse", make_excavator),
        ("bungalow", make_cabin),
    ):
        obj = builder().to_object(name)
        export(obj, os.path.join(OUT_DIR, f"{name}.glb"))
        print("objet", name, len(obj.data.polygons), "faces")


main()
