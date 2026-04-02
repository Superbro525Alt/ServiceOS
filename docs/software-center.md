# Software Center

## Role

`software-center-app` is the first graphical software distribution client.

It is intentionally thin:

- it renders package catalog and provenance information
- it invokes install/update/remove/sync actions through `package-service`
- it does not own repository logic, trust policy, install state, or rollback

That keeps the authority boundary clean:

- `package-service` remains the package authority
- the software center remains a desktop client

## Current UI model

The current app shows:

- package catalog entries
- category and summary text
- repository index for the selected package
- trust state
- channel and ring
- installed, active, and rollback version state
- source path for the selected package
- actions for sync, install/update, and remove

Current interaction:

- pointer selection
- wheel scrolling
- `Up` / `Down` / `PageUp` / `PageDown`
- `Enter` for install/update
- `Backspace` / `Delete` for remove
- `R` for repository sync

## Authority model

The software center receives:

- a surface handle
- an app-control handle
- an explicit `package-service` handle

It does not receive:

- direct `storage-service` package-root authority
- direct `network-service` authority for repository fetches
- direct root-manager activation power

That means package trust and install/update policy still run through the real
package service, even when the user acts from the GUI.

## Relationship to desktop lifecycle

The app is launched by `desktop-shell-service` through the existing app/window
model.

Current desktop integration includes:

- launcher visibility
- focus/minimize/maximize/close through the normal desktop shell
- package operations reflected through the same backend state that the shell
  uses

## Current limitations

The current software center is a real client, but still intentionally small.

It does not yet provide:

- repository onboarding dialogs
- rich search UX
- screenshots or long descriptions
- recommendations
- progress bars for long-running installs
- richer rollback/recovery UI
- open-with or app-association policy editing

## Roadmap note

Open software-distribution UX follow-on work is tracked centrally in
[docs/roadmap.md](roadmap.md). This page intentionally stays focused on the
current implemented software-center client.
