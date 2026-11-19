"""A made-up figure to try a pal's poses on: a mannequin of simple pieces on a person's skeleton,
its own left orange and its right blue, a red nose where it faces (docs/spatipal-pals.md).

    blender -b --python tools/mannequin.py -- out.glb [--idle]

Its skeleton is laid out as Blender's own human (hips, spine, chest, neck, head; a collarbone,
upper arm, forearm and hand each side; thigh, shin and foot), arms down at 45 degrees, facing
Blender's -Y (glTF's +Z). Each piece hangs from one bone alone, so each bone's turn shows as it
is. `--idle` gives it an `idle` that breathes and sways a little, to see a pose held over one.
"""

import math
import sys

import bmesh
import bpy
from mathutils import Matrix, Vector

args = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
out = next((a for a in args if not a.startswith("--")), "mannequin.glb")
idle = "--idle" in args

bpy.ops.wm.read_factory_settings(use_empty=True)
scene = bpy.context.scene

# Blender's frame: +X its left, -Y its front, +Z up. Metres.
BONES = [
    ("hips", None, (0, 0, 0.95), (0, 0, 1.05)),
    ("spine", "hips", (0, 0, 1.05), (0, 0, 1.25)),
    ("chest", "spine", (0, 0, 1.25), (0, 0, 1.42)),
    ("neck", "chest", (0, 0, 1.42), (0, 0, 1.52)),
    ("head", "neck", (0, 0, 1.52), (0, 0, 1.75)),
]
for side, s in (("L", 1), ("R", -1)):
    BONES += [
        (f"shoulder.{side}", "chest", (0.03 * s, -0.02, 1.40), (0.17 * s, 0, 1.42)),
        (f"upper_arm.{side}", f"shoulder.{side}", (0.17 * s, 0, 1.42), (0.37 * s, 0.01, 1.22)),
        (f"forearm.{side}", f"upper_arm.{side}", (0.37 * s, 0.01, 1.22), (0.55 * s, 0, 1.04)),
        (f"hand.{side}", f"forearm.{side}", (0.55 * s, 0, 1.04), (0.62 * s, 0, 0.97)),
        (f"thigh.{side}", "hips", (0.09 * s, 0, 0.92), (0.10 * s, -0.01, 0.50)),
        (f"shin.{side}", f"thigh.{side}", (0.10 * s, -0.01, 0.50), (0.10 * s, 0.02, 0.08)),
        (f"foot.{side}", f"shin.{side}", (0.10 * s, 0.02, 0.08), (0.10 * s, -0.12, 0.02)),
    ]

arm = bpy.data.armatures.new("Mannequin")
rig = bpy.data.objects.new("Mannequin", arm)
scene.collection.objects.link(rig)
bpy.context.view_layer.objects.active = rig
bpy.ops.object.mode_set(mode="EDIT")
for name, parent, head, tail in BONES:
    b = arm.edit_bones.new(name)
    b.head, b.tail = head, tail
    if parent:
        b.parent = arm.edit_bones[parent]
bpy.ops.object.mode_set(mode="OBJECT")

COLOURS = {
    "body": (0.55, 0.56, 0.60),
    "skin": (0.85, 0.72, 0.62),
    "left": (0.95, 0.50, 0.12),
    "right": (0.15, 0.40, 0.90),
    "nose": (0.85, 0.10, 0.10),
    "eye": (0.05, 0.05, 0.08),
}
mesh = bpy.data.meshes.new("Body")
body = bpy.data.objects.new("Body", mesh)
scene.collection.objects.link(body)
for name, rgb in COLOURS.items():
    m = bpy.data.materials.new(name)
    try:
        m.use_nodes = True
    except AttributeError:
        pass
    m.node_tree.nodes["Principled BSDF"].inputs["Base Color"].default_value = (*rgb, 1.0)
    mesh.materials.append(m)
material = {name: k for k, name in enumerate(COLOURS)}
groups = {name: body.vertex_groups.new(name=name).index for name, *_ in BONES}

bm = bmesh.new()
deform = bm.verts.layers.deform.verify()


def along(a, b):
    """A matrix whose Z runs from a to b, at their middle."""
    a, b = Vector(a), Vector(b)
    turn = Vector((0, 0, 1)).rotation_difference((b - a).normalized()).to_matrix().to_4x4()
    return Matrix.Translation((a + b) / 2) @ turn, (b - a).length


def piece(make, bone, colour, smooth=True):
    """A piece made by `make`, hung from `bone` alone, in `colour`."""
    faces_before = set(bm.faces)
    verts = make()["verts"]
    for v in verts:
        v[deform][groups[bone]] = 1.0
    for f in bm.faces:
        if f not in faces_before:
            f.material_index = material[colour]
            f.smooth = smooth


def rod(bone, a, b, radius, colour, end=None):
    m, length = along(a, b)
    piece(lambda: bmesh.ops.create_cone(bm, cap_ends=True, segments=14, radius1=radius, radius2=end or radius, depth=length, matrix=m), bone, colour)


