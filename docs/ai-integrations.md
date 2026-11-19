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
| Dictation's key | A **mic key on the on-screen keyboard's top strip**, at its right end, opposite the sound toggle (since 2026-11-09). Click to start, click to stop. |
| Languages | **No switching.** The owner speaks English and Indonesian, often in one sentence. A setting lists the languages spoken (English and Indonesian to start, any of Whisper's); with one listed it is given to Whisper, with several Whisper detects — which kept mixed sentences as spoken in a test, where forcing English *translated* the Indonesian. |
| Context | What has focus steers Whisper's spelling: a terminal's recent commands and file names, a page's words, a folder's names, a Mac field's text. Spatiand's own apps say it themselves; Mac windows through Accessibility; anything else by its title. |
| Terminals | Spoken symbols become symbols, then a quick pass of the small model; **never presses Enter**. |
| The assistant | A setting, off by default. **Its own process** (a first-party app), with its chat window; Spatiand draws its avatar and plays its voice from where the avatar stands. |
| Avatar | **VRM** (VRoid and the like: humanoid, expressions, spring bones) and **plain glTF/GLB** (robots, anything). Moods, animations, outfits, a voice of its own. Light: a small skinned mesh, nothing like a game character. In the corner **opposite the pinned windows**; it can be hidden and stay active. |
| Hearing | Three states: **listening** (continuous), **not listening**, **muted** (answers in text only). In continuous listening, speech counts when it **starts with the character's name**, or comes **within a few seconds of its answer** (a follow-up). |
| Models | Each role is a **setting**, picked from hemera's list: the assistant's model, dictation clean-up's, and a **larger one for heavy work**. Default small model: **`neohorse-1:4b`** (below). Since 2026-11-19 they are the room's, on the **AI card** of its settings, with hemera's address and dictation's languages and words: more than one app uses them (below, "The AI settings"). |
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
- **It hears the wearer over itself, not itself** (since 2026-11-15): its own voice is taken back
  out of the microphone, and speaking over it stops it -- below, "Talking over it".
- Speech: `voxcpm2` streamed (`stream_format: sse`, 24 kHz PCM), sentence by sentence as the
  model writes; a voice from hemera's library, or one described in words and **made into a library
  voice once** so that every sentence is the same voice -- below; the mouth from the sound.

## Built so far

2026-10-28:

- **`crates/spatiand-voice`**: phrases cut on the device, hemera's transcription and chat, the
  prompt from the focus, the word list, a terminal's spoken symbols and the clean-up pass.
  `cargo test -p spatiand-voice --test hemera -- --ignored` runs it against hemera end to end.
- **The mic key** (room keyboard and the Deck's screen) -- in the right alt's place until
  2026-11-09, then at the top strip's right end at the owner's asking, opposite the sound toggle,
  its microphone drawn by the room's text renderer (`spatiand_render::symbols`) because the Beam
  Pro had no font that could draw U+1F3A4 and left the key blank -- the microphone
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
  among them, until 2026-11-19: they are the room's AI card's now). Chats, reminders and settings
  in the shared folder (`assistant/`, `assistant.json`, `voice.json`, the last two's shared part
  carried into `ai.json` since). The small model decides in one word whether the larger one
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

### Talking over it, one voice, and what the wearer sees (2026-11-15)

The owner, testing on the Mac: "let user interrupt with voice if the spatipal is speaking...
sometimes I just want it to be shut up"; "long response, I found the SpatiPal is changing voices";
"always focus on what the user see"; and asked to describe what he saw in SpatiMesh, zoomed onto
one picture, it read the whole page. Then, on the last two: "don't change the browser... make
something like screen capture. The priority is what the user see... the hidden things -- the page
or the other application -- become additional information if they are related to the question."

**Talking over it** (`spatiand_voice::echo`, `crates/spatipal/src/ears.rs`). The microphone stays
open while it speaks. **One design for every platform, in SpatiPal, against the exact samples it
plays**: each 50 ms slice handed to the room is kept with the moment it is to be heard
(`echo::Played`), and the microphone is put on the same timeline by counting its samples from the
earliest arrival (`MicClock`, steady, no jumps for a late read). Then:

1. the delay between the two -- the room, the device, the air: 50 ms to a second, different on
   every machine -- is found by matching their loudness over the last four seconds, and believed
   when two matches half a second apart agree (`DelayFinder`);
2. a partitioned-block frequency-domain adaptive filter (overlap-save, constrained gradient, 192
   ms after the delay; the family of Speex's and WebRTC's linear stage) learns the path -- the
   room's own spatial filters, the speakers, the frame -- and takes it out (`Canceller`); it learns
   only while the microphone is not well above its voice, and starts again if it ever adds more
   than it takes;
3. a gate judges each 16 ms in 16 bands from 100 Hz to 3.7 kHz: what its voice leaves in each band
   for what was played there is learnt as it speaks (the loud end, the 90th of a hundred), and the
   wearer is heard where there is three times that after the canceller **and** twice its voice's
   loud end in the microphone as it came, over the room's floor, with a pitch (`Gate`). The second
   condition is what keeps it from stopping itself: on one clock the canceller took most of its
   voice away, but its slips at the start of a sentence, judged alone, were heard as the wearer
   four times in a minute and a half of real speech;
4. six such blocks of the last sixteen, three running, while it speaks or has something to say:
   **it stops** (`BargeIn`) -- the sentence being made is cut off at hemera, nothing waiting is
   said, and what the room has not played yet is dropped (`Voice::interrupt`, `Stream::clear`);
   the model writing the rest is stopped; what the wearer says is cut from where they began (the
   last 0.6 s, kept) and is the next request. **Only "stop", "quiet", "shh", "diam", "cukup",
   "berhenti" and the like** (`ears::is_hush`): it is quiet, and nothing is asked. What is heard
   after it stopped is checked against what it had been saying (`ears::not_its_own`): mostly its
   own words (three in four of the longer ones) and it is not asked at all; its own words at the
   start ("...a woman named Mera. Wait, what is the capital of France?") are left out -- a
   safeguard behind the gate.

Until the delay is found, what it played over the whole last second stands for what may be heard,
the loudest of it, and only once it has played for longer than a second: careful while its echo is
still being looked for, and the way the wearer is heard at all when its voice never reaches the
microphone (earphones), where there is no echo to find.

Why not the platforms' own cancellers: each takes as its reference what *it* plays. macOS's
voice-processing unit cancels only what goes out through that unit; Android's
`VOICE_COMMUNICATION` source and `AcousticEchoCanceler` the call path's output, which is rarely the
glasses' USB sound; PipeWire's echo-cancel module the sink routed through it. The voice reaches the
glasses through the room's own HRTF mixer on every platform, so none of them sees it without
moving the room's whole sound through voice processing -- every film and game with it (ducked,
narrowed, gain-ridden). **The proper alternative**, if the in-process one is not enough on a
device: the room plays and records through the platform's voice path only while the assistant
listens -- the Mac's output unit becoming a VoiceProcessingIO unit with its input, Android's capture
the `VoiceCommunication` input preset with the room's output in communication mode, PipeWire's
`echo-cancel` loaded by the room for its own streams -- one platform piece each, under the same
`Echo`. Not built: none of it could be heard here.

