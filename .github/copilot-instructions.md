# Copilot Cockpit

Aplicación en Rust, desarrollada desde cero, para trabajar de forma más eficiente con GitHub Copilot CLI.

## Objetivo

1. **Multiproyecto:** varias ventanas/paneles, uno por proyecto, cada uno con su propia sesión de Copilot (`cwd` = carpeta del proyecto).
2. **Dashboard en tiempo real:** vista interactiva de todos los agentes y subagentes en ejecución (estado, tool calls, plan, permisos pendientes).

## Decisión técnica clave

La integración con Copilot se hace como **cliente ACP** (Agent Client Protocol, public preview):
`copilot --acp --stdio` (o `--port N`), JSON-RPC sobre NDJSON. No se parsea la salida de un PTY salvo como último recurso.
Docs: https://docs.github.com/en/copilot/reference/copilot-cli-reference/acp-server

## Fase actual: MVP de TUI

Spike cerrado (conclusiones en `docs/spike-acp.md`): ACP para control (prompts, permisos) y
`~/.copilot/session-state/<sessionId>/events.jsonl` para la jerarquía de subagentes.
MVP: `copilot-cockpit tui [--config FILE]` (por defecto `~/.config/copilot-cockpit/projects.toml`,
con `[[projects]]` `name`/`path`): una sesión ACP por proyecto, pestañas con transcripción, prompts,
permisos (`y`/`n`) y dashboard de agentes (`d`). `copilot-cockpit spike ...` conserva el spike.
Teclas completas en `docs/tui-keys.md`. UI en `src/tui/{mod,theme,widgets,modals,session,input,markdown,dashboard,history,open,settings}.rs`.

## Stack

- Rust estable, edición 2024, `tokio` para async, `serde`/`serde_json`, `anyhow` o `thiserror`.
- Crate `agent-client-protocol` si encaja; si no, JSON-RPC mínimo propio.
- UI: `ratatui` (reexporta `crossterm`); `serde` + `toml` para la config. Sin `notify`: `events.jsonl` se sondea cada 1 s.

## Convenciones

- El acceso a Copilot va aislado en un módulo adaptador (`src/acp/`): el protocolo está en preview y cambiará.
- Sin `unwrap()`/`expect()` fuera de tests; errores con contexto.
- `cargo fmt`, `cargo clippy -- -D warnings` y `cargo test` deben pasar antes de dar algo por terminado.
- Dependencias mínimas; justificar cada crate nuevo.
- Nunca commitear secretos ni logs de sesiones reales con datos sensibles.
- Comentarios solo donde aclaren algo no obvio.

## Flujo de trabajo multiagente

Ver `.github/agents/`:
- **developer**: implementa el cambio más pequeño que resuelve la tarea.
- **tester**: verifica con tests y revisa el cambio.

Ambos usan las skills `ponytail` y `ponytail-review`. Flujo: developer implementa → tester valida → si hay hallazgos, vuelve a developer.
