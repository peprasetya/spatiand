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

Not yet: SpatiMesh's description (the page's words, through its `RunScript`), Mac windows'
through Accessibility, a settings page for languages, words and models, the Deck build (it was
offline) and the Beam Pro's, and the whole of the assistant.