def ball(bone, at, size, colour, turn=Matrix.Identity(4)):
    m = Matrix.Translation(at) @ turn @ Matrix.Diagonal((*size, 1.0))
    piece(lambda: bmesh.ops.create_uvsphere(bm, u_segments=16, v_segments=10, radius=1.0, matrix=m), bone, colour)


def block(bone, m, colour):
    piece(lambda: bmesh.ops.create_cube(bm, size=1.0, matrix=m), bone, colour, smooth=False)


ball("hips", (0, 0, 0.95), (0.16, 0.10, 0.10), "body")
rod("spine", (0, 0, 1.0), (0, 0, 1.25), 0.11, "body", end=0.13)
ball("chest", (0, 0, 1.33), (0.17, 0.10, 0.13), "body")
rod("neck", (0, 0, 1.42), (0, 0, 1.53), 0.045, "skin")
ball("head", (0, 0, 1.63), (0.09, 0.10, 0.115), "skin")
m, _ = along((0, -0.08, 1.62), (0, -0.14, 1.60))
piece(lambda: bmesh.ops.create_cone(bm, cap_ends=True, segments=10, radius1=0.022, radius2=0.004, depth=0.06, matrix=m), "head", "nose")
for x in (0.035, -0.035):
    ball("head", (x, -0.085, 1.665), (0.014, 0.01, 0.014), "eye")
for side, s, colour in (("L", 1, "left"), ("R", -1, "right")):
    bone = dict((n, (h, t)) for n, _, h, t in BONES)
    h, t = bone[f"shoulder.{side}"]
    rod(f"shoulder.{side}", (0.06 * s, -0.01, 1.40), t, 0.04, "body")
    h, t = bone[f"upper_arm.{side}"]
    ball(f"upper_arm.{side}", h, (0.055, 0.055, 0.055), colour)
    rod(f"upper_arm.{side}", h, t, 0.045, colour, end=0.04)
    h, t = bone[f"forearm.{side}"]
    ball(f"forearm.{side}", h, (0.042, 0.042, 0.042), colour)
    rod(f"forearm.{side}", h, t, 0.038, colour, end=0.03)
    # A hand, its palm towards the thigh at rest, its thumb forwards.
    h, t = bone[f"hand.{side}"]
    m, length = along(h, (Vector(t) - Vector(h)) * 1.4 + Vector(h))
    block(f"hand.{side}", m @ Matrix.Diagonal((0.025, 0.075, length, 1.0)), "skin")
    thumb, _ = along(Vector(h) + Vector((0, -0.035, -0.02)), Vector(h) + Vector((0.02 * s, -0.07, -0.06)))
    piece(lambda: bmesh.ops.create_cone(bm, cap_ends=True, segments=8, radius1=0.012, radius2=0.01, depth=0.05, matrix=thumb), f"hand.{side}", "skin")
    h, t = bone[f"thigh.{side}"]
    rod(f"thigh.{side}", h, t, 0.075, colour, end=0.055)
    h, t = bone[f"shin.{side}"]
    ball(f"shin.{side}", h, (0.055, 0.055, 0.055), colour)
    rod(f"shin.{side}", h, t, 0.05, colour, end=0.035)
    h, t = bone[f"foot.{side}"]
    block(f"foot.{side}", Matrix.Translation((0.10 * s, -0.05, 0.035)) @ Matrix.Diagonal((0.085, 0.24, 0.06, 1.0)), "skin")

bm.to_mesh(mesh)
bm.free()
body.parent = rig
body.modifiers.new("Armature", "ARMATURE").object = rig

if idle:
    # Breathing, and the arms swaying a little, over two seconds at 30 a second.
    bpy.context.view_layer.objects.active = rig
    bpy.ops.object.mode_set(mode="POSE")
    for b in rig.pose.bones:
        b.rotation_mode = "QUATERNION"
    frames = [(1, 0.0), (31, 1.0), (61, 0.0)]
    for frame, k in frames:
        rig.pose.bones["chest"].rotation_quaternion = Matrix.Rotation(math.radians(-2.0 * k), 4, "X").to_quaternion()
        rig.pose.bones["head"].rotation_quaternion = Matrix.Rotation(math.radians(3.0 * k), 4, "X").to_quaternion()
        for side, s in (("L", 1), ("R", -1)):
            rig.pose.bones[f"upper_arm.{side}"].rotation_quaternion = Matrix.Rotation(math.radians(4.0 * k * s), 4, "Y").to_quaternion()
        for name in ("chest", "head", "upper_arm.L", "upper_arm.R"):
            rig.pose.bones[name].keyframe_insert("rotation_quaternion", frame=frame)
    rig.animation_data.action.name = "idle"
    bpy.ops.object.mode_set(mode="OBJECT")

bpy.ops.export_scene.gltf(filepath=out, export_format="GLB", export_yup=True, export_animations=idle, export_skins=True, export_morph=False, export_def_bones=True)
print(f"mannequin: {out}: {len(BONES)} bones, {len(mesh.vertices)} vertices, idle {idle}")