Measured on deepmagpie, nobody at a microphone: the snapshot room with a WAV as its microphone
and SpatiPal in it, `SPATIPAL_TEST_ECHO=<gain>,<ms>,<ppm>,<distortion>` putting what it plays back
into the microphone through a made-up room (reflections, a tail, two clocks apart, small speakers'
squash); its voice is voxcpm2's own (peaks -1 dBFS, speech -19 dBFS), the wearer's a man's voice
from hemera's library (-24 dBFS). Gain 0.4 is its voice at the microphone as loud as the wearer's
(normal volume: speakers at the temples, the mouth a little further); 1.0 is 6-8 dB louder.

| | normal (0.4) | loud (1.0, 2.0) |
|---|---|---|
| Long answers, nobody speaking over them | **no stop** (21 sentences) | no stop in 4 runs (14, 17, 20 and 29 sentences); **a false stop in 2 runs of 6**, about twice in five minutes of loud speech, both with its voice 6-8 dB over the wearer's, heavily squashed, on clocks 40 ppm apart |
| "Wait. What is the capital of France?" 20 s into an answer | stopped twice, the question heard whole and answered ("Paris."), the story cut where it was | stopped; heard as "What is the capital of France?" (the "Wait" before it was sure), answered |
| "Stop." alone | stopped, quiet, nothing asked | -- |
| A question over its answer, none of its voice in the microphone (earphones) | stopped, the question heard and answered | -- |

