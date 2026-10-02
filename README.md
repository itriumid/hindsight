# Hindsight

Keep the last few minutes of what was said, in case you need them.

Hindsight is a free desktop application from [Itrium](https://itrium.id), for macOS, Windows and
Linux. It keeps a rolling recording of the last few minutes (up to three hours) in memory, so
when someone says something you'll want to check later, you press a hotkey and save it. Until
you save, nothing is written to disk, and nothing ever leaves your computer.

## Status

**The application is early, but it works**: after a first-run screen, it records, lives in the
menu bar, saves the last 1, 5 or 15 minutes as a clip, lists and plays your clips (with WAV export
for apps that can't open Opus), and has settings for the microphone, how far back it goes (up to
three hours), a save shortcut and launch at login. Get it from the
[latest release](https://github.com/itriumid/hindsight/releases/latest), or on macOS with
`brew install --cask itriumid/tap/hindsight`; [itrium.id/hindsight](https://itrium.id/hindsight)
has the details. The recording core (`core/`) underneath it is done and measured, through a
spike (`spike/`) made to answer whether the idea holds up before any interface was designed.
On macOS:

- About 2% of one processor core while recording.
- A three-hour buffer takes 22 MB, allocated once. It's encrypted in memory with a key made
  fresh every time Hindsight starts and locked in RAM, so even if the system writes part of the
  buffer to swap, it only writes scrambled bytes. Where the system allows it (macOS and Windows,
  and Linux up to its default limit of about an hour), the buffer is locked in RAM as well.
- Saving the last 15 minutes takes a few milliseconds and makes a 1.9 MB `.opus` file.
- A microphone that disconnects, goes quiet, or sends only silence is noticed within seconds,
  and recording moves to a fallback microphone.

The same benchmark runs on macOS, Linux and Windows (Intel and ARM) on every change.

One thing no application can control: hibernation writes all of memory to disk, locked memory
included. Turn on your disk's encryption (FileVault, BitLocker, or LUKS) to cover that.

## Colors

Hindsight offers four palettes under **Settings › Colors**. Rhodonite, Itrium's own, is the
default. The other three are [Catppuccin](https://catppuccin.com) flavors (Mocha, Macchiato and
Frappé), each paired with Catppuccin Latte in light mode. The theme (System, Light or Dark)
picks the light or dark version of whichever palette is chosen.

The palettes come from [`@itrium/palettes`](https://github.com/itriumid/palettes), shared with
[Honk](https://github.com/itriumid/honk), which also has the colors and how they were adapted.
Every one meets level AA of the Web Content Accessibility Guidelines in every theme, as
Hindsight uses it: `pnpm test` checks that, and CI runs it.

## Recording people

Recording a conversation can need the consent of everyone in it, and the rules differ by
country. Hindsight will make it obvious that it's recording; it's still on you to use it where
that's allowed.

## License

[MIT](LICENSE)
