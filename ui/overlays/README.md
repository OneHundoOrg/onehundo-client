# Graphics-API Overlays (Deferred / Optional)

Per `onehundo-client/AGENTS.md`:
- Core tracking, achievement evaluation, toast notifications (`ui/notifications/`), and `Game Health` diagnostics (`ui/game-health/`) **must never** be gated on graphics-API overlays (Vulkan/OpenGL/DXGI).
- In-game overlays belong in this directory in later phases as an optional presentation consumer of `daemon/diagnostics/` and `ui/notifications/`.
