"""Planche de contrôle des arbres générés (LOD0 devant, LOD1 derrière).

    blender -b --factory-startup --python tools/blender/preview_trees.py -- crates/render/assets/trees sortie.png
"""

import math
import os
import sys

import bpy
from mathutils import Vector

ARGS = sys.argv[sys.argv.index("--") + 1:]
SRC, OUT = os.path.abspath(ARGS[0]), os.path.abspath(ARGS[1])
SPECIES = ["epicea", "pin", "chene", "bouleau"]

bpy.ops.wm.read_factory_settings(use_empty=True)
scene = bpy.context.scene

mat = bpy.data.materials.new("vertex_color")
mat.use_nodes = True
nodes = mat.node_tree.nodes
bsdf = next(n for n in nodes if n.type == "BSDF_PRINCIPLED")
attr = nodes.new("ShaderNodeAttribute")
attr.attribute_name = "Color"
attr.attribute_type = "GEOMETRY"
mat.node_tree.links.new(attr.outputs["Color"], bsdf.inputs["Base Color"])
bsdf.inputs["Roughness"].default_value = 0.9

for i, name in enumerate(SPECIES):
    for lod, y in ((0, 0.0), (1, 16.0)):
        bpy.ops.import_scene.gltf(filepath=os.path.join(SRC, f"{name}_lod{lod}.glb"))
        for obj in bpy.context.selected_objects:
            if obj.type == "MESH":
                obj.location = Vector((i * 14.0, y, 0.0))
                obj.data.materials.clear()
                obj.data.materials.append(mat)
                if obj.data.color_attributes:
                    attr.attribute_name = obj.data.color_attributes[0].name

bpy.ops.mesh.primitive_plane_add(size=200, location=(21, 8, 0))
ground = bpy.context.active_object
gmat = bpy.data.materials.new("ground")
gmat.use_nodes = True
next(n for n in gmat.node_tree.nodes if n.type == "BSDF_PRINCIPLED").inputs["Base Color"].default_value = (0.25, 0.3, 0.15, 1)
ground.data.materials.append(gmat)

sun = bpy.data.objects.new("sun", bpy.data.lights.new("sun", "SUN"))
sun.data.energy = 4.0
sun.rotation_euler = (math.radians(50), 0, math.radians(35))
scene.collection.objects.link(sun)
world = bpy.data.worlds.new("w")
world.use_nodes = True
world.node_tree.nodes["Background"].inputs[0].default_value = (0.55, 0.68, 0.85, 1)
world.node_tree.nodes["Background"].inputs[1].default_value = 0.8
scene.world = world

cam = bpy.data.objects.new("cam", bpy.data.cameras.new("cam"))
cam.location = Vector((21, -64, 16))
direction = Vector((21, 8, 9)) - cam.location
cam.rotation_euler = direction.to_track_quat("-Z", "Y").to_euler()
scene.collection.objects.link(cam)
scene.camera = cam

scene.render.engine = "CYCLES"
scene.cycles.samples = 24
scene.cycles.device = "CPU"
scene.render.resolution_x, scene.render.resolution_y = 1400, 700
scene.render.filepath = OUT
bpy.ops.render.render(write_still=True)
print("rendu :", OUT)
