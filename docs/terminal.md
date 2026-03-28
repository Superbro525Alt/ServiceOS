# Graphical Terminal and Shell Hosting

## Role

The graphical terminal is the first desktop application that hosts the existing
operator shell inside a normal desktop window.

It does not introduce a second shell implementation. Instead, it adds a
terminal-session boundary between shell logic and graphical presentation so the
same command/runtime stack can be reused in both environments.

## Structure

The current stack is:

```text
shell-service
  -> console-service session
       -> raw serial console path

terminal-app
  -> terminal-service session
       -> shared shell command/runtime library

desktop-shell-service
  -> launches terminal-app through root-manager
```

This keeps the boundaries explicit:

- `shell-service`
  - remains the text-first operator service
- `terminal-service`
  - owns terminal-session lifecycle and line discipline
  - reuses the shared shell command/runtime code
- `terminal-app`
  - is only the graphical terminal UI and keyboard/presentation layer
- `desktop-shell-service`
  - remains responsible for app launch, focus, and window management

## Shared shell model

The shell implementation now lives behind a shared library boundary inside the
`shell-service` package.

That shared layer provides:

- the command parser and dispatcher
- built-in command implementations
- the common prompt and shell-ready text
- a small `ShellOutput` abstraction so the same shell logic can write either to
  a console session or to a terminal session

Current hosting paths:

- `shell-service`
  - uses `ShellOutput` backed by `console-service`
- `terminal-service`
  - uses `ShellOutput` backed by the terminal-session channel

This preserves one shell behavior model across serial and graphical use.

## Terminal-session model

`terminal-service` provides a small PTY-like session boundary.

Per session it owns:

- one bidirectional session channel
- line-edit state
- cursor position within the current input line
- history ring
- terminal size metadata
- lifecycle state

Current session operations:

- open a session
- query session status
- list sessions
- send input bytes
- report resize
- close a session
- receive output text

The current model is intentionally simple:

- shell execution is synchronous per session
- terminal output is text-oriented rather than a full PTY byte stream
- minimal ANSI/CSI handling is supported for prompt redraw and cursor movement

That is enough for real shell use without hardwiring shell behavior into the
graphical UI.

## Keyboard and control behavior

Terminal-local keyboard behavior is handled through the real desktop input path:

1. kernel input backend produces keyboard events
2. `session-service` routes them into the desktop interaction contract
3. `desktop-shell-service` delivers focused app key/text events over the
   app-control channel
4. `terminal-app` translates those events into terminal-session input bytes
5. `terminal-service` applies line editing or runs the shared shell command

Currently supported interactive behavior:

- typing printable text
- `Enter` to submit a command
- `Backspace`
- left and right arrow movement within the current line
- up and down command-history traversal
- history recall and editing
- `Ctrl+C` to cancel the current line and redraw the prompt
- prompt redraw after command output
- focus-aware keyboard delivery through the normal desktop app path

Global desktop shortcuts remain outside the terminal session. The terminal only
consumes input after the desktop has decided that the terminal window is the
focused app target.

## Rendering model

`terminal-app` is a normal shared-buffer graphical client.

It currently:

- opens a normal desktop surface
- attaches one writable shared buffer
- draws its own titlebar and terminal content into that buffer
- tracks a scrollback grid
- renders a simple monospaced bitmap font
- shows a focused cursor
- handles redraw on output, resize, and focus changes

The terminal window therefore uses the same graphics/session/window model as
other desktop apps instead of bypassing the compositor.

## Lifecycle

Launch flow:

1. `desktop-shell-service` requests `TerminalApp` launch through the root
   manager
2. the root manager grants the app its surface handle, app-control channel, and
   a rights-scoped handle to `terminal-service`
3. `terminal-app` opens a terminal session from `terminal-service`
4. `terminal-service` creates a session channel and emits a
   `terminal-session-opened` lifecycle record
5. the app begins rendering terminal output and forwarding keyboard input

Close flow:

1. window close reaches `terminal-app` through the normal app-control path
2. `terminal-app` closes its terminal session
3. `terminal-service` tears down the session and emits a
   `terminal-session-closed` lifecycle record
4. the app exits as a normal graphical task
5. `desktop-shell-service` cleans up the window state

## Current limitations

The terminal is now real and usable, but still intentionally scoped:

- no tabs or panes
- no text selection or copy/paste yet
- no full ANSI/VT escape coverage
- no shell job control
- no independent background process group handling
- no alternate screen buffer
- no configurable themes or profiles

## Deferred

- tabs and split panes
- richer ANSI/VT escape handling
- selection, copy, and paste within the desktop capability model
- better terminal resize semantics for future richer process environments
- terminal themes and profiles
- remote terminal and SSH-oriented workflows
