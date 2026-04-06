# UI Shared Helpers

This crate is intentionally small. Reuse it when multiple graphical clients share
the same low-level rendering or app-surface lifecycle, and keep app-specific
state, layout, and interaction logic local to each app.

Current shared entry points:

- `SurfaceBuffers`
  - owns mapped double-buffer setup and slot rotation for client-rendered surfaces
- `FirstPresentSurface`
  - keeps newly launched mapped-buffer windows hidden until their first complete frame is submitted
- `draw_window_frame_rgba8888` / `draw_window_titlebar_rgba8888`
  - shared RGBA window chrome for standard desktop apps
- `poll_app_lifecycle`
  - shared bootstrap lifecycle polling for graphical apps
- `decode_app_pointer_action` / `decode_app_key_action`
  - shared decoding for app-control input messages

Keep logic local when:

- the app has custom chrome or theme rules, like the terminal
- the drawing path mixes scene primitives with mapped buffers
- the event flow encodes app-specific behavior rather than shared plumbing

Do not turn this crate into a retained-mode widget framework. It exists to hold
stable client-side graphics and lifecycle scaffolding that would otherwise be
copy-pasted across apps.
