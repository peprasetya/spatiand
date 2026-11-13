# Carrying the patches to a new Chromium

The tools of the first carry (153 to 157, 2026-11-13/14), for the monthly update
(docs/spatimesh.md, "The monthly update", is the procedure; this is what each tool does).
They take where they work from the environment:

    CARRY_WORKTREE   a git worktree of the Chromium tree, at the new version's tag
    CARRY_STATE      a scratch folder for this carry: its position, logs, and stack.txt
    CARRY_BASE       the new version's tag (attribute.py)

| | |
|---|---|
| `carry.sh <state>` | Applies each commit of `stack.txt` (`<commit> <subject>` a line, oldest first; the commits must be reachable from the worktree, e.g. the old tree as a git alternate) three-way, **each file by itself**, and commits it under its own message. Paths in the dependencies that are repositories of their own are written to `<state>/sub/` instead. Stops at a conflict; `CONTINUE=1` commits what was resolved and goes on. |
| `drive.sh` | `carry.sh` again and again, with `autoresolve.py` in between, until a conflict needs a person. |
| `autoresolve.py` | The conflicts a rule settles: a file no Linux-UI build compiles takes upstream's side; both sides added at one place, both; the patch only added lines beside a block upstream changed, upstream's block and the added lines. Needs `merge.conflictStyle diff3` in the worktree. |
| `show.py`, `pick.py` | The hunks left, compactly; and a hunk settled by a letter (upstream's, the patch's, both). Every hand resolution is written to `<state>/resolve.log`, which is read afterwards. |
| `applyeach.py <diff>` | A large diff applied file by file, three-way, so that one file which cannot be applied does not hold back the rest (the Mac's port, `mac/wip/`, is carried with this). |
| `audit.py` | For every patch, the files it changed before and does not touch after, less those the log accounts for. **Run it when the carry says it is done.** As it was written for the first carry: its paths are that carry's. |
| `attribute.py <fixes.diff>` | Gives each build fix to the patch that last touched its file, as `fixup!` commits, for `git rebase -i --autosquash`; a file no patch touches goes by `<state>/orphans.map` (`<path prefix><tab><subject>`). |

**What the first carry taught:**

- `git apply` takes a whole diff or none of it. Applied a commit at a time, one file gone upstream
  cost ten patches every other file they had (about 490 files), with nothing said: found only by
  comparing each patch's list of files before and after. Hence each file by itself, and the audit.
- Most conflicts are two sides adding at one place (a feature list's end, a build file's list):
  rules settle those. Of 310 patches about 55 needed a person, and the lost files another 110.
- The settings pages moved from Polymer to Lit: a hunk in the old syntax applies without conflict
  and does nothing. Read every change under `chrome/browser/resources/settings`.
- A patch that removes a GN flag or a function is broken by every new use upstream adds. The
  compiler finds those; look for the same name in files the patch never touched.
- The generated lists under `out/*/gen/cromite_flags` are not made again when a patch adds a
  file to them: delete that folder after carrying, or the build silently lacks the new entries.
- Never `git stash`, `checkout` or `reset` in a tree whose fixes are still uncommitted, and never
  edit `build.sh` while it runs.

`carry.sh`, `drive.sh`, `pick.py`, `show.py` and `attribute.py` were changed after their one use
(the file-by-file fix, and taking their folders from the environment): those lines have not run yet.
