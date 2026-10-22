# rsmpeg, as Spatiand builds it

The Rust bindings SpatiReel, SpatiLoupe, SpatiShelf, SpatiMesh and the host decode with. This is
rsmpeg **0.18.0+ffmpeg.8.0** from crates.io, unchanged except for the patches below, used in place
of the crates.io release by `[patch.crates-io]` in the workspace's Cargo.toml.

**Why:** Spatiand ships FFmpeg 9 (`third_party/ffmpeg/versions.sh`), and no rsmpeg release
supports it yet.

| Patch | What |
|---|---|
| `patches/0001-ffmpeg-9.patch` | `AVCodec`'s lists (`pix_fmts`, `sample_fmts`, `supported_framerates`, `supported_samplerates`) were removed from the struct in FFmpeg 9: read through `avcodec_get_supported_config` (there since FFmpeg 7.1). `av_parser_init` takes the codec id enum in 9 and an int before: cast to whatever the bindings say. |

**To move to a new rsmpeg:** replace `src/`, `Cargo.toml`, `LICENSE` and `README.md` with the
new release's (`~/.cargo/registry/src/*/rsmpeg-<version>/` after a `cargo fetch`), then apply
each patch in order with `patch -p1 < patches/NNNN-*.patch` from this folder. A patch that no
longer applies is either upstream already (drop it) or needs redoing against the new code. When
a release supports the FFmpeg we ship, delete this folder and the `[patch.crates-io]` entry.
