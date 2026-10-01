# Contributing to Hindsight

Thanks for helping. This page covers setting up, the rules a pull request has to pass, and how
releases are cut. The rules are short, and two of them are enforced by required checks, so
reading them first saves you a red pull request.

## Reporting a bug or asking for a feature

Open a [GitHub Issue](https://github.com/itriumid/hindsight/issues). Search first. For a bug,
include your OS and version, how you installed Hindsight (`.dmg`, `.exe`, `.msi`, `.deb`,
`.rpm` or `.AppImage`), the microphone you record from, and the steps to reproduce it. **Never
attach a recording of other people** to an issue. Tech debt and tooling problems have their own
**Technical debt** form.

## Setting up

You need [Rust](https://rustup.rs) (stable), Node.js 24, pnpm 12 and `cmake`, which compiles
libopus from source (`brew install cmake` on macOS).

```sh
pnpm install
pnpm tauri dev
```

On Linux, install Tauri's [system dependencies](https://tauri.app/start/prerequisites/#linux)
plus the ALSA headers (`libasound2-dev` on Debian and Ubuntu, `alsa-lib-devel` on Fedora).

`pnpm tauri dev` runs Hindsight inside your terminal, so on macOS the terminal is the app asking
for the microphone and Hindsight's own permission prompt never appears. To test permissions,
build the app and open it:

```sh
pnpm tauri build --debug --bundles app
open target/debug/bundle/macos/Hindsight.app
```

Before opening a pull request, run all three checks, even if your change only looks like it
touches one side:

| What | Command |
| --- | --- |
| Type-check the frontend | `pnpm check` |
| Build the frontend | `pnpm build` |
| Test the Rust side | `cargo test --workspace` |

[`AGENTS.md`](AGENTS.md) lists the rules that are easy to break by accident: recorded audio never
touches disk until someone saves, nothing records before the first-run screen, clips are
streamed and never decoded whole, and a nonce is never reused.

## Pull requests

Hindsight follows a shared set of conventions, kept in
[agent-handbook](https://github.com/itriumid/agent-handbook/blob/main/conventions). The parts that matter here:

- **Branch from `main`** and name the branch `<type>/[<issue>-]<slug>`, where `<type>` is one of
  `feature`, `enhancement`, `fix` or `chore`: for example `fix/12-fallback-flicker` or
  `feature/clip-renaming`. The **Branch name** check enforces it.
  [Details and how to pick a type](https://github.com/itriumid/agent-handbook/blob/main/conventions/rules/branching.md).
- **Title the pull request in the imperative mood**, with no `feat:`-style prefix or trailing
  period ("Add clip renaming"). It becomes the merge commit's subject, so it's the changelog.
- **Fill in the template**: Why, What changed, How to verify. Write `Closes #12` if it resolves
  an issue. [More on descriptions and commits](https://github.com/itriumid/agent-handbook/blob/main/conventions/rules/pull-requests.md).
- **Commits are kept as written.** Pull requests land as merge commits, never squashed, so
  make each commit one reviewable change with a real message. To catch up with `main`,
  rebase, don't merge it in.
- **No tool attribution** in commits or the pull request: no `Co-Authored-By:` bot trailers, no
  "generated with". The **Commit messages** check enforces it. Using tools is fine; stamping
  them into history isn't. [Why](https://github.com/itriumid/agent-handbook/blob/main/conventions/rules/ai-agents.md#no-self-attribution-in-anything-kept).
  If you work with an AI coding tool, point it at [`AGENTS.md`](AGENTS.md).

Every pull request also has to pass the six **Build** checks (macOS, Linux, Linux ARM, Windows,
Windows 32-bit, Windows ARM) from [`build.yml`](.github/workflows/build.yml), which type-check,
test and bundle the app on each. Changes to the recording core also run
[`spike.yml`](.github/workflows/spike.yml), which measures it on every system.

## Versioning

Hindsight follows [Semantic Versioning](https://semver.org): `MAJOR.MINOR.PATCH`, tagged
`v0.1.0`. Hindsight is an app, not a library, so "compatible" means what people rely on between
versions:

- their settings
- their saved clips, which are standard `.opus` files
- the operating systems Hindsight runs on

| Bump | When | Example |
| --- | --- | --- |
| **MAJOR** | Something people rely on stops working: saved settings or clips no longer open, a feature is removed, or an OS version is no longer supported | `1.4.2` → `2.0.0` |
| **MINOR** | Something new that doesn't break anything | `1.4.2` → `1.5.0` |
| **PATCH** | A fix, with nothing new | `1.4.2` → `1.4.3` |

While Hindsight is `0.x`, the major number stays at 0: a breaking change bumps **minor** instead,
and everything else bumps **patch**. `1.0.0` is the first version with a promise of stability.

Some things hold at every version, whatever the number says:

- **Upgrading never loses settings or clips.** New settings get defaults when an older settings
  file is read, and clips stay standard Ogg Opus.
- **No prerelease or build suffixes** such as `-beta.1` or `+build.5`. Windows installers only
  accept numeric versions, so the release workflow refuses those tags. To try a release before
  tagging it, use the dry run described below.

## Releasing

Maintainers only.

1. Pick the version (see **Versioning** above), then set it everywhere it's recorded:

   ```sh
   pnpm bump-version 0.1.0
   ```

   That updates `package.json`, `src-tauri/Cargo.toml`, `Cargo.lock` and
   `src-tauri/tauri.conf.json`, changing only the version lines. It refuses anything that isn't
   `MAJOR.MINOR.PATCH`, or isn't higher than the current version. Land the four files through a
   pull request, like any other change.

2. Tag the merge commit on `main` and push the tag. Release tags are annotated, so they record
   who tagged them and when, and so they also work when git is set to sign tags
   (`tag.gpgSign`), which needs a message:

   ```sh
   git switch main && git pull
   git tag -a v0.1.0 -m "Hindsight 0.1.0"
   git push origin v0.1.0
   ```

3. [`release.yml`](.github/workflows/release.yml) checks that the tag is `vMAJOR.MINOR.PATCH` and
   matches the app's version, creates a **draft** release with notes generated from the merged
   pull requests' labels, and builds the installers on each operating system, attaching them to
   that draft as they finish. Once every installer is there, it also attaches copies of the main
   ones without the version in their names (`Hindsight_universal.dmg`), so
   `releases/latest/download/` links always fetch the newest release.
4. When every build is green, open the draft on the Releases page, check the notes and the
   attached files, and click **Publish release**. Releases are immutable once published, so
   check before you click.
5. Publishing starts [`homebrew.yml`](.github/workflows/homebrew.yml), which opens a pull request
   on [itriumid/homebrew-tap](https://github.com/itriumid/homebrew-tap) pointing the Hindsight
   cask at the new `.dmg`. Merge it there once its checks pass. If the workflow failed, run it
   again from the Actions tab with the tag. It needs the `HOMEBREW_TAP_TOKEN` secret (see the
   workflow's header) and a `Casks/hindsight.rb` to update, so the very first release adds the
   cask by hand.

To try the release builds without making a release, run the **Release** workflow manually from
the Actions tab. It builds the same installers and keeps them as workflow artifacts instead.
