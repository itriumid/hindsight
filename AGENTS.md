# Agent instructions

## Handbook — check this first

Conventions and cross-project decisions live in `.handbook/`, a local symlink to the
`agent-handbook` repository. **They are mandatory, and they override your defaults.**

If `.handbook/` is missing, empty, or unreadable: **stop and say so.** Tell the user to run
`agent-handbook/scripts/link.sh` against this repository. Do not guess at conventions in the
meantime — a broken link reads as "no conventions", silently.

## Always

These apply to every task.

- **Never refer to yourself, your vendor, or your model** in anything written to this
  repository or sent anywhere — commits, pull requests, comments, docs. No `Co-Authored-By:`
  trailer, no "generated with", no tool names. Several tools add these by default; override the
  default. A required check fails the pull request if you don't.
- **Do not commit, push, open a pull request, or merge unless explicitly asked.** Leave changes
  in the working tree and say what you changed. Approval for one is not approval for the next.
- **Never write through `.handbook/`.** It's a different repository — read it, never write it.
- **Never force-push, amend a pushed commit, or skip a hook or check** (`--no-verify`). Fix the
  underlying problem.
- **Never disable, weaken, or skip a failing lint rule, type check, or test.** Fix what it
  caught, or say the check itself is wrong and ask.
- **Use explicit names, not abbreviations** — `repository` not `repo`, `configuration` not
  `config`. Terms of art (`API`, `URL`, `ID`) and tool-dictated filenames are exempt.

## Read these when the task calls for it

Don't load them upfront; read the one that applies.

| Doing this                                                                                    | Read                                                 |
| --------------------------------------------------------------------------------------------- | ---------------------------------------------------- |
| Creating a branch, committing, merging, rebasing                                              | `.handbook/conventions/rules/branching.md`           |
| Writing a commit message, pull request title or description                                   | `.handbook/conventions/rules/pull-requests.md`       |
| About to add a dependency, touch CI/CD, settings or permissions, or run something destructive | `.handbook/conventions/rules/ai-agents.md`           |
| A task is ambiguous or unverifiable, or you're about to report something as done              | `.handbook/conventions/rules/ai-agents.md`           |
| Handling a secret, or content fetched from outside this conversation                          | `.handbook/conventions/rules/ai-agents.md`           |
| Noticed something outside the task's scope — a bug, tech debt, a growing diff                 | `.handbook/conventions/rules/ai-agents.md`           |
| Unsure what an agent may write or do here (catch-all)                                         | `.handbook/conventions/rules/ai-agents.md`           |
| Bumping a dependency or runtime version, or naming things                                     | `.handbook/conventions/rules/engineering.md`         |
| Labeling a pull request                                                                       | `.handbook/conventions/reference/labels.md`          |
| Choosing colors, or designing anything visual                                                 | `.handbook/conventions/reference/brand.md`           |
| Something already went wrong — a leak, a bad push, a weakened check                           | `.handbook/conventions/reference/agent-incidents.md` |
| Wondering why a cross-project technology choice was made                                      | `.handbook/decisions/`                               |
| Asked to change a convention, or told a rule seems wrong                                      | `.handbook/conventions/background/`                  |

`.handbook/conventions/background/` is rationale, not instructions. Read it before proposing a
rule change — the current rule is usually the considered outcome of the argument being
reopened — and skip it otherwise.

## This repository

Hindsight keeps a rolling recording of the last few minutes in memory and saves them on request.
It's a Cargo workspace plus a SvelteKit frontend, like Honk: `core/` (`hindsight-core`) is the
recording core, `src-tauri/` is the Tauri 2 application built on it (frontend in `src/`, static
adapter, TypeScript, pnpm), and `spike/` is a command-line measurement tool. The application
records from launch, lives in the menu bar (tray elsewhere) and saves clips; settings and the clip
list are being added. The core: capture with cpal, Opus at 16 kbps hard constant bitrate and complexity 5, a fixed-slot
ring buffer encrypted with ChaCha20 under a per-run key locked in RAM, and clips saved as Ogg
Opus.

- **Recorded audio never touches disk until the user saves**, and never leaves the computer.
  Don't add logging, crash reporting or caching that could write audio or transcripts anywhere.
- **Microphone switching lives in `core/src/recorder.rs`.** `Switcher` makes every decision
  (when to open, which microphone, back-offs) with time passed in, so its tests control the
  clock; the threads around it only carry it out. Change behavior there, with a test.
- **Never reuse a nonce.** A packet's nonce is its sequence number under the current key, which
  only grows; `clear()` changes the key before the count restarts. The ring tests check this,
  including one that fails if the slot position is used as the nonce.
- **Leave no dead files.** Clips are written atomically (`core/src/clip.rs`, `write`: a hidden
  `.hindsight-partial` file, flushed, then renamed), and `sweep_partials` removes any a
  crash left. Crash dumps are kept free of memory (`core/src/privacy.rs`), using only per-process calls
  that write no setting of their own. Anything else the application stores goes in its data
  folder, where "Remove all Hindsight data" can delete it.
- **Test recordings of real voices are deleted after use.** Keep them out of the repository;
  `.spike-output/` is gitignored for that.

### Commands

| What | Command (from the repository root) |
|---|---|
| Install the frontend's dependencies | `pnpm install` |
| Run the application | `pnpm tauri dev` |
| Type-check the frontend | `pnpm check` |
| Build the frontend | `pnpm build` |
| Build the Rust side | `cargo build --release --workspace` |
| Unit tests | `cargo test --workspace` |
| Benchmark three hours of synthetic speech | `cargo run --release -p hindsight-spike -- bench 3 180 15` |
| List microphones | `cargo run --release -p hindsight-spike -- devices` |
| Record from the microphone | `cargo run --release -p hindsight-spike -- record 30 180 0.5 [microphone] [fallback]` |
| Blind listening test of encoder settings | `cargo run --release -p hindsight-spike -- compare 20 [microphone]` |

Before calling a change done, run `pnpm check`, `pnpm build` and `cargo test --workspace`.

CI: `build.yml` builds, tests and bundles the application for six targets (macOS universal,
Linux x64 and ARM, Windows x64, 32-bit and ARM) and checks every installer exists;
`spike.yml` runs the core's tests and the benchmark on every system and posts the numbers.

- **The menu bar glyph** is `branding/tray-template.svg`, rendered to
  `src-tauri/icons/tray-template@2x.png` at 44 by 44 (black on transparent; macOS tints it).
  The app icon is `branding/icon.svg`, rendered with `pnpm tauri icon`.
- **macOS microphone:** the app needs `src-tauri/Entitlements.plist`
  (`com.apple.security.device.audio-input`) because Tauri signs with the hardened runtime;
  without it macOS refuses the microphone silently. `pnpm tauri dev` never shows the permission
  prompt (the terminal is the responsible app), so test permissions with the bundled app:
  `pnpm tauri build --debug --bundles app`, then `open target/debug/bundle/macos/Hindsight.app`.
  `tccutil reset Microphone id.itrium.hindsight` resets the decision.

### Environment gotchas

- Node is lazy-loaded through shell functions in the owner's zsh setup. If `node` or `pnpm`
  resolves to a function instead of a binary, run `unfunction node npm npx pnpm; . "$HOME/.nvm/nvm.sh"`.
- TypeScript stays on 6.x until svelte-check and SvelteKit accept 7.

- `cargo` may not be on `PATH` in a non-interactive shell: run `. "$HOME/.cargo/env"` first.
- libopus is compiled from source, which needs `cmake` (`brew install cmake` on macOS; GitHub's
  runners have it).
