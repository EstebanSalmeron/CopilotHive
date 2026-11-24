# TUI: teclas

`copilot-cockpit tui [--config FILE]`. Terminal mínima 60x17; el panel de agentes se oculta por debajo de 100 columnas.

| Modo | Tecla | Acción |
|------|-------|--------|
| Global | `Tab` / `Shift+Tab`, `Alt+1..9` | cambiar de proyecto (el borrador se conserva por pestaña) |
| Global | `Ctrl+C` | cancelar el turno en curso (`session/cancel`) |
| Normal | `Enter` / `i` | escribir un prompt |
| Normal | `f` | `/fleet`: abre el prompt con `/fleet ` ya escrito; tras el objetivo, Copilot reparte el trabajo en subagentes paralelos (visibles en el dashboard `d`/`s`). `/fleet` es un comando que Copilot anuncia por ACP, así que se envía tal cual |
| Normal | `Esc` | cancelar el turno en curso |
| Normal | `↑↓` / `j k`, `PgUp/PgDn`, `Home/End` | scroll; `End` reanuda el autoscroll |
| Normal | `d` | dashboard lateral: mostrar / ocultar |
| Normal | `s` | vista completa de subagentes: abrir / cerrar |
| Normal | `H` | historial: sesiones anteriores del proyecto (`↑↓` elegir, `Enter` abrir, `r` reanudar en una pestaña nueva (`session/load`), `Esc` volver/cerrar; en la conversación `↑↓ PgUp/PgDn Home/End`) |
| Normal | `S` | ajustes: tema (oscuro/claro) e idioma (español/English); `↑↓` elegir, `Enter`/`←→` cambiar, `Esc` cerrar. Se guarda en `~/.config/copilot-cockpit/settings.toml` |
| Normal | `A` | allow all on/off (pide confirmación al activar; también en el modal de permiso) |
| Normal | `m` / `e` | selector de modelo / reasoning effort de la sesión (`session/set_config_option`) |
| Subagentes | `Enter` | abre el prompt con «Usa el subagente <nombre> para: » (si el editor está vacío) para lanzar el agente elegido |
| Subagentes | `↑↓`, `m` | elegir agente; `m` fija su modelo obligatorio 🔒 |
| Normal | `!` | saltar al siguiente proyecto con permiso pendiente |
| Normal | `?` | ayuda |
| Normal | `q` | salir (confirma si hay agentes trabajando) |
| Escribiendo | `Enter` | enviar |
| Escribiendo | `Alt+Enter` / `Shift+Enter` / `Ctrl+J` | nueva línea |
| Escribiendo | `←→`, `Home/End`, `Ctrl+W`, `Ctrl+U` | edición |
| Escribiendo | `Esc` | salir del prompt (conserva el borrador); `↑↓`/`PgUp/PgDn` hacen scroll |
| Permiso | `y` / `n` / `a` | aprobar / rechazar / aprobar siempre (`Esc` rechaza) |

`Shift+Enter` solo se distingue de `Enter` en terminales con protocolo de teclado ampliado; `Alt+Enter` y `Ctrl+J` funcionan en todas.


## Allow all, modelos y dashboard

- **Allow all**: envía `session/set_config_option allow_all on|off` a Copilot y, como respaldo, la TUI aprueba sola cada permiso (queda una nota `⚡ auto-aprobado` en la transcripción). Indicador `⚡ ALLOW ALL` parpadeante en la barra y `⚡` en la pestaña. Es por sesión.
- **Modelo / effort**: las listas salen de las `configOptions` de ACP; la UI se actualiza con `config_option_update`. `reasoning_effort` solo existe para modelos que lo soportan.
- **Modelo obligatorio por subagente** 🔒: se escribe en `~/.copilot/settings.json` → `subagents.agents.<nombre>` con `modelPolicy: "required"` (lo mismo que hace `/subagents`). Es **global del usuario**, no por proyecto. El `model:` del frontmatter de `.github/agents/*.md` es solo un valor por defecto: se muestra pero no se edita. Un cambio vale para subagentes que se lancen después; no está verificado que una sesión ya abierta relea el fichero.
- **Vista de subagentes** (`s`): todos los subagentes (custom + built-in + observados) con estado, modelo asignado vs usado (ámbar si difieren), ejecuciones, tool calls y tokens acumulados por tipo de agente.
- Cambios hechos desde la UI no se vuelven a escribir en el TOML. Ejemplo de config: `docs/config.example.toml`.
- **AI Credits**: se suman `copilotUsage.totalNanoAiu` de cada `session.usage_record` de `events.jsonl` (1e9 nano-AIU = 1 AI credit = 0,01 USD, según la doc de facturación de GitHub). Se muestran en la barra de info (sesión), en el sidebar y la vista de subagentes (por agente / tipo) y en el total. El USD es una estimación a tarifa de lista: no incluye créditos incluidos en tu plan ni descuentos.