After a false stop Whisper writes its own words; those are now checked against what it was saying
(`ears::not_its_own`, tested on the transcript the rig produced) and not asked -- the earlier rig run
had sent them to the model as a request.

The same pieces against 95 s of its real recorded voice, offline (`echo::tests::on_real_speech`,
`ECHO_SPEECH=...`): **no stop in 12 conditions** (gain 0.4, 1.0, 2.0; one clock and 40 and 150 ppm
apart; clean to squashed) -- 19 minutes of its voice; a sentence spoken over it stopped it **272 ms**
after it began in 8 of 8 at normal volume, and in 6 of 8 when it was 6-8 dB louder (not in its
first seconds, while the delay and its voice are still being learnt); a lone "Stop." in 5 of 8 at
normal volume and 1 of 8 louder. So: **speak up, or say a few words**, over a loud answer. In the
first two to three seconds of the first answer after it starts, nothing stops it: the delay and
what its voice leaves are not known yet. The canceller took 6-10 dB away on one clock and 0-4 dB
on two; the gate does not lean on it.

Not verified: any of it with a real microphone and real speakers -- the glasses on a head, the
Mac's or the Beam Pro's capture (they are unchanged: the room's microphone as before, one design in
SpatiPal on all three); a moving head (the path changes; it relearns as it goes). The log says what
it does: `echo: its voice comes back N ms after it is played`, `ears: while it speaks: ...` every 10
s, `ears: the wearer began speaking over it: it stops (...)`.

**One voice** (`spatiand_voice::speech::made_voice`, `crates/spatipal/src/voice.rs`). The cause:
with no library voice chosen (the default), SpatiPal gave the description with every sentence, and
hemera hands a description to VoxCPM2 in front of the text (`SpeechApi.java`: `"(" + instructions +
")" + input`) -- **the model designs a voice for each request, from that request's text** (its seed
is fixed, so the same sentence always gets the same voice, and different sentences different ones).
Spoken a sentence at a time, an answer was a choir: five sentences of one answer had median pitches
from 139 to 238 Hz, and hemera's own speaker model (`diarized_json`) put the Indonesian one down as
somebody else. Now the description is said once, ten seconds of it in both languages, and kept in
hemera's library as a voice named from the model and the description (`spatipal-<hash>`, consent
"synthetic..."); every sentence is said in that voice (`voice: sentence N in the voice spatipal-...`
in the log), the same in every answer and on every device, made again only if the description
changes; made when SpatiPal starts, so the first answer does not wait. Checked: the same five
sentences, one speaker by the diarization; a 14-sentence answer said both ways, same words: pitch
from sentence to sentence 1.8 semitones (sd) against 2.6, timbre 2.4 against 3.1 (mean log-mel
distance from the set's own average) -- and no sentence in another voice. What still moves is
VoxCPM2's own prosody with a reference. **The proper alternative**, for a voice closer still: hemera
offering a library voice as VoxCPM2's continuation prompt (`prompt_audio` and its text, which
audio.cpp has) rather than only a reference; or a real recording, with its owner's consent, chosen
as the voice in SpatiPal's settings (that path was always one voice).

**What the wearer sees** (`crates/spatiand/src/assist.rs` `seen`, `crates/spatipal/src/brain.rs`).
**The room's picture is the primary context; the application's text is secondary.** The room knows
where the wearer looks and what each window looks like as it is drawn, for every kind of window on
every platform (Spatiand's apps, SpatiMesh, Mac windows, a host's): `{"do": "seen"}` answers with
the window looked at -- the one in the middle of the view, else the focused one in view, else the
one filling most of it, never SpatiPal's own -- its picture **cropped to the part inside the field
of view, at the zoom it is drawn at** (a page zoomed onto one picture is that picture: the browser
draws its zoom), which part that is, how much of the view it fills, and the other windows in view;
`"view": true` adds the left eye's picture of the whole view, read back from the frame as the
screenshot is. The part in view is found by projecting a grid of points of the window through the
left eye; what fills the view, and which window is in front, by a grid of rays from the eye. A
request with no window named now acts on the window looked at (`target`).

