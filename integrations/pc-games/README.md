# Unmodified PC Games Integration Adapter

For unmodified PC games without source instrumentation:
- Use **Plane C (`daemon/observation/external-importers/`)** save-file scanners or platform importers (`steam_api`, `save_file`, `log_parser`).
- Observation records must use `LOCALLY_OBSERVED`, `PLATFORM_CONFIRMED`, or `INFERRED` evidence classes—never `DIRECT_INTEGRATION_EVENT` or `RUNTIME_OBSERVED`.
- For modded/quick native integrations (`onehundo_quick.h`), run in the frictionless `LOCAL` trust lane.
