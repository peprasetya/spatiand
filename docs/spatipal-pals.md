# SpatiPal pals: someone of your own

The owner, 2026-11-19, of a SpatiPal of his own making that stays his: it "should not include in
the spatiand source code... what can goes public is the documentation to have custom model", with a
figure that can "change appearance, clothes, hair, pose, animation", made on hemera and "simple
(not very high detail polygon) for fast 3d".

A **pal** is that: a folder the wearer makes, outside Spatiand's source, which SpatiPal becomes
when it is chosen -- a name it answers to, who it is, a figure that changes clothes and moves, a
voice, models of its own. Nothing of any pal is in this repository. This is how a pal is made, what
SpatiPal and the room do with one, and how one was made on hemera.

## What a pal is

| | |
|---|---|
| Where it is kept | `~/.config/spatiand/apps/assistant/pals/<folder>/`: the folder spatiand-sync shares between the wearer's linked machines, peer to peer, so a pal made on one is on all of them -- the Deck, the Mac, the Beam Pro. **A file over 4 MB is not sent** (spatiand-sync's limit for a setting): keep a figure under it, or copy it to each machine. |
| How it is chosen | SpatiPal's Settings, **Pal**: it steps through None and the pals there are. None is SpatiPal as its settings have it (Pixel, the robot). |
| What it sets | Its name (the wake word), who it is, its figure, its voice, and its models if it names them -- over the settings' own, which are kept as they were and come back when no pal is chosen. The settings page shows those lines as the pal's and does not show its persona at all: the window is in the room. |
| Its chats | Its own. The chat list shows the chats had with the pal chosen; another pal's, and those of SpatiPal with no pal, are not shown, and are still there when that one is chosen again. |
| Private | `"private": true` keeps what is heard and said out of SpatiPal's log -- only how many letters there were is written -- and its window describes no chat to the room (dictation and the assistant read windows by their descriptions). |
| Where its words go | To the models on hemera, and nowhere else. Its persona is part of the system message of every turn. |
| On every platform | The same: SpatiPal and the room's figure renderer are the same code on the Deck, the Mac and the Beam Pro. |

## The folder

```text
pal.json      what it is called, its files, its voice and models; its figure's choices in words
persona.md    who it is
figure.glb    its figure (or a .vrm)
look.json     how it looks now: written by SpatiPal as it changes, so it looks the same tomorrow
```

The folder and its files should be the wearer's alone (`chmod 700` the folder, `600` the files);
SpatiPal writes `look.json` that way.

### pal.json

Every field may be left out.

| Field | |
|---|---|
| `name` | What it answers to and calls itself. |
| `persona` | A file in the folder (`persona.md`), read whole; or, when there is no such file, the words themselves. HTML comments (`<!-- ... -->`) in it are its maker's notes and are not given to the model. |
| `figure` | Its figure: a file in the folder. None: the figure chosen in the settings. |
| `size` | Its figure's height against the room's usual, 0.5 to 3. The room draws a larger figure into a larger picture (512 pixels square to 1024), so it is as sharp. |
| `voice`, `described` | A voice from hemera's library by name, or one described in words, made into a library voice once (as the settings' is) so that every sentence is the same voice. |
| `model`, `large_model` | Its own everyday and larger models; the settings' when left out. |
| `private` | See above. |
| `signs` | `false` takes away the robot's lights over its head (listening, writing down, thinking): for a figure whose own animations show what it is doing. |
| `options` | Its figure's options in words, by group, for its model: `{"outfit": {"dress": "a short black dress and heels"}}`. |
| `moves`, `poses` | Its animations in words: `{"wave": "a wave"}`. A move is played once; a pose is held until another is chosen, or none. An animation in neither is offered as a move, unless it is one the room plays by itself (below). |
| `faces` | Its expressions in words: offered to hold (a pout, a smirk). |
| `colours` | Its materials that may be given another colour, in words: `{"hair": "your hair"}`. |

Edited, a pal's files are read again within ten seconds, on whichever machine has it running; a
file that cannot be read just then (half synchronised) leaves the pal as it was.

### persona.md

Who it is, in its own second person ("You are ..."), as long as it needs. It comes after
SpatiPal's own first line -- "You are <name>, the wearer's assistant in Spatiand, a 3D desktop
seen through glasses" -- and before SpatiPal's own rules, which every pal keeps: speak in short
sentences, in the wearer's language, with no stage directions (`*smiles*`: its figure acts
instead), never code aloud; ask before closing a window, deleting,
sending or buying anything, or pressing Enter in a terminal (closing a window and Enter are held
by SpatiPal itself until the wearer says yes, whatever a persona says). Say how it uses its
figure: when it changes clothes, which moves fit which moods.

