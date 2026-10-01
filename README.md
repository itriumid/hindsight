# Hindsight

Keep the last few minutes of what was said, in case you need them.

Hindsight is a free desktop application from [Itrium](https://itrium.id), for macOS, Windows and
Linux. It keeps a rolling recording of the last few minutes (up to three hours) in memory, so
when someone says something you'll want to check later, you press a hotkey and save it. Until
you save, nothing is written to disk, and nothing ever leaves your computer.

## Status

**There's no application yet.** This repository holds a measurement spike (`spike/`): the
recording and saving core, built to answer whether the idea holds up before any interface is
designed. On macOS it does:

- About 2% of one processor core while recording.
- A three-hour buffer takes 22 MB, allocated once and locked in memory, so the system never
  writes it to swap.
- Saving the last 15 minutes takes a few milliseconds and makes a 1.9 MB `.opus` file.
- A microphone that disconnects, goes quiet, or sends only silence is noticed within seconds,
  and recording moves to a fallback microphone.

Windows and Linux are measured next.

## Recording people

Recording a conversation can need the consent of everyone in it, and the rules differ by
country. Hindsight will make it obvious that it's recording; it's still on you to use it where
that's allowed.

## License

[MIT](LICENSE)
