# TUI: teclas

`copilot-hive tui [--config FILE]`. Terminal mínima 60x20; el panel de agentes se oculta por debajo de 100 columnas.

| Modo | Tecla | Acción |
|------|-------|--------|
| Global | `Tab` / `Shift+Tab`, `Alt+1..9` | cambiar de proyecto dentro del grupo actual (el borrador se conserva por pestaña) |
| Normal | `}` / `{` | grupo siguiente / anterior (salta a su primera pestaña); sin efecto si no hay grupos. La barra muestra una fila con todos los grupos (nº de proyectos, `●` si trabajan, `!` rojo si hay un permiso pendiente; `!` salta a él) y, debajo, las pestañas del grupo actual (la barra se desplaza para mantener visible la activa) |
| Global | `Ctrl+C` | cancelar el turno en curso (`session/cancel`) |
| Normal | `Enter` / `i` | escribir un prompt |
| Normal | `f` | `/fleet`: abre el prompt con `/fleet ` ya escrito; tras el objetivo, Copilot reparte el trabajo en subagentes paralelos (visibles en el dashboard `d`/`s`). `/fleet` es un comando que Copilot anuncia por ACP, así que se envía tal cual |
| Normal | `Esc` | cancelar el turno en curso |
| Normal | `↑↓` / `j k`, `PgUp/PgDn`, `Home/End` | scroll del panel activo (`End` reanuda el autoscroll en la transcripción) |
| Normal | `]` / `[` | cambiar el foco entre los paneles visibles (transcripción, dashboard, Subagentes, MCP) |
| Ratón | Click izquierdo / rueda | enfocar un panel (sale del prompt si se pulsa en el sidebar) / scroll en el panel bajo el cursor |
| Normal | `d` | dashboard: mostrar / ocultar |
| Normal | `s` | vista completa de subagentes: abrir / cerrar |
| Normal | `H` | historial: sesiones anteriores del proyecto (`↑↓` elegir, `Enter` abrir, `r` reanudar en una pestaña nueva (`session/load`), `Esc` volver/cerrar; en la conversación `↑↓ PgUp/PgDn Home/End`) |
| Normal | `g` | sesión multiproyecto: elige varios proyectos abiertos (`↑↓` mover, `Espacio` elegir —el 1º es el `cwd`—, `g` elegir/quitar todo el grupo, `c` limpiar, `Enter` iniciar con ≥2, `Esc` cerrar). Abre una pestaña `a + b +N` en el grupo del 1º, con `copilot --acp --stdio --add-dir <resto>`: la sesión puede acceder a todas las carpetas, pero el contexto, el historial y los agentes/skills de proyecto son los del 1º. `/clear`, `/restart` y `H`→`r` conservan las carpetas extra |
| Normal | `o` | abrir carpeta: explorador de directorios para abrir una nueva pestaña con sesión ACP nueva |
| Abrir | `↑↓` / `j k`, `PgUp/PgDn`, `Home/End` | mover selección en el explorador |
| Abrir | `Enter` / `→` / `l` | entrar en la carpeta seleccionada (o `..`; los archivos se muestran atenuados y no se abren) |
| Abrir | `Backspace` / `←` / `h` | subir al directorio padre |
| Abrir | `.` | mostrar / ocultar archivos y carpetas ocultos |
| Abrir | `~` | ir a `$HOME` |
| Abrir | `o` / `Esc` | abrir la carpeta actual en una nueva pestaña / cerrar |
| Normal | `S` | ajustes: tema (oscuro/claro) e idioma (español/English); `↑↓` elegir, `Enter`/`←→` cambiar, `Esc` cerrar. Se guarda en `~/.config/copilot-hive/settings.toml` |
| Normal | `M` | servidores MCP del proyecto: `↑↓` elegir, `Espacio` activar/desactivar, `Enter` aplicar, `Esc` cancelar. Se guarda por carpeta en `~/.config/copilot-hive/mcp.toml` y se aplica reiniciando el proceso ACP con `--disable-mcp-server <n>` y recargando la sesión (`session/load`); allow all y modelo se restablecen. Lista los de `~/.copilot/mcp-config.json`, `.mcp.json` y `.github/mcp.json` |
| Normal | `p` | prompts guardados localmente (`~/.config/copilot-hive/prompts.toml`, compartidos entre proyectos): `↑↓` elegir, `Enter` aplicar al editor, `n` nuevo, `v` nuevo desde el portapapeles (luego solo pones nombre y `Enter`), `s` guardar borrador actual, `e` editar, `d` borrar, `Esc` cerrar; en el formulario `Ctrl+V` pega el portapapeles en el texto (solo macOS) |
| Normal | `A` | allow all on/off (pide confirmación al activar; también en el modal de permiso) |
| Normal | `m` / `e` | selector de modelo / reasoning effort de la sesión (`session/set_config_option`) |
| Subagentes | `Enter` | abre el prompt con «Usa el subagente <nombre> para: » (si el editor está vacío) para lanzar el agente elegido |
| Subagentes | `↑↓`, `m` | elegir agente; `m` fija su modelo obligatorio 🔒 |
| Normal | `!` | saltar al siguiente proyecto con permiso pendiente |
| Normal | `?` | ayuda |
| Normal | `C` / `R` | `/clear`: sesión ACP nueva en el mismo proyecto (contexto vacío) / `/restart`: reinicia el proceso y recarga la sesión (`session/load`) para aplicar cambios en agentes o MCP. Piden confirmación (`y`); allow all y modelo se restablecen. También se pueden escribir `/clear` o `/restart` como prompt (sin confirmación) |
| Normal | `t` | tutorial animado de 6 páginas (`←→`/`Enter` navegar, `1-6` ir a, `Esc` cerrar); no se abre solo |
| Normal | `x` | cerrar la pestaña actual (confirma si trabaja; la última no se cierra) |
| Normal | `q` | salir (confirma si hay agentes trabajando) |
| Escribiendo | `Enter` | enviar |
| Escribiendo | `Alt+Enter` / `Shift+Enter` / `Ctrl+J` | nueva línea |
| Escribiendo | `←→`, `Home/End`, `Ctrl+W`, `Ctrl+U` | edición |
| Escribiendo | `Ctrl+V` | pega una imagen (PNG) del portapapeles (solo macOS, vía `osascript`); se envía como bloque `image` de ACP con el siguiente prompt. `Backspace` con el editor vacío quita la última. Se ve como `📎 ×N` en el título del prompt |
| Escribiendo | `Ctrl+S` | guarda el borrador actual como prompt local reutilizable (abre el formulario con el texto precargado) |
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
