# Dictation and the assistant

The plan for Spatiand's two AI integrations, as agreed on 2026-10-28: **dictation** into whatever
has focus, and an **assistant** with an avatar in the room. Like `spatiand-apps.md`, it is written
so that a session which was not part of the discussion can build from it, and it ends with what
has been built.

Both lean on **hemera**, the model router on deepmagpie (`http://deepmagpie:12434`), whose own
hand-off notes are in the hemera repository: `docs/integrate-spatiand.md`, `docs/media-api.md`.
All speech and all models run there; Spatiand's devices capture and play audio, gather context
and act.

## Decided

| | |
|---|---|
| Dictation's key | A **mic key on the on-screen keyboard's top strip**, beside the sound toggle. Click to start, click to stop. |
| Languages | **No switching.** The owner speaks English and Indonesian, often in one sentence. A setting lists the languages spoken (English and Indonesian to start, any of Whisper's); with one listed it is given to Whisper, with several Whisper detects — which kept mixed sentences as spoken in a test, where forcing English *translated* the Indonesian. |
| Context | What has focus steers Whisper's spelling: a terminal's recent commands and file names, a page's words, a folder's names, a Mac field's text. Spatiand's own apps say it themselves; Mac windows through Accessibility; anything else by its title. |
| Terminals | Spoken symbols become symbols, then a quick pass of the small model; **never presses Enter**. |
| The assistant | A setting, off by default. **Its own process** (a first-party app), with its chat window; Spatiand draws its avatar and plays its voice from where the avatar stands. |
| Avatar | **VRM** (VRoid and the like: humanoid, expressions, spring bones) and **plain glTF/GLB** (robots, anything). Moods, animations, outfits, a voice of its own. Light: a small skinned mesh, nothing like a game character. In the corner **opposite the pinned windows**; it can be hidden and stay active. |
| Hearing | Three states: **listening** (continuous), **not listening**, **muted** (answers in text only). In continuous listening, speech counts when it **starts with the character's name**, or comes **within a few seconds of its answer** (a follow-up). |
| Models | Each role is a **setting**, picked from hemera's list: the assistant's model, dictation clean-up's, and a **larger one for heavy work**. Default small model: **`neohorse-1:4b`** (below). |
| Handing over | **The small model decides, quickly and by itself, to hand a request to the larger one** (changed 2026-10-28 from "offered"). While the larger one thinks, the avatar shows it is thinking; a setting has **the thinking read aloud** as it comes. |
| Working windows | The assistant does what the wearer does with a window: **reads it** (its description, or a picture for the model to look at), **scrolls, clicks and right-clicks at a point, drags, types, presses keys**. Through the room's own pointer and keyboard, so the same for Spatiand's apps, a Mac's windows and a host's. |
| Chats and reminders | **Shared across devices**: kept in the folder Spatiand already syncs between linked machines (`~/.config/spatiand/apps`). A setting decides whether a conversation continues the last chat or starts a new one. |
| Order | Dictation first, then the assistant: voice and chat, then its actions, then the avatar. |

## The small model: `neohorse-1:4b`

The owner's choice: **huihui-ai/Huihui-NeoHorse-1-4B-abliterated-GGUF**, `Q4_K` (2.78 GB) with
`mmproj-model-f16.gguf` (0.67 GB), a fine-tune of Qwen3.5-4B whose MTP head and vision tower
come from Qwen's own. On deepmagpie in `/media/shared/AIModels/LLM/gguf/NeoHorse-1-4B/`, in
hemera as `neohorse-1:4b`: llama.cpp latest, Vulkan, `-c 65536 --spec-type draft-mtp
--spec-draft-n-max 3`, kept loaded (`keepAliveSeconds: -1`).

Measured 2026-10-28 through hemera, thinking off (`chat_template_kwargs: {enable_thinking: false}`):

| Model | Cold load | First token | Speed | Reminder tool call | Picture of a failed build |
|---|---|---|---|---|---|
| `neohorse-1:4b` | 3 s | 0.17 s | ~70/s | right | right, in Indonesian, 1.0–1.5 s |
| `ornith-1.5:9b` | 10 s | 0.3 s | ~40/s | right | — |
| `Ornith-1.5:35b` | 24 s | 0.11–0.24 s | 43–76/s | right, with a spoken line | no vision |
| `gemma4:e4b` (F16) | 24 s | 0.25 s | 15–20/s | time garbled, plan narrated | — |

Gemma 4 E2B was weighed: much weaker at tool use by its own card (Tau2 24.5% against Qwen3.5-4B's
79.9%), but it hears audio itself and runs on a phone — the candidate for an **offline** model on
the device one day, not for the assistant's everyday work.

**Hemera keeps several models loaded for good** (`keepAliveSeconds: -1`): a test that loads one
leaves it holding memory until it is stopped (`POST /hemera/api/models/stop {"name": ...}`).

## Dictation

### Hearing

- The microphone the compositor already opens for remote hosts (`pw-record` on the Deck, Core
  Audio and AAudio through `spatiand_audio::server::Capture`), at 16 kHz mono.
- **Phrases are cut on the device** by a pause detector (loudness over the room's own noise), and
  each phrase is one `POST /v1/audio/transcriptions` (`whisper-large-v3-turbo`, temperature 0):
  0.4–1.5 s measured. **Not** the Realtime socket hemera's note suggests: while any Realtime socket
  is open hemera holds every background job, and dictation would hold them for as long as
  Spatiand runs. Drafts while a long phrase is still being spoken can come later from re-reading
  the growing phrase.

### The prompt

Whisper reads the last ~150 words of `prompt` and leans towards them; the most relevant goes last.
In order: the focused thing's vocabulary (names, commands, paths), then its recent text, then the
owner's **own last few dictated phrases** (so it follows the way they mix the two languages).
Measured: a mixed example fixed "schedule-nya", "linker-nya"; context fixed "left pad". A coined
word ("Spatiand" heard as "sepatian") is not fixed by context — that needs the word list.

### After Whisper

1. **Word list**: words from the context plus the owner's own list; a near miss in sound and
   spelling is replaced ("sepatian" → "Spatiand").
2. **Terminal or code**: spoken symbols (`slash` `/`, `dot` `.`, `dash` `-`, `pipe` `|`, `tilde`
   `~`, letters spelled out joined, numbers as digits; Indonesian words for them too), then the
   small model with the terminal's context. Shown before it is typed; never Enter.
3. Typed into the focused window as keystrokes (every window, Mac and remote ones included, is a
   surface with the compositor's keyboard). Text that is not on the keyboard is a later problem
   (text-input, or a Mac window's Unicode events).

### Context from what has focus

`spatiand_xr_surface_v1.describe` (new): the compositor asks a window to describe itself into a
file descriptor, as JSON — `kind` (terminal, web, files, text…), `title`, `text` (what it shows),
`vocabulary`, and per kind more (a terminal's directory and recent commands). The same answer
serves the assistant when it reads a window. Spatiand's own apps answer through the SDK; Mac
windows from the Swift side through Accessibility; everything else by its title and application.

## The assistant

### Parts

- **SpatiPal** (working name), a first-party app: hemera client, conversation loop with tools,
  chat history and reminders, its chat window (with a settings page), and its voice (TTS played
  through `spatiand_media::sound::Stream` under a sound key the room places at the avatar).
- **`spatiand_assist_v1`** (new), offered only to Spatiand's own executables: the list of apps and
  windows, a window's description and picture, the room's actions (open, focus, hide, pin, close,
  recentre, settings), the microphone as a stream (one capture for the whole session: Android
  will not give two processes the microphone at once), and the avatar (which model, which
  animation, expression, mouth).
- **The avatar renderer** in the compositor: glTF/VRM, skinning, morph targets, animations,
  spring bones; toon-ish shading; placed opposite `pip_corner`.

### Its actions (tools)

List apps; open an app (local or a host's); list windows; focus, hide, show, pin, close a window;
read a window (its description, or a picture for the model to look at, with a grid so it can say
where); work a window like a hand: scroll, click, right-click, double-click, drag at a point in
it, type text, press keys and shortcuts; recentre; change a setting (look, pinned windows' corner
and size, spatial sound, keyboard click, environment, sound devices); reminders (set, list,
cancel; spoken by the avatar when due, with a card); hand over to the larger model. Closing
windows, pressing Enter in a terminal and anything that sends or deletes ask first.

### Hearing and speaking

- The microphone from the compositor; pauses cut phrases as in dictation; each phrase transcribed.
  The name at the start, or a follow-up within ~8 s of an answer, makes it the assistant's.
- **It does not hear itself**: the microphone is ignored while it speaks (and a moment after).
- Speech: `voxcpm2` streamed (`stream_format: sse`, 24 kHz PCM), sentence by sentence as the
  model writes; a voice from hemera's library or described in words; the mouth from the sound.

## Built so far

2026-10-28:

- **`crates/spatiand-voice`**: phrases cut on the device, hemera's transcription and chat, the
  prompt from the focus, the word list, a terminal's spoken symbols and the clean-up pass.
  `cargo test -p spatiand-voice --test hemera -- --ignored` runs it against hemera end to end.
- **The mic key** in the right alt's place (room keyboard and the Deck's screen), the microphone
  (`listen.rs`), typing as keystrokes, a terminal's text on the notice card (`dictate.rs`). The
  Mac's menu asks for the microphone. `SPATIAND_LISTEN_WAV` makes a WAV file the microphone; the
  snapshot room's `dictate:` step uses it.
- **`describe`** (`spatiand_xr_surface_v1`, version 9) and `Event::Describe` / `App::describe` in
  the SDK; SpatiQuill answers with its screen, the commands at its prompts, its folder and the
  names in it, SpatiShelf with its folder and names.
- Checked on the Mac in the snapshot room: a spoken `ls /etc/httpd` typed into SpatiQuill after
  pressing the card; SpatiQuill's description reached dictation (14 names, 2 commands).

Later the same day:

- **Descriptions** from SpatiMesh (a script in the page: title, address, headings, name-like words,
  the field being typed in, last) and from Mac windows (`MacDescribe.swift`, Accessibility, off
  the main thread, through `SP_DESCRIBE`).
- **SpatiPal** (`crates/spatipal`), the assistant, with its chat window and settings (dictation's
  among them). Chats, reminders and settings in the shared folder (`assistant/`,
  `assistant.json`, `voice.json`). The small model decides in one word whether the larger one
  answers (`brain.rs`, `wants_larger`); a `hand_over` tool remains as well. Tools in `tools.rs`.
  **Closing a window and Enter in a terminal are held by the app until the wearer says yes**:
  the model, given a `confirmed` flag, was seen to set it itself and press Enter. Speech a
  sentence at a time, paced into the room (`voice.rs`); never code. Ears (`ears.rs`): the room's
  microphone, name or follow-up.
- **`spatiand_assist_v1`** (`crates/spatiand/src/assist.rs`): offered to all, answered for the one
  process given the session's key. Windows, apps, describe, picture, click/scroll/drag/type/key,
  the room's controls, notice, the figure, the microphone. A request with no window acts on the
  focused one -- never the assistant's own. Started with the room when switched on, through the
  shell (`--background`).
- **The figure** (`avatar.rs`): a robot drawn from shapes by default; the wearer's VRM/GLB from
  `assistant/characters/` (`figure.rs`); per-eye pictures with depth, laid in as quads; moods from
  SpatiPal. Its voice is placed at it (`audio::VOICE`; on the Deck through the sink its launch
  names). A GLB's vertex colours tint it; a VRM's are left alone, as MToon leaves them. Not
  read: KTX2/Basis pictures (drawn white) and glass (`KHR_materials_transmission`, drawn
  opaque) -- VRM figures, painted for MToon in PNG or JPEG, use neither. Changing figures frees
  the last one's GPU memory.
- Checked in the Mac's snapshot room: reading a terminal, typing into it, the held Enter released
  by "ya", the hand-over to `Qwen3.8:27b-q3` with its thinking shown, the robot in view, a wake
  by name from a phrase played from a file (`SPATIAND_LISTEN_WAV`). Figures: a VRM made in the
  test, and the Khronos glTF samples the OpenXR conformance suite ships (on the Deck, in
  `~/OpenXR-CTS/src/conformance/conformance_test/gltf_examples/`): the vertex-colour test
  passes, alpha cut-offs fall at their marks, an 11 MB file reads.

Not yet checked: any of it in the glasses or with a real voice; the assistant running on the
Deck or the Beam Pro (both build; the Deck's tests pass); a real VRM file -- there is none on any
of the machines.

### Quick, and seen to be working (2026-10-29)

The owner's first real use: answers came 10-20 s after asking, a follow-up without the name was
dropped without a word, and nothing showed what Pixel was doing. Where the time went, from the
log and measured against hemera:

- **Nothing was loaded.** Whisper, the small model and the voice were each started by the first
  request that wanted them -- 10.5 s in the small model alone. Earlier tests had stopped them.
- **A decision before every answer**: the small model was asked "quick or larger" first, a round
  trip of 0.35-2.2 s from the Mac (Wi-Fi to deepmagpie over Tailscale: 7-330 ms a round trip).
- **A system message that changed every minute** (the time and the windows were in it), so the
  server read ~500 tokens and the whole chat again every turn: 1.8-2.3 s to the first word.
- **Whisper after the pause**: 0.8 s of quiet to be sure, then ~0.6 s of Whisper (2-3 s when
  deepmagpie's GPU is busy with something else).
- **The follow-up window was measured to when Whisper's text came back**, not to when the
  wearer began: a follow-up begun three seconds after the answer and spoken for four arrived
  after the 8 s had passed.

What was done:

- `warm.rs`: on starting to listen, hemera is asked to start Whisper and the voice
  (`POST /hemera/api/models/load`), and the small model reads the start of every request (its
  tools and system message) once; the larger model too, but only if something else started it.
  Looked at again, at most every 3 minutes, when someone begins speaking.
- The system message is the same every turn; the room as it is goes with the wearer's words as
  a note in brackets. The chat given starts in steps of 8, so its start rarely moves.
- The decision is asked **beside** the small model's answer; the answer is held (unshown,
  unsaid) until it comes, then given or dropped for the larger model. The decision's prompt
  says the quick model knows the time and date, with a few examples: after an answer that gave
  the time, "what day is it tomorrow?" had gone to the larger model; with them 24 of 25 test
  requests went where they should.
- **Drafts** (`phrases.rs`, `Cut::Draft`): at 0.3 s of quiet the phrase so far goes to Whisper;
  when the pause proves to be the end, the draft's text is the phrase's. Whisper writes on
  helper threads, one job at a time, the latest draft waiting; the ears' state is never late.
- **A follow-up counts from when it began**: within 8 s of the last of an answer finished, the
  voice going quiet, or a phrase meant for it.
- **Whisper's prompt uses the name as it is used** ("Pixel, buka browser. ... Pixel, can you
  check the log?"): given only "Pixel." it left the name out of "Pixel, write a short Python
  function..." as the ears cut it; now 11 of 11 test phrases, none given a name they lacked.
- The log says what each step took: `ears: ... written N s after it ended`, `brain: first words
  ... N s into the turn`, `voice: first sound N s after the wearer stopped speaking`.

**The figure's states** (`state` beside the old `mood`, which a room without them still reads):
`idle` (dozing: not listening), `listening` (for its name; its antenna glows with the room's
sound -- `assist::heard_level`, from the microphone it is fed), `checking` (out of a
conversation, a phrase is being written to see if it begins with its name: the antenna blinks
amber, nothing else stirs), `attentive` (in a conversation: leaning in, the lights at its ears
lit -- no name needed), `hearing` (a hand cupped to its ear, the voice drawn in as dots),
`transcribing` (eyes a line running along what was said, three amber dots over its head),
`thinking` (the small model: a hand to its chin, a blue spark circling its head), `pondering`
(the larger model: arms folded, three slow dots), `working` (a tool: it types), `talking` (only
while its voice is heard, no longer while speech is still being made), `happy` (once an answer
is heard out). Poses ease from one to the next. A VRM figure shows them with its expressions
and the antenna's light and dots over its head. `SPATIAND_FIGURE='{"state":"hearing",
"level":0.7}'` shows a state in the snapshot room with no assistant.

Checked in the Mac's snapshot room against hemera, with phrases spoken by macOS's voices into
`SPATIAND_LISTEN_WAV`: from the end of speech to the first sound handed to the room 1.6 s for the
small model, 5.2 s for the larger (primed); the follow-up without the name answered; a sentence
said 30 s later ignored; every state photographed. The new SpatiPal was put into the installed
app alone (it sends the old moods too, so the running room shows what it knew before).

Not yet checked: the new gestures in the glasses (they are the room's: Spatiand must be rebuilt
and restarted); a real voice in a real room -- its own voice heard back, a pause in the middle of
an Indonesian sentence; the Deck and the Beam Pro; a VRM figure's states (still no VRM file).

### What the owner's next test found (2026-10-30)

- **A silent room asked "can you check the log?" over and over.** My Whisper prompt of the day
  before ended "Pixel, can you check the log?"; Whisper given noise reads back the end of its
  prompt, and with the name in it that was a request, and each answer kept the conversation open.
  And the cutter fed it 25 s "phrases" of silence: its floor was the quietest moment, so the
  room's own wavering kept them going. Now: the name is not last in the prompt; the floor is the
  quiet end of the last 3 s; **a voice's loudness moves 3-6 dB from window to window and a hum's
  under 0.5 dB**, so what does not move is let go within 3 s; and what Whisper writes is let go when
  it barely believes it (mean word probability under 0.4), when there are far too few letters
  for the time, or when one sentence repeats. A workaround for a real voice detector (Silero or
  the like), which is the proper answer if this lets noise through. Checked on generated noise;
  **not on the owner's microphone** -- `ears: let go, ...` in the log says what it drops.
- **Speech stuttered while a model wrote**: they share a GPU. voxcpm2 makes speech at 2.5x alone,
  0.7x beside the small model, 0.6x beside the larger. A sentence now waits up to 0.6 s for the
  model to finish, and is played only once the rest will come in time (`voice::enough`, at most 4 s
  held; `voice: ran dry` in the log if it still does). Costs up to about 1 s before the first sound.
- **It read windows that were not in the room**: a Mac's windows arrive hidden. The note given
  with each message now separates those on show from the hidden, the system message says to
  leave hidden ones alone unless named, `list_windows` puts them last, and a request naming no
  window never takes a hidden one.
- **The figure sat a third of the way out** from the middle: the room moved it in if it poked out,
  never out to the edge, and measured a box square in device coordinates, wider than the figure
  on a 16:9 view. Now it is placed at the corner by its true extent, either way.
- **No way to shut it down**: closing its window left it listening. Closing now quits it unless it
  is switched on (`on`); the `dismiss` tool ("you may go") has it say goodbye and go -- or, switched
  on, stay deaf and unseen for its reminders until opened again.

## Dictation with no room

The same phrases and clean-up, with a switch instead of the mic key: while it is on the microphone
is open and what is said is typed into the front window (`dictate::on_a_desktop`).

- **The Mac's menu bar**: "Dictation: type what I say into the front window", remembered across
  launches (`MacDictation.swift`). The icon becomes a microphone while listening. Typed as Unicode
  key events, so any layout; needs the Microphone and Accessibility permissions. It gives way while
  in the glasses, where the room holds the keyboard and has its own mic key.
- **The Deck's desktop**: the Dictation entry under Spatiand in Plasma's menu (`desktop_dictation.rs`).
  It does not start the room. Typed by a uinput keyboard, so only what the US layout has; the window
  in front comes from a KWin script read through `dbus-monitor`, because KWin scripts have no other
  way out (a D-Bus name of its own would be the proper way, and a D-Bus library for it).

A terminal's words wait in both: on a card at the Mac, on a notification on the Deck. Only the
window's name is known on the Deck; on the Mac what is in it is read by Accessibility. Everything
said is typed while it is on, so a conversation in the room is typed too. Only phrases go to hemera,
cut on the device by pause detection.