SpatiPal, every turn: the note before the wearer's words says which window they are looking at and
how much of it is in view ("the top middle of it, 25% of the window"), which others are also in view
and which are out of view. When the request is about what is seen (`brain::about_the_view`: "this",
"see", "describe", "screen", "picture", "ini", "lihat", "gambar", "layar", ...), the picture of what
they see goes with it -- or of the whole view, for "everything", "these windows", "sekeliling". The
window's own text (`describe`: the whole page, a terminal's scroll-back) is given only when the
request reaches past the screen (`brain::past_the_view`: "the rest", "whole", "summarize", "below",
"the other window", "sisanya", "seluruh", ...), marked to the model as not on screen. The system
message: what they see comes first; answer from the picture; text marked not on screen only for what
the request needs beyond the view; other windows only when asked for, and then results may go there.
`look_at_window` is now the room's `seen` (the part in view); `read_window` says its text may be
more than is on screen. Both models take pictures: `neohorse-1:4b` and `Qwen3.8:27b-q3` are each
loaded in hemera with their `mmproj` (`/opt/hemera/hemera.json`).

No browser change (the owner's decision). `describe` keeps its meaning, now written down in the
protocol's notes: the client's own account, which may be more than is on show. The assistant
protocol's `ask` notes say what `seen` answers; no request was added, so no client's copy of the
XML needed changing (Chromium's is brought up to date with its next monthly build).

Checked, 2026-11-15:

- **The cause, on the Deck with the real SpatiMesh** (its test room, `tools/chromium-test`, the
  installed build): a test page, "Village news" -- paragraphs, and three drawn pictures under
  "The harbour" (a sailboat), "The park" (a tree) and "Housing" (a house). `describe` answered
  with the whole page's words, 1103 characters, the sections below the screen included, and nothing
  of the pictures; pinched in to fill the window with the sailboat (`pinch:3`), it answered with
  the same whole page. Before, that was what SpatiPal read.
- **Now, end to end in the snapshot room on deepmagpie** (where the browser cannot run: a stand-in
  window showing the Deck's picture of the page and answering `describe` with the Deck's answer;
  SpatiPal asked by a WAV, its chat window open in front of the page): "describe what I see" --
  zoomed: *"a sailboat with a brown hull and white sails, sailing on dark blue water under a light
  blue sky with a yellow circle (the sun)... mostly blank white space to the right"*; not zoomed:
  the first two sections and *"A 'The park' heading at the bottom"* -- nothing of what is below
  the screen. Then "what does the rest of the page say?", spoken over the first answer: it stopped,
  was given the page's own text marked not on screen, and answered *"Below what's on screen, the
  page continues: under 'The park', volunteers will paint the benches... a 'Housing' section..."*.
  The pictures it was given are kept by `SPATIPAL_TEST_RECORD` (the window as shown, its own chat
  window looked through). "Describe what I see" goes to the quick model, which sees: 1-2 s.
- The geometry -- the part of a window in view, the window in front, turned away -- by unit tests
  in `assist.rs`.

Not verified: the crop to the part in view on a real head (the snapshot room places windows where
its head looks, so its windows were always whole in view); the view picture (`"view": true`, read
back from the frame) on any glasses -- the snapshot room draws no frame to read; the Mac build (the
room's `seen` and the read-back are in the Beam Pro's and the Mac's shared backend, which checks
for Android here); Mac windows and a host's in the room (the same texture path, not run).

### The AI settings, in the room's settings (2026-11-19)

The owner: "I do not see new setting/card for AI, and spatipal still hold the setting hemera,
dictation, model selection *need to put in general for model selection, as it may impact other app
too"; "move shared setting from spatipal to this card, like the hemera url, but keep setting
specific spatipal in spatipal, like model, voice, etc" -- the models then to the general card too.
The same day, for choosing text in pictures (built later): "2 language... primary and secondary...
the selection in primary, translate to secondary. other, translate to primary" and "read automatic".

**The AI card** (`crates/spatiand-shell/src/ai.rs`, behind it `crates/spatiand/src/ai.rs`), one
row in the room's settings, the same on the Deck, the Beam Pro and the Mac (the menu code is
shared; each loop only opens it, hands it what the wearer changed and ticks it):

| Row | What it is | Chosen |
|---|---|---|
| hemera's address | the server every model and voice is on | typed, as a computer's address is (any keyboard, the room's, the phone's, dictation); `http://` added when left out |
| *Models*: Everyday model, Larger model, Terminal clean-up model | named for what they do | from hemera's text models (`/v1/models`, `hemera.kind` `text`, as SpatiPal offered them), a page of their own, the one in use marked |
| Text reading model | reads the words in pictures | "hemera's choice" (the default: no model sent; the card says which that is, `/v1/ocr`'s `default_model`) or one of hemera's `ocr` models by name |
| *Dictation*: Languages spoken | as SpatiPal stepped them: English and Indonesian mixed, English, Indonesian, any | each press the next |
| Your own words | the word list | a page: each word (A twice forgets it, a card having no question to ask), and "Add a word", typed |
| *Text in pictures*: Read text in pictures by itself | on by default | each press |
| First language, Second language | English and Indonesian to begin with | a page of 24 languages; choosing for one the other's swaps them |

Under the three of text in pictures: "Text in the first language is translated into the second;
anything else into the first. For the text in pictures and paused films in SpatiLoupe and SpatiReel."
(Until Live Text came, the same day: "Nothing reads text in pictures yet".)
hemera is asked for its lists on a thread when the card opens (three GETs, no model started), and
again when its address changes; a model page opened before it answers shows the one in use, and
keeps the cursor on it when the list comes.

**One store: `ai.json` in the shared folder** (`crates/spatiand-ai`, `~/.config/spatiand/apps`),
synchronised between paired machines by spatiand-sync as SpatiPal's settings always were -- the
newest file wins, so a choice made on one machine is everyone's. Read by the room's dictation each
time it starts (`VoiceSettings::load`, now the AI settings' speech part), written by the card (read
afresh first, never over a file that cannot be read), and read by any app through the SDK:
`spatiand_app::ai::settings()`, with `Event::AiSettings` from `App::wait` when the file changes,
looked at each second as the look is. **A file, not the protocol**, because that is what already
carries settings between the room and its apps (the look in `prefs.toml`, read the same way) and
between machines (the shared folder): the protocol would have had to grow, in every client's copy,
for what a file the sync already carries does. What a newer build adds to the file is kept when an
older one saves it.

**Carried over once.** The first read of a folder with no `ai.json` (the room's or SpatiPal's, on
each machine) makes it from SpatiPal's older files -- `voice.json` (hemera, languages, words,
clean-up and transcription models, pause) and `assistant.json`'s `model` and `large_model` -- and
writes it **with their time**, not now's: a machine whose older files were behind another's makes an
older `ai.json`, and the other's wins. A folder with neither gets the defaults and writes nothing
(defaults written now would be the newest settings everywhere). Half a file being synchronised is
not carried until it is whole. The older files are left as they are, and SpatiPal keeps the old
`model` and `large_model` in `assistant.json` when it saves it, for a machine still running an
older SpatiPal; a change made in an older SpatiPal after this is not carried.

**What SpatiPal keeps**: "Assistant" (start with the room, name, personality, figure, shown),
"Voice and ears" (voice, voice described, speak answers, listen for its name, while the larger model
thinks) and "Chats" (go on in the last chat), and one line: "hemera, the models and dictation are in
Spatiand's settings, under AI." It holds the AI settings and takes a change up as it comes: the
next turn asks with the new models and hemera, its ears are started again if their languages or
words changed, hemera is asked to have the new models ready.

**The look and feel card** came with it ("make separate card for look and feel setting. pinned
windows, look, steadiness, environment, can be there... so the main menu of setting is
simplified"): the same four rows with the same pages and prefs, the look and steadiness stepping
where they are, as the pinned windows' rows do; B from the environment picker or the pinned windows'
list comes back to it. The main menu: Recentre, Keyboard, Controller layout, Take a screenshot,
Record a video, Windows, (Sound devices,) Look and feel, AI, Remote computers, Bluetooth, Wi-Fi,
Calibrate head tracking, Leave Spatiand.

Checked on deepmagpie: unit tests for the carrying over (all of it, once, dated as the older files;
a store with values not written over; nothing written for a new folder; half a file not carried
until whole; an unreadable store an error, left alone; a newer build's fields kept; 0600), for the
card (its rows and headings, the model, language and word pages, the swap, the address typed from
any keyboard), for the SDK's watch, and for SpatiPal keeping the old models in its file. In the
snapshot room, scratch folders only and a stand-in for hemera answering its two lists: the main
menu before and after, both cards and each page of the AI card; the real build carrying a pair of
older files over on the card's first opening (`ai.json` made, 0600, dated as the newer of them); a
change on the card kept and shown; SpatiPal's settings window, dark and light; and SpatiPal, running,
taking up a change made to `ai.json` from outside within a second, and having hemera ready the new
everyday model. The Beam Pro's library and SpatiPal type-check for Android. Not checked: the
glasses; the Deck, the Beam Pro and the Mac themselves (the Mac not built: the owner decides when);
two paired machines synchronising `ai.json`.

### Live Text: the text in pictures, read by hemera (2026-11-19)

The owner: "I want it can select text from images (also on image of pdf)... even non latin, unicode
character. Like apple preview."; "read automatic"; of translating, "2 language... primary and
secondary... the selection in primary, translate to secondary. other, translate to primary". What the
wearer does, and how SpatiLoupe and SpatiReel choose what to read, is in docs/spatiand-apps.md
("Live Text"); here, what is asked of hemera, and what the assistant does with it.

- **Reading** (`spatiand_voice::hemera::ocr`, hemera's `docs/ocr-api.md`): `POST /v1/ocr`, the picture
  whole in a part `image` -- a photograph as its own file, a screenshot, a frame or a page drawn as a
  PNG, nothing shrunk unless over hemera's 50 million pixels -- and its options: the AI card's first
  and second language then `auto` (`["en", "id", "auto"]`), characters, orientation 0 for a frame or
  a page drawn, the card's text reading model or none for hemera's choice. A 503 (hemera's reader
  starting) is waited out once for its Retry-After; every refusal is said in hemera's own words. Each
  reading has a connection of its own, shut when the wearer moves on: hemera stops its work. Read by
  itself when the card's "Read text in pictures by itself" is on; else from the picture's menu ("Read
  the text").
- **Translating** (`spatiand_voice::hemera::translate`): with the card's everyday model, thinking
  off. Told the rule itself, the 4B model gave Indonesian back unchanged, a lone "Diskon" as it was,
  Japanese in Indonesian; so the model is asked only which language the text is in (a two-letter
  code), the rule is applied in code -- text in the first language into the second, anything else
  into the first -- and the model then asked to translate into that one language, the lines numbered
  and given back numbered so that each is where its line was, never a digit changed. Against hemera:
  Indonesian, English, Japanese, Arabic, a sign and a lone word each went the way the rule gives, in
  0.4-0.7 s, lines and digits kept; an ID card's labels ("Nama") were left as they were. A workaround
  for a model that does not keep to the rule: the proper way is one that does, or hemera translating
  into a language it is given.
- **The assistant** (`crates/spatipal/src/brain.rs`): when the window looked at is SpatiLoupe's or
  SpatiReel's, its description is asked for with every request, and the text read in its picture
  (after `spatiand_app::live_text::HEADING`, each word read less surely than 0.9 marked `[?word?]`)
  goes with the wearer's words: "The text in the picture in window N, read from it exactly as it is
  printed, on screen now. Answer names, numbers and codes from this text, not by reading the picture
  again; a word in [? ?] was read with less certainty: say so if you give it". Asked "Pixel, what is
  the NIK on this card?" in the headless room, SpatiLoupe showing the ID card photographed askew, it
  answered "The NIK on this card is 4188889534495509." in 1.3 s -- the card's number. Its answers have
  a Copy button beside them under the pointer: the chat's text cannot be chosen.
- **Privacy**: hemera neither keeps nor logs a picture or its text, and Spatiand logs only sizes,
  times and counts; what is read is kept in the application's memory for the session (SpatiLoupe, by
  the file's path, size and time), never on disk -- an ID card passes through.

Not verified: any of it on the Deck, the Beam Pro, the Mac or the glasses; SpatiPal's answer from a
paused film's text (the same path as SpatiLoupe's, by its application's id).

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
