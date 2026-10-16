# Fery Yundara Putera

Everything here is public. Each line names its source; the bot may only say what is
written here.

## Who

- Senior Software Engineer in Indonesia (UTC+7), working remotely; open to a full-time
  remote role. [github.com/daefery](https://github.com/daefery), [daefery.github.io](https://daefery.github.io)
- Twelve years in software, eight of them on a fully remote team. [github.com/daefery](https://github.com/daefery)
- How he works: he sets the constraints, gives coding agents small, clear tasks, and checks
  the result against what the user needs; an agent saying "done" is a claim to check, not
  proof. [daefery.github.io](https://daefery.github.io)
- Contact: feryyp.work@gmail.com. [github.com/daefery](https://github.com/daefery)

## Work

- Solve Education! (2018 to now): leads the AI automation platform and built the team's
  AI coding-agent setup; before that owned the frontend architecture of edbot.ai, which
  delivered 32M+ lessons to 100K+ peak monthly users across 10+ countries, and built the
  team's first CI/CD. [daefery.github.io](https://daefery.github.io), [github.com/daefery](https://github.com/daefery)
- Earlier: Xtremax (2016 to 2018, backend developer for clients in Singapore and Indonesia),
  Elephant Talk (2015 to 2017, C# microservices and AngularJS apps), Suzuki Indomobil
  (2014 to 2015, .NET dealer management system). [daefery.github.io](https://daefery.github.io)
- Merged contributions to Automattic/jetpack in October 2026: #53574 (Pinterest share
  button on AMP pages, with tests) and #53543 (apply_shortcodes instead of do_shortcode).
  [github.com/daefery](https://github.com/daefery)
- Research: "Per-Outcome Baseline Normalization Makes Proper Forecast Scores Improper",
  with Bramantya Farid Prakoso, accepted for a poster at Agenthon 2026, a workshop held at
  NeurIPS 2026 (non-archival). Always say "a NeurIPS 2026 workshop", never "NeurIPS" alone.
  [daefery.github.io](https://daefery.github.io)

## ravnori (his memory tool for coding agents)

- Local-first memory for coding agents, written in Rust with SQLite, AGPL-3.0, public since
  4 October 2026. It keeps one record of every Claude Code, Codex and pi session, read from
  the transcripts those agents already save. [github.com/daefery/ravnori](https://github.com/daefery/ravnori)
- The idea: capture reads the transcript files and saves its read position in the same
  database write, so a crash, a skipped hook or a model outage delays a memory instead of
  losing it. On Fery's machine it re-read 1.45 GB of transcripts from scratch with zero
  duplicates. [ravnori README](https://github.com/daefery/ravnori#readme)
- Every memory cites the transcript lines it came from, and each one knows whether the code
  its session wrote is still in the repository. [ravnori README](https://github.com/daefery/ravnori#readme)
- It began as a replacement for claude-mem on his own machine: there, the claude-mem version
  installed in September stored nothing for 157 of 493 sessions in 30 days, which is the
  failure ravnori was built to avoid. `rvn import` keeps claude-mem's ids. Never call
  ravnori's capture "impossible to lose" or "guaranteed": say a failure delays a memory
  while the transcript is still on disk.
  [comparison](https://daefery.github.io/ravnori/comparison.html)
- Its comparison page was rechecked against claude-mem v13.39 on 16 October 2026 and says
  where claude-mem has since closed gaps: raw tool calls kept before any model call, hooks
  spooled to disk, the observer pausing instead of dropping work, and pi support.
  [comparison](https://daefery.github.io/ravnori/comparison.html)
- Where ravnori is behind, by its own page: vague questions find their memory in the top 5
  only 60% of the time, agents open fewer than 1 in 10 of the memories it shows, three
  agents rather than claude-mem's 10+, and one user so far.
  [comparison](https://daefery.github.io/ravnori/comparison.html)

## Other things he built

- Lariksa: offline meeting notes for Windows (Rust, Tauri, whisper.cpp, llama.cpp); every
  action item cites the transcript line it came from. [github.com/daefery/lariksa](https://github.com/daefery/lariksa)
- Four apps designed and shipped solo: Vacua (macOS), Qadha, Plareon and Zokuu (React
  Native). [github.com/daefery](https://github.com/daefery)

## This channel

- Built by Fery's coding agent under his direction, for the claude-mem application at
  cmem.ai/apply. Noise IK over WebSocket: the server's key is published in this repo, the
  caller proves its own key in the first message, and everything after is encrypted and
  authenticated. The bot behind it has no tools.
  [README](https://github.com/daefery/bot2bot#readme)
