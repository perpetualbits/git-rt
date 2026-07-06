# Critical analysis of Terminator — the random-crash bugs

We were asked to be critical of Terminator and find the bug(s) behind its
random crashes. This is the result of a source audit of
`reference/terminator/terminatorlib/` (GPLv2). Findings are ranked by how
likely each is to be *the* intermittent-crash culprit. Each entry ends with how
`rt`'s design structurally avoids it.

---

## #1 (CONFIRMED, top suspect) — `cwd.py:15-20`: unguarded cwd probe on a dead pid

```python
def get_pid_cwd(pid = None):
    psinfo = psutil.Process(pid).as_dict()          # raises NoSuchProcess if pid is gone
    dbg('psinfo: %s %s' % (psinfo['cwd'], psinfo['pid']))
    return psinfo['cwd']                             # KeyError if 'cwd' absent (perms/zombie)
```

- **Why it fires:** `get_cwd()` (`terminal.py:267`) falls back to this on every
  **split**, **new window**, and drag/drop whenever VTE's OSC7 URI is missing.
  At exactly those moments the child shell pid is frequently *already dead* (it
  just exited, or is being replaced). `psutil.Process(pid)` then raises
  `NoSuchProcess`; `as_dict()` can raise `AccessDenied`/`ZombieProcess`; and on
  some platforms the dict simply has no `'cwd'` key → `KeyError`.
- **Why it's intermittent:** it depends entirely on whether the pid is alive and
  readable at the microsecond a key is pressed. The exception escapes a GTK key
  handler → the operation tears down and the process commonly aborts.
- **Verified:** read the file directly; there is no `try/except` anywhere in the
  function, and `psinfo['cwd']` is a raw dict index.
- **rt mitigation:** `Pane::cwd()` returns `Option<PathBuf>`. We read
  `/proc/<pid>/cwd` via `std::fs::read_link` and map **every** error
  (`ENOENT` = pid gone, `EACCES`, etc.) to `None`. No panic path. cwd is
  advisory (used to seed a new pane's directory); absence just means "fall back
  to $HOME".

## #2 — `terminal.py:1911-1914`: visible-bell timeout is a use-after-destroy

A 100 ms `timeout_add(on_bell_cleanup, widget, ...)` later calls
`widget.get_window().process_updates(True)`. If the terminal is closed within
that window, the GdkWindow is gone → `get_window()` returns `None` →
`AttributeError`. Intermittent: needs a bell immediately before a close.
- **rt mitigation:** timers are owned by the pane; on pane drop every pending
  timer is cancelled. A "bell flash" is just a render-state flag with an
  expiry, checked on the next frame — no callback can outlive the pane.

## #3 — `terminator.py:173`: `deregister_terminal` double-remove

```python
self.terminals.remove(terminal)   # ValueError if already removed
```
A terminal can be deregistered twice when a user close races the `child-exited`
handler, or on unzoom-then-close re-entry. The second `list.remove` throws.
- **rt mitigation:** panes live in a `HashMap<PaneId, Pane>`; removal is
  idempotent (`remove` on an absent key is a no-op). Close is driven by a single
  owner, not by racing signal handlers.

## #4 — `terminal.py:1519-1533`: deferred size-allocate touches a freed VTE

An `idle_add(do_deferred_on_vte_size_allocate)` later reads
`self.vte.get_column_count()`; if `close()` (`del self.vte`) ran first, the idle
fires against a missing attribute → `AttributeError`. Intermittent: resize
queued in the same frame as a close.
- **rt mitigation:** no deferred GTK idle callbacks at all. Resize is computed
  from the layout tree during the render pass against live panes only; a closed
  pane is simply not in the tree.

## #5 — `terminator.py:592-594`: `terminals.index(term)` during reparenting

`idx = terminals.index(term)` throws `ValueError` if `term` isn't in the freshly
walked descendant list — possible for a grouped/broadcast term while a split is
mid-flight.
- **rt mitigation:** membership tests use `Option` (`iter().position()`),
  skipping misses; broadcast targets are resolved against the current pane map.

## #6 (low) — `notebook.py:502/519`: deferred tab-switch on a stale page

`do_deferred_on_tab_switch` can reference a page removed between queue and fire.
Mostly None-guarded, low crash risk; noted for completeness.

---

## The common root cause

Every one of these is the **same shape**: a *deferred* or *signal-driven*
callback (GLib `timeout_add`/`idle_add`, or a GTK signal) runs against widget or
process state that changed underneath it — a classic GTK main-loop reentrancy /
use-after-free hazard, made worse by unguarded external OS queries (#1).

`rt` removes the entire class by construction:
1. **No widget reparenting.** Layout is a pure data tree (`rt-core`); panes are
   rendered onto one GPU surface, never reparented GTK widgets.
2. **No deferred callbacks holding stale references.** State is pulled during
   the render/update pass from live owners; there are no fire-later closures
   capturing soon-to-be-freed widgets.
3. **Fallible OS queries return `Option`/`Result`,** never unwrap on the hot
   path. cwd, pid, and PTY reads all degrade gracefully.

This is *why* the port is both faster (alacritty engine) and more robust
(structurally can't hit Terminator's reentrancy crashes).