The everyday model is SpatiPal's small one unless the pal names its own; a long persona is read
once and kept by the server (the system message is the same every turn), so its length costs the
first answer only.

## The figure

A GLB, or a VRM (which is a GLB with a humanoid and its expressions). The room reads it itself
(`crates/spatiand/src/figure.rs`), the same way on every platform, and draws it low in the corner
of the view opposite the pinned windows, facing the wearer, about 15 cm tall at 1.35 m (times
`size`). glTF's way up: +Y up, facing +Z, metres. A VRM 0.x, which faces -Z, is turned.

### Its skeleton

glTF skins: each vertex hangs from up to four joints, by its weights. The room poses the skeleton
itself, on the CPU -- a figure is thousands of vertices, not hundreds of thousands, and the same
code then runs on every GPU the room runs on. A figure with no animations stands as it was made,
its skinned parts where its skeleton at rest puts them. Parts not skinned, under a node an
animation moves, move with it rigidly.

### Its animations

glTF animations, by name, of nodes' translation, rotation and scale; step, linear and cubic
spline keys. Morph-weight animations are not played (the room moves the face itself).

**The room plays one by itself for what SpatiPal is doing**, named after its state, and blends
from one to the next over 0.3 s:

| State | When | Played, the first the figure has |
|---|---|---|
| `idle` | not listening | `idle` |
| `listening` | listening for its name | `listening`, `idle` |
| `checking` | a phrase being written, out of a conversation, to see if it starts with its name | `checking`, `listening`, `idle` |
| `attentive` | in a conversation: no name needed | `attentive`, `listening`, `idle` |
| `hearing` | the wearer speaking to it | `hearing`, `attentive`, `listening`, `idle` |
| `transcribing` | what they said being written down | `transcribing`, `thinking`, `idle` |
| `thinking` | the small model answering | `thinking`, `idle` |
| `pondering` | the larger model thinking | `pondering`, `thinking`, `idle` |
| `working` | doing something in the room | `working`, `thinking`, `idle` |
| `talking` | its voice being heard | `talking`, `idle` |
| `happy` | an answer just heard out | `happy`, `idle` |

So `idle` is the one every figure that moves should have. Every other animation is its own, for
its model to choose: a **move** plays once, then the figure goes back to its state's; a **pose**
loops until another pose, or none, is chosen -- while it talks, listens, thinks. A move asked for
in a pose plays, then the pose comes back. A name is matched in any case, and an exporter's
`Armature|wave` is `wave`.

### Poses of its own

A figure with a person's skeleton can also be posed **in words, by its pal's model** (the owner,
2026-11-19: "if animation too complex, then pose") -- no animation needed for it. Its model names
the parts it moves and where each points, from its own point of view:

```json
{"left_upper_arm": "down, out, a little back", "left_forearm": "in, a little down, a little front",
 "head": "a little down, tilt right"}
```

- **The words** (`spatiand_voice::figure::aim`, read the same by SpatiPal and the room): `front`,
  `back`, `up`, `down`, `left`, `right`, `out` (away from its middle), `in` (across it), any of
  them `a little`. An upper arm points where its elbow goes, a forearm where its hand goes (or
  `straight`), a hand where its fingers go, a thigh where its knee goes, a shin where its foot goes
  (or `straight`); the head looks, and tilts (`tilt left`); the chest leans, and turns
  (`turn right`); a shoulder goes up, front or back. `relaxed` leaves a part to its animation.
  Words that point nowhere, or that say more than a way to point (a model thinking aloud in its
  call), are answered with how to say it.
- **The parts** (`BODY_PARTS`): `head`, `chest`, `left_`/`right_` + `shoulder`, `upper_arm`,
  `forearm`, `hand`, `thigh`, `shin`. **Which joints they are**: a VRM's humanoid, as it names its
  bones; any other skeleton found by its shape at rest, whatever its joints are called -- its feet
  the lowest joints either side, its hips where their legs meet, its spine the way up from there,
  its arms the branches reaching farthest out; of each limb the two longest bones in a row (an
  upper arm and a forearm past a collarbone; a thigh and a shin). A collarbone, when there is one,
  is a shoulder. A fox, a robot's arm, a figure with no legs: no body, and no poses of its own.
