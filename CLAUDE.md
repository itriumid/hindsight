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
