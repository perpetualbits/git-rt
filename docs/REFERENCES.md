# Reference sources

The `reference/` directory holds read-only clones of the two projects `rt`
draws from. It is **gitignored** — the code is not part of `rt`'s history and
keeps its own upstream licenses. Re-fetch with:

```sh
mkdir -p ~/git/rt/reference && cd ~/git/rt/reference
git clone --depth 1 https://github.com/gnome-terminator/terminator.git
git clone --depth 1 https://github.com/alacritty/alacritty.git
```

## Terminator (the feature model we port)
- Language: Python 3 + GTK3 (PyGObject), terminal widget = GTK VTE.
- ~15.8k LOC under `terminatorlib/`.
- Key modules studied:
  - `paned.py` — recursive horizontal/vertical split containers.
  - `notebook.py` — tabs.
  - `container.py` / `factory.py` — the reparenting machinery (source of the
    crash class we avoid; see `TERMINATOR_BUGS.md`).
  - `terminator.py` — the Borg (shared-state) singleton, groups/broadcast.
  - `keybindings.py` — the keymap we mirror.
  - `terminal.py` — the VTE widget wrapper; `cwd.py` — cwd detection.
- License: GPLv2. We do **not** copy its code; we reimplement behavior.

## Alacritty (the engine we reuse)
- Rust workspace. The reusable crate is **`alacritty_terminal`**
  (PTY via its `tty` module, damage-tracked `Grid`, VTE/ANSI parser).
- Version vendored: `alacritty_terminal 0.26.1-dev`, `alacritty 0.18.0-dev`,
  edition 2024, rust-version 1.85.
- We depend on `alacritty_terminal` (the published crate) rather than the
  front-end. License: Apache-2.0 / MIT.

## License posture for rt
- Reusing `alacritty_terminal` as a dependency → Apache-2.0/MIT (permissive).
- Terminator is GPLv2 but we take *ideas/behavior*, not code. To be safe and
  simple, `rt` is licensed **GPLv3-or-later** (compatible superset, matches the
  spirit of porting a GPL project). Recorded in ADR; revisit if we ever copy
  any Terminator code verbatim (we must not).