- **How the room turns them** (`crates/spatiand/src/posing.rs`): each bone the short way from
  where it points to where it is asked to; an elbow or a knee bends as a person's does, one way and
  only so far (150 degrees), the upper bone first turned about itself (up to 100 degrees at a
  shoulder, 45 at a hip) so that its hinge bends towards where the lower bone is to point; a lean
  of the chest shared along the spine, a look along the neck (most of it the head's). An arm
  pointed across the chest goes in front of it; a knee lifts no higher than a little above level.
  Over whatever animation is playing -- `idle`, `talking` -- for the parts it names; a move plays
  whole and the pose comes back after it; a change of pose takes 0.6 s.
- **Poses by name, made of words**, for a figure with a body: `hands_on_hips`, `left_hand_on_hip`
  and `right_hand_on_hip`, `arms_folded`, `thinking`, `shrug`, `left_hand_raised` and
  `right_hand_raised`, `arms_up`, `hands_behind_head`, `hands_behind_back`, `lean_in`, `point`,
  `look_over_left_shoulder` and `look_over_right_shoulder`, `right_hand_on_heart` and
  `left_hand_on_heart`, `look_up`, `look_down`, `head_tilt` (`STOCK` in
  `crates/spatipal/src/pack.rs`, each looked at from the front and the side; `hands_behind_back`
  from the back too). One of the figure's own animations of the same name is its own instead; a
  part it lacks is left out. Any other pose is made from words (below): the small model, given
  every pose by name and left to make its own part by part, chose the nearest name instead
  ("look up at the sky" over its shoulder), so it is told to take one of these only by its name,
  and is not given `own_pose` at all.

### Its options

**A node named `group=option` is one of that group's options, with everything under it**:
`outfit=dress`, `outfit=jeans`, `hair=bob`, `hair=long`. The room shows only the option chosen of
each group, and a group's first (in the scene, parents before children) until one is chosen.
Several nodes may make up one option, and Blender's way of keeping names apart (`outfit=dress.001`,
always three digits) is the same option; a node of no group (the body, the eyes) is always there.
An option that is nothing -- no hat -- is an empty node: `hat=none`. A group named `move`, `pose`
or `face` is not offered (those are the tool's own words).

Options can be whole looks, each a mesh of the figure in other clothes and hair, or parts over one
body: a body always there, clothes and hair in groups of their own -- a doll, dressed (the owner,
2026-11-19: "the body is naked... can be dressed... concept like second life avatar"; "dont
hide... but at layer"). Clothes made for the body and fitted to it, each with its own weights,
move with the skin; a few millimetres between them keep the skin from showing through, so nothing
of the body need be hidden or cut away (below).

### Its face

A VRM's expressions, by VRM 1.0's names (0.x's are read under them: `joy` is `happy`); a plain
GLB's morph targets by their own names -- what Blender calls shape keys, which it exports as the
mesh's `extras.targetNames`. The room moves `blink` now and then, `aa` and `oh` while its voice is
heard, `happy` after an answer, `relaxed`, `surprised` and the looks (`lookUp`, `lookDown`) with
the states. Any other can be held, as its model asks (`faces` in `pal.json`). A MakeHuman body
gets them all from MakeHuman's own expression units (below).

### Its colours

A material may be given another colour by its name: the colour takes the place of the material's
own, tinting a picture on it as glTF's base colour does. Offered to its model only for the materials
`colours` in `pal.json` names. Several materials may share a name, and Blender's way of keeping
names apart (`hair.001`, three digits) is the same name: a doll's three hairstyles, each its own
picture, are all its `hair` (`spatiand_voice::figure::material_name`). A colour cannot lighten a
dark picture: a material meant to be recoloured is best plain, or its picture grey -- only its
light and shade -- with its own colour as the material's base colour, which a colour asked for
then takes the place of (as the doll below).

### Not read

Draco and meshopt compression, KTX2 and Basis pictures (drawn white), glass
(`KHR_materials_transmission`, drawn opaque), data outside the file, more than four joints to a
vertex, morph-weight animations. Pictures are made at most 1024 pixels across. Sparse accessors
(Blender's shape keys) are read since 2026-11-19; before, they were taken as zeroes.

## What its model can do with it: `appearance`

When a pal's figure offers anything, its models get a tool, `appearance`, made from the figure
itself (`spatiand_voice::figure` reads what a GLB offers, for SpatiPal and the room alike) and the
words in `pal.json`: one choice per group of options (`"outfit": "dress"`), `move`, `pose` (or
`none`; for a figure with a body, the poses made of words too), `own_pose` (a pose of its own, as
above; its parts named in its description rather than as the schema's properties: hemera holds a
call to its schema, and llama.cpp holds an object's properties to the order they are listed in,
which put a forearm out of reach after its upper arm), `face` (or `none`), and
`<material>_colour` (`#rrggbb` or a common colour's name). The
tool's description says to use it when the wearer asks, or when the pal feels like it, and to say
so in its own way. Each turn the wearer's words come with how it looks now ("[How you look now:
outfit dress; holding the pose hand_on_hip.]"), so it knows what it is wearing. A choice that is
not there is answered with the ones that are, for the model to choose again.

SpatiPal keeps the look in `look.json` and tells the room, through `spatiand_assist_v1`'s `avatar`
(the protocol's notes, `crates/spatiand-proto/protocol/spatiand-xr-v1.xml`, have its fields).

## Moves and poses made from words: `make_move`

For a figure with a body, its models also get `make_move` (`crates/spatipal/src/moves.rs`; the
owner, 2026-11-19, of hemera's motion model: "build the Spatiand side"): anything a person's body
can do that is not one of its moves or poses -- a cartwheel, a bow, kneeling, standing on one leg
-- is made by hemera from words and kept, to be done again by its name.

| `kind` | | Kept as |
|---|---|---|
| `move` | done once, now; then it stands as before | `moves/<name>.json` |
| `pose` | a position held until another, or none -- its words say how it is while in it, not how it gets there | `poses/<name>.json` |
| `idle` | a quiet loop, how it stands whenever nothing else happens | `moves/idle.json` |
| `forget` | lets one it made go, by its name | -- |

- **The words** are hemera's HY-Motion's: one or two actions of "a person", in English, only the
  body ("a person kneels on one knee", "a person spins around twice with both arms out"); 1 to 10
  seconds, 3 or 4 by default. Each file keeps the words and the seed, which make it again exactly;
  80-300 KB each, synchronised with the pal.
- **Made** in about 7 s; 35-45 s more when hemera has first to load its model (21 GB, unloaded
  after ten idle minutes). The tool answers at once that it is not ready yet and that the pal will
  do it by itself the moment it is, and to answer in character that it is about to: told only to
  say something meanwhile, a 4B said "there you go" 45 s before the move came; told "then you do
  it", both models acted it out at once ("I'm standing on one leg now", "... there! Did you see
  that?"). Now: "Hold onto your virtual seats", "One second, I'm learning that spin!"
- **Done** as it comes: a move played once, a pose held (and kept in `look.json`), an idle the
  figure's from then on. Several may be made at once, each done as it comes. A pose still being
  made is not taken once another pose or move is asked for after it, or another pose taken -- it
  is kept, to take by its name later ("Actually no, just put your hands on your hips": the pose
  asked first came 7 s later, and was kept, not taken). Let go of while it is made, it is not kept.
  Made for a pal no longer in the room, it is kept for that pal and not done.
- **Not made again**: a name it has already -- its own, the figure's, a pose by name -- is done at
  once, and so is the only one whose name has all the words of the name asked ("hips":
  `hands_on_hips`). After one move was made, the small model made the next it was asked for the
  same way, though it was one of its poses.
- **Played by the room** (`crates/spatiand/src/motion.rs`) as the figure's own animations, made
  from the motion -- SMPL's 22 joints onto the body the room finds in the figure (above): each
  limb first brought from its rest to where it points in a T-pose, then turned as the motion turns
  it; the body and head as they are; a hand with its forearm (the motion has no fingers); the hips
  moved by the figure's leg over SMPL's, within half a leg of where it stands. A move fades in and
  out as the figure's own do, and a pose of words lies over it. A made one of the same name as the
  figure's own is the one done.

`appearance`'s moves and poses list the ones made ("a move of your own, made from words"). Every
turn's note tells the model that its body can do anything asked of it, by one of its moves or
poses or a new one made with `make_move`, never "I cannot": told only by the tool's description,
the small model still refused a backflip.

## Making a figure

Any GLB or VRM that keeps to the above. Three ways:

- **By hand in Blender**: a body and an armature, clothes and hair as objects named
  `outfit=...`, `hair=...`, each with an Armature modifier; one action per animation (`idle` and
  the states', then its own); shape keys for the face. Export as below.
- **VRoid Studio and the like** make VRMs with expressions and spring bones (spring bones are not
  simulated: hair stays where its bones put it). A VRM's licence is in its meta (`VRMC_vrm.meta`,
  0.x's `meta`): many forbid violent or sexual use, or use by anyone but their author.
- **A doll from MakeHuman**, in Blender: one body, clothes and hair fitted over it as layers,
  MakeHuman's own faces, colours that change. How one was made so on deepmagpie, 2026-11-19 and
  20, is below ("A doll from MakeHuman").
- **On hemera**, from words to a picture to a mesh, then Blender. How one figure was made so, on
  deepmagpie on 2026-11-19 -- five looks, each with its own clothes and hair, 32 animations, one
  3.5 MB GLB (a mesh a look, its clothes part of its skin: replaced by a doll the same day):

### From hemera

1. **A picture of each look**, from words: `POST /v1/images/generations` with
   `qwen_image_2.1_turbo_int8_convrot`, 768 x 1344, about 25 s each once loaded. The same words for
   the figure in every one, then that look's clothes and hair, and **the same seed**: the figure
   stays itself. Words that ask for what a mesh and a skeleton need: "full-body 3D render ... a
   semi-realistic game character, standing in an A-pose with arms angled down and slightly away
   from the body, palms toward the thighs, feet a little apart, facing the camera, front view, the
   whole body in frame ... plain light grey background, soft even studio lighting, no shadows".
   Editing the first picture into the others (`/v1/images/edits`, the same model) kept the face
   but mottled the skin and the background, at 8 steps and worse at 16: not for meshes.
2. **A mesh of each picture**: `POST /v1/3d/generations` (multipart: `model`
   `trellis_2_int8_convrot`, `texture` `vertex`, `image`), polled at `GET .../{id}` and fetched
   from `.../{id}/content`: about 140 s, 700 000 faces with colours on their vertices, 21 MB. Of
   hemera's three families TRELLIS.2 has the cleanest shape and the best colour; it followed the
   picture closely, arms clear of the body and legs apart. Afterwards
   `POST /hemera/api/models/stop` for the image and 3D models: hemera keeps what it loads.
3. **Low-poly, in Blender** (5.3, `blender -b --python`):
   - Each mesh scaled by **its body's height, not its hair's**: the tip of the nose (the front-most
     point of the head) at 0.925 of it -- a high ponytail made one mesh 7% taller than its body, and
     scaled by its whole height that look's body came out smaller than the others'.
   - A voxel remesh (the height / 280) makes one closed surface; then it is **closed**: swollen by
     12 mm, remeshed (/ 240), shrunk back, remeshed. Without that, thin parts -- loose knit, a
     messy bun's strands -- came out as sheets with up to a thousand tunnels through them, which
     stopped the decimator thousands of triangles short and shattered the unwrap into three
     thousand islands that bled into each other in the picture (a coarser remesh alone made more
     tunnels, not fewer). Closed, the worst look had 3 tunnels and 277 islands. Specks under 1%
     go.
   - Decimated to 5500 triangles; Smart UV Project at 89 degrees (few seams: each one splits the
     vertices along it); its vertex colours baked onto a 1024-pixel picture (Cycles, an Emission
     shader from the Color Attribute, selected to active, a 2 cm cage). Each look one object,
     named `outfit=<look>`.
4. **A skeleton for each look**, from its own shape -- heels, a bun and arms held a little otherwise
   put the joints elsewhere: joints at a person's proportions of the height (pelvis 0.50, waist
   0.58, ribs 0.67, neck 0.82, skull 0.885, shoulders 0.81), each arm along the line from its
   shoulder to the farthest point of its hand (elbow 0.42 of the way, wrist 0.76), knees and
   ankles from the legs' cross-sections. Depth is the middle of the body's front and back there --
   not a median of its points: a decimated mesh keeps more of them where it has more detail, its
   front, which put the spine in the belly. Bound with automatic weights, at most four joints to a
   vertex.
5. **Its animations, as poses in the figure's own frame**: each bone turned from where it stands
   at rest so that its end goes towards the figure's front, its back, up, down, out or in, or turned about
   itself -- the same words for both sides, mirrored -- so one set fits every skeleton. An arm
   raised to the side bends its elbow forwards; turned out first, upwards (a wave). Each
   animation one Blender action, keyed into a slot for each skeleton; a loop ends where it began.
   A morph target `aa` lets the jaw down a little, which the room moves while the figure talks.
6. **Exported**: GLB, pictures as JPEG at 82, `export_animation_mode='NLA_TRACKS'` -- each action
   an NLA track of its name on every skeleton, so each animation moves all of them (with several
   armatures, `'ACTIONS'` exports each one's active action only: one animation) -- sampled every
   fourth frame at 30 fps, deforming bones only, no vertex colours. Blender writes shape keys as
   sparse accessors, which the room reads.

Size, against spatiand-sync's 4 MB: five skeletons' animations were 1.6 MB of the 3.5, the
pictures 0.8, the meshes the rest. A sixth look wants fewer frames (`export_frame_step`) or
triangles.

The scripts for that figure are its owner's and are not here; the steps are all there is to them.

### A doll from MakeHuman

[MPFB](https://static.makehumancommunity.org/mpfb.html) 2 (MakeHuman's Blender extension; 2.0.18
in Blender 5.3 on deepmagpie), its body and its assets -- clothes, hair, brows, lashes, eyes,
skins, made for its body, each with its own fitting and weights -- scripted with
`blender -b --python`. The steps:

1. **The body**: `HumanService.create_human()`, its shape by macros
   (`HumanObjectProperties.set_value`: gender, age, muscle, weight, height, proportions, the
   ethnic mix...) and `TargetService.reapply_macro_details`; a skin
   (`HumanService.set_character_skin`, `GAMEENGINE`); the **`game_engine` skeleton**
   (`HumanService.add_builtin_rig`): 53 bones, `pelvis` up `spine_01`-`03` to `neck_01` and
   `head`, `clavicle`/`upperarm`/`lowerarm`/`hand` and fingers, `thigh`/`calf`/`foot`/`ball`,
   which the room finds a body in by its shape.
2. **Layers**: eyes, brows, lashes, then each hairstyle and each outfit's garments,
   `HumanService.add_mhclo_asset(..., subdiv_levels=0)`, their Subdivision modifiers taken off;
   each object named `hair=<style>` or `outfit=<look>` (several garments of one look are
   `outfit=<look>.001`, `.002`...); an empty `outfit=none`. Each garment pushed out along its
   normals by **4 mm**: fitted exactly, the skin showed through where it bends. **Nothing
   hidden**: MPFB hides the body under clothes with Mask modifiers by its delete-groups; they are
   taken off, so every outfit -- and none -- is over the whole body.
3. **Made lighter**: the targets baked (`TargetService.bake_targets`), helpers taken off
   (`ExportService.bake_modifiers_remove_helpers`), the body decimated to half (6700 vertices)
   before it is rigged further; layers as they are.
4. **Faces from MakeHuman's expression units**, which MPFB ships as targets
   (`targets/expression/units/<african|asian|caucasian>/`): an eye closed, opened or narrowed, the
   brows up, down or in, the mouth open, its corners pulled, pursed, pushed out, pressed... --
   sculpted for this body. Each is read on the whole body, before it is made lighter
   (`TargetService.load_target`, then the shape key's distance from the rest), blended by the
   body's ethnic mix as MakeHuman blends them. Each face is a mix of units, made a shape key of the
   lighter body vertex by vertex from **the four nearest of the whole body's that face the same
   way**, by nearness -- and of the brows and lashes, which lie on the skin, from the nearest.
   From the nearest alone, where the lips meet (the upper's inside facing down, the lower's up, a
   hair apart) an open mouth grew fangs, and a kiss creased the cheek. The faces made:
   `blink` (both eyes closed), `aa` (the mouth open), `oh` (open and pursed), `happy`, `relaxed`,
   `surprised` -- the room's -- and `smile`, `grin`, `wink`, `pout`, `kiss`, `angry`, `sad`,
   `eyebrow_raise` to hold. A new shape key from the rest shape (`from_mix=False`): MPFB pins the
   last made, and from the mix the second took the first in whole. Made by hand instead -- a jaw
   turned about a hinge placed by the head bone, lids turned about the eyes -- the mouth bent the
   cheeks (the game skeleton's head bone is not where the old figure's was) and the lashes tore
   from the lids.
5. **Materials, simply**: the base colour's picture and transparency where it is cut out (hair,
   brows, lashes, as PNG); everything else opaque (JPEG); no normal maps; pictures 1024 pixels at
   most for the skin, 512 for clothes, 256 for hair (a picture's size is 0 until it is loaded:
   load it before scaling it). **Colours that change**: each hairstyle's material named `hair`,
   each garment's by what it is (`dress`, `top`...; `pal.json`'s `colours` say which); its
   picture made grey (its light, against its brightest 5%), its average colour -- times a tint
   the outfit asks for -- the material's base colour, through a Mix node set to Multiply, which
   the exporter writes as glTF's base colour factor. It looks as it did; a colour asked for
   takes the place of the base colour.
6. **Animations**: each a Blender action of poses in the figure's own frame (as with the figure
   from hemera, below), every animation an NLA track of its name.
7. **Exported**: GLB, `export_animation_mode='NLA_TRACKS'`, `export_frame_step` 4, deforming
   bones only, no vertex colours, `export_morph=True`, **`export_morph_normal=False`** (the room
   moves only the positions; the normals were half the faces' bytes); Blender writes shape keys as
   sparse accessors -- only the vertices they move -- which the room reads.

Size, against spatiand-sync's 4 MB: 3.9 MB for one body, three hairstyles, six outfits of one to
three garments, 32 animations and 14 faces -- the pictures 0.8 MB of it, the faces 0.1. Blender's
own start (a cube, a camera, a light) is deleted first: it was exported with the doll.

## Trying one

- **A made-up figure to pose**: `blender -b --python tools/mannequin.py -- /tmp/mannequin.glb`
  (`--idle` for one with an `idle`): a mannequin on a person's skeleton, its own left orange and
  its right blue, a red nose where it faces. In the snapshot room, `"pose": {...}` as above, and
  `turn` (degrees, a test's) to see it from the side.

- **A figure alone, in the snapshot room**: `SPATIAND_FIGURE` takes what `avatar` takes, and
  `time`, which holds every animation that many seconds in, for one picture of one moment:

  ```text
  SPATIAND_BACKEND=snapshot SPATIAND_SNAPSHOT=/tmp/f.png SPATIAND_VIEW=world SPATIAND_HMD=null \
  SPATIAND_INPUT=off SPATIAND_FIGURE='{"state":"talking","path":"/path/figure.glb",
  "options":{"outfit":"dress"},"pose":"hand_on_hip","time":0.5,"size":1.6}' spatiand
  ```

  The log says what the figure has: `avatar: ... read: N parts, ..., N joints in N skins,
  expressions [...], animations [...], options [...]`.
- **SpatiPal with it**: in the snapshot room with its own folder of settings
  (`SPATIAND_APPS_CONFIG`), the pal copied into its `apps/assistant/pals/`, `pal` set in its
  `assistant.json`, SpatiPal run as the room's client (a file named `spatipal`) and asked by
  `SPATIAND_STEPS` (`type:`, `key:28`, `wait:`). Its log says `pal <folder>: N groups of options,
  N animations, N expressions`, and `brain: appearance(outfit, move) -> ...` when its model used
  the tool.
- **Its model's choices, against hemera**: `cargo test -p spatipal -- --ignored --test-threads 1
  appearance` asks the small model, with a made-up pal and SpatiPal's own system message and
  tools, to change its clothes and twirl, and to get into its pyjamas (2026-11-19, `neohorse-1:4b`:
  `appearance({"move":"spin","outfit":"dress"})` and `appearance({"outfit":"pyjamas"})`, 1.5 s).
  The same way: `own_pose` (eleven poses asked for, by name or to be made), `make_move` (moves,
  a pose, an idle, a forget, and what it says while one is made), `body_is_quick` (what it is
  asked to do with its body, and in the room, goes to the quick model, or the quick model is
  already doing it), `does_the_next` (having done one thing with a tool, it does the next with
  one too). `SPATIPAL_TEST_MODEL=Qwen3.8:27b-q3` for the larger model.

## What was checked (2026-11-19)

**Poses of its own**, on deepmagpie:

- The skeleton found by its shape: a person in an A-pose and in a T-pose, the same with every joint
  named `joint`, one turned to face -Z as a VRM 0.x is, a VRM 1.0's humanoid by its names; a chain
  of sixteen and a person one arm short are no body (`cargo test -p spatiand-voice figure`). The
  words read and refused as above.
- The room's turns: an upper arm up, an elbow bent forwards and a wave's arm, straightened; hands
  on hips landing at the waist's sides; an arm across the chest going in front of it; the head's
  look and the chest's lean; a knee bent backwards and not lifted over the hip; a VRM 0.x's own
  left (`cargo test -p spatiand --bin spatiand posing::`).
- Each pose by name, and others (a hand on the heart, a knee raised, looking up and left), on the
  mannequin in the snapshot room, from the front and the side, still and over its `idle`.
- The figure of five looks made on hemera above: `95 joints in 5 skins, 5 bodies to pose` -- the
  room's log, read for that line alone; how a pose looks on it has not been looked at.
- **Its models' poses, against hemera** (`cargo test -p spatipal -- --ignored --test-threads 1
  own_pose`, eight requests; `SPATIPAL_TEST_MODEL` for another model): `Qwen3.8:27b-q3` chose the
  pose by name for each it has (1.3 s, once loaded; 33 s the first) and made its own of the head
  for the sky. `neohorse-1:4b` made all eight in under 3 s -- but made hands on hips of its own
  rather than by its name, and wrote "on heart" for a hand: two of eight answered with how to say
  it. Before the parts were named in words, both models' poses stopped at their first arm (the
  schema's order, above). Not yet: the small model kept to the poses by name, with poses of its own
  the larger model's.

On deepmagpie, in the snapshot room (no glasses):

- The skeletons of Khronos' Fox and CesiumMan posed by their own animations (the Fox's `Walk` at
  two moments, CesiumMan's unnamed one by its name, `animation 0`), and CesiumMan standing at rest
  with no `idle`; the lights over a figure's head now over it, not at its neck (they were drawn at
  the robot's chin height before).
- The figure of five looks made on hemera as above: `5 parts, 5 pictures, 95 joints in 5 skins,
  expressions ["aa"], 32 animations, options [outfit: 5]`; each look shown alone as chosen, a held
  pose (a hand on the hip, arms crossed) and a state's (talking) at size 1.6.
- **SpatiPal with that pal**, typed to in the snapshot room: its window and name the pal's, its
  chat kept as the pal's; "Put on your dress and give me a twirl" routed to the small model (0.4
  s), `appearance(move, outfit)` in 0.6 s, its first words 0.8 s into the turn, the dress on the
  figure and in `look.json`; "It's late. Get comfortable." -- it chose its night clothes itself;
  "Now show me how you sulk." -- arms crossed, held.
- What those turns found, fixed: the small model's routing sent a change of clothes to the larger
  model (a 27B, which then had to load) -- the routing prompt now counts changing its own figure
  as quick, and six such requests went to the quick model three times in three; a reply's stage
  direction ("*leans in, smiling*") would have been read aloud -- the system message now says not
  to write them, and the voice drops a `*...*` of a few words (markdown's doubled asterisks are
  kept as before).
- The new tests: `cargo test -p spatiand --bin spatiand figure:: avatar::` (skins, animations
  sampled step, linear and cubic, options, a plain GLB's targets as expressions, sparse accessors,
  the short way round), `cargo test -p spatiand-voice figure`, `cargo test -p spatipal` (the tool
  from what a figure offers, a call changing the look, a wrong choice answered with the right
  ones, the pal's settings over the wearer's and back, a persona's comments left out, a stage
  direction not said), and the one against hemera above.
- Built for Linux and for Android (the room, `spatiand-android`, and SpatiPal); the room's 518
  tests and SpatiPal's 42 pass.

Not checked: in the glasses; on the Deck, the Mac and the Beam Pro (the same code -- the Mac is
built on littleotter only when the owner says so); a VRM with a skeleton (there is still none on
any machine); a figure of separate clothes over one body; a pal's voice (the snapshot room's
SpatiPal was silent).

## What was checked (2026-11-20)

On deepmagpie, overnight, with the owner away (his words: "make sure you use the time to make this
feature polished.. nothing left over, no bug untested"):

- **Against hemera** (`cargo test -p spatipal -- --ignored --test-threads 1 ...`), the small model
  and, where it applies, `Qwen3.8:27b-q3`: `own_pose` 11 of 11 for both (the small model then 16
  runs in a row after a single miss); `make_move` 7 of 7 for both, and what each then says while
  it is made, in character and not claiming it done; `body_is_quick` 16 requests three times
  each, each kept with the quick model or already done by it; `does_the_next` 5 follow-ups three
  times each, each done with a tool; `appearance`.
- **In the snapshot room, SpatiPal typed to**, with a made-up pal on a mannequin: a pose made from
  words and held (44 s, hemera loading first); "forget it" while a move was made (made in 40.7 s,
  not kept, not done); another pose taken while one was made (the late one kept, not taken);
  "Forget that ... a cartwheel instead" (both made at once, the pose not taken, the cartwheel
  done); five turns of windows and poses, a pose asked of `make_move` by a name it had done at
  once; hemera refusing the first ask (a stand-in) -- asked again, and done.
- **A doll from MakeHuman**, as above: each look, each hairstyle given a colour (red, red, blonde),
  a sweater green, a dress blue with black shoes, a two-piece swimsuit white, in the snapshot
  room; its faces in Blender's close-ups (each of the fourteen) and the room's own (talking, a
  held wink, surprise); a made-up pal on it asked "Dye your hair red", "Give me a wink", "Put on
  your swimsuit, and make it white" -- each done by `appearance`. Its figure checked by its
  numbers and dressed only.
- `cargo test -p spatipal` (55), `-p spatiand-voice`, `-p spatiand` (534); clippy; the Beam Pro's
  `cargo ndk check` of `spatiand-android`, SpatiPal and spatiand-voice.

Not checked: in the glasses; a pal's voice saying what it does while a move is made (the
snapshot room's SpatiPal is silent); feet planted in a move hemera makes (its figures slide when
they turn or travel); fingers (not in hemera's motions).
