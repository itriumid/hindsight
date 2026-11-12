@AGENTS.md

<!--
  Claude Code reads CLAUDE.md and not AGENTS.md, so this file imports it. Keep the shared
  instructions in AGENTS.md so every tool reads the same thing; put anything that only applies
  to Claude Code below.

  Don't add `@.handbook/...` imports here — `.handbook/` resolves outside this repository, and
  an import that resolves outside the working directory is treated as external: it prompts for
  approval once, and declining disables it permanently and silently. AGENTS.md tells the agent
  to *read* those files on demand instead, which isn't gated.
-->

## Choosing a model for a subagent

`.claude/settings.json` starts a session on the strongest model and its subagents on a lighter
one. That subagent default fits a well-scoped task with a clear way to check the result. It is a
default, not a limit — set the model on the individual subagent when the task calls for it:

- **Lighter still** for mechanical work: searching, listing, renaming, edits that follow a
  pattern already written down.
- **The session's own model** when the task needs judgment a lighter model is likely to get
  wrong — an unclear root cause, a design choice, a change that crosses several modules.

A task too unclear to choose a model for is too unclear to hand to a subagent yet.
