# Spike ACP: ¿subagentes como entidades propias?

Ejecutado con `copilot` 1.0.115 (`copilot --acp --stdio`), `cargo run` con el prompt por defecto
(dos subagentes `explore` en paralelo). Volcado crudo: `spike-output.jsonl` (gitignored; 142 mensajes).

## Conclusión

**Los subagentes NO son entidades de primer nivel en ACP.** No hay `session/new` hijo, ni un
`sessionUpdate` de tipo agente/subagente, ni mensajes de ciclo de vida propios. Todo llega como
`session/update` de la **misma sesión**, pero hay una **pista de atribución**: las tool calls que
ejecuta un subagente llevan `_meta["github.com/copilot"].agentId`. Es suficiente para reconstruir
un árbol de agentes en el dashboard, con limitaciones (abajo).

## Qué se observa

Conteo de `sessionUpdate`: `agent_message_chunk` 113, `usage_update` 7, `tool_call_update` 7,
`tool_call` 6, `config_option_update` 3, `available_commands_update` 2, `session_info_update` 1.
No hubo `session/request_permission` (los tools usados eran de solo lectura); el auto-approve
está implementado pero **no se ha ejercitado** en este run. Tampoco apareció `plan`.

1. **Lanzar un subagente = tool call normal** del agente padre (herramienta `task`, `kind: "other"`),
   con `rawInput.agent_type` y `rawInput.mode: "background"`. Sin `_meta.agentId`:

```json
{"sessionUpdate":"tool_call","toolCallId":"toolu_01E6…","title":"List top-level files","kind":"other","status":"pending",
 "rawInput":{"name":"list-top","agent_type":"explore","description":"List top-level files","mode":"background","prompt":"…"}}
```

2. **Su `tool_call_update` devuelve el `agent_id`** solo como texto libre:
   `"Agent started in background with agent_id: 146fa6f7-…"`. Parsear texto es frágil.

3. **Las tool calls del subagente** aparecen intercaladas en el mismo stream, planas (sin
   `parentToolCallId`), distinguidas solo por `_meta`:

```json
{"sessionUpdate":"tool_call","toolCallId":"call_24ef…","title":"List top-level repository entries","kind":"execute","status":"pending",
 "rawInput":{"command":"ls -1Ap …"},
 "_meta":{"github.com/copilot":{"agentId":"146fa6f7-b18c-4884-905d-66c8197b952a"}}}
```

   Las del agente raíz no llevan `_meta.agentId`.

4. **El padre recoge resultados con `read_agent`** (otra tool call normal, `rawInput.agent_id`),
   cuyo `tool_call_update` trae el estado como texto (`status: idle`, `elapsed`, `total_turns`).

## Implicaciones para el dashboard

- Agrupar tool calls por `_meta["github.com/copilot"].agentId` (ausente = agente raíz).
- Enlazar `agentId` con la tool call `task` que lo lanzó: no hay campo estructurado; hay que parsear
  el texto `agent_id: <uuid>` del `tool_call_update` (o casar por orden/`description`). Es el punto débil.
- Estado del subagente (running/idle/completed) no es estructurado: se infiere de las tool calls
  y del texto de `read_agent`. No hay `plan` ni progreso propio por subagente.
- Con `mode: "background"` el padre puede terminar el turno antes que los hijos; un turno
  (`stopReason: end_turn`) no implica que no queden subagentes activos.
- `_meta` es extensión propietaria en preview: aislar su lectura en `src/acp/`.

## Pendiente

- Ejercitar `session/request_permission` con un prompt que escriba/ejecute comandos y comprobar si
  el permiso de un subagente lleva `_meta.agentId`.
- Probar subagentes en modo síncrono y subagentes anidados (nietos).

## Reproducir

```
cargo run -- spike [--cwd DIR] [--out FILE] ["prompt"]
```

## Session-state (`~/.copilot/session-state/<sessionId>/`)

Investigado con dos sesiones del spike (CLI 1.0.115). **Conclusión: sí, expone la jerarquía
padre→subagente de forma estructurada y en tiempo real**, mucho mejor que ACP.

- `<sessionId>` = el `sessionId` de ACP. Contenido: `events.jsonl`, `workspace.yaml`,
  `checkpoints/`, `files/`, `research/`, `inuse.<pid>.lock`.
- `events.jsonl`: una línea JSON por evento, **append incremental** (observado: 3→8→40→63→67 líneas
  durante el run, sin reescritura). Apto para vigilar con `notify` (+ guardar offset de bytes y leer
  solo líneas completas nuevas; el fichero tiene permisos 0600).
- Envoltorio de cada evento: `{type, id, parentId, timestamp, data, agentId?}`.
  - `id`/`parentId`: cadena causal entre eventos (no es la jerarquía de agentes).
  - **`agentId` de nivel raíz del evento**: ausente = agente raíz; presente = evento del subagente
    (mensajes, turnos, `tool.execution_*`, `user.message`, `model_change`…). Es el mismo UUID que
    `_meta.agentId` de ACP.

### Eventos relevantes

| type | campos clave (`data`) | uso |
|---|---|---|
| `tool.execution_start` | `toolCallId`, `toolName`, `arguments` | `toolName=task` (args: `name`, `agent_type`, `description`, `prompt`, `mode`) |
| `subagent.started` | **`toolCallId`** (de la `task`), `agentType`, `agentName`, `agentDisplayName`, `agentDescription`, `executionMode`, `model` | **enlace tool call `task` → `agentId`** (campo raíz) |
| `subagent.configured` / `subagent.selected` | `model`, `multiTurn`, `tools` | el subagente empieza a trabajar |
| `subagent.completed` | `toolCallId`, `durationMs`, `totalTokens`, `totalToolCalls`, `model` | fin + métricas |
| `system.notification` | `kind: {type:"agent_idle", agentId, agentType, displayName, description}` | subagente background queda idle |
| `tool.execution_complete` | `toolCallId`, `success`, `causalDependencies` | en `read_agent`, `causalDependencies` = agentId leídos |
| `assistant.turn_start/_end`, `assistant.message` (`toolRequests[]`) | `turnId` | turnos por agente (según `agentId` raíz) |
| `user.message` | `parentAgentTaskId` | existe campo de padre; no verificado su valor |
| `session.usage_record`, `session.usage_checkpoint` | tokens/AIU | consumo |

Orden observado: `task` start → `subagent.started` → (`tool.execution_complete` de la `task`
inmediato en modo background) → `subagent.configured/selected` → eventos con `agentId` →
`subagent.completed` → `system.notification agent_idle` → `read_agent`.

### Campos de enlace padre/hijo

- `subagent.started.data.toolCallId` + `subagent.started.agentId` ⇒ **toolCallId del `task` ↔ agentId**
  (sin parsear texto). Con `data.agentType/agentDisplayName` para el nombre.
- `agentId` raíz en cada evento ⇒ pertenencia de tool calls/turnos al subagente.
- Fin: `subagent.completed` (mismo `toolCallId`/`agentId`); idle: `system.notification`.
- Padre inmediato explícito (`parentToolCallId`/`parentAgentId`) **no existe**: con un solo nivel el
  padre es el agente raíz. Para nietos habría que ver si la `task` lanzada por un subagente lleva
  `agentId` raíz (no probado; pendiente).
- Un `agent_idle`/`read_agent` no marca "terminado" definitivo: el subagente es multi-turno
  (`resumable`/`multiTurn`).

### Veredicto

Mejor fuente que ACP para el dashboard de jerarquía/estado de subagentes: estructurada, con
inicio/fin y métricas, y escrita incrementalmente. Limitaciones: formato interno no documentado
(puede cambiar entre versiones), no sirve para permisos pendientes ni para enviar prompts (eso sigue
siendo ACP), y los eventos contienen prompts/contenido completo (no commitear ni loguear). Propuesta:
ACP para control + `notify` sobre `events.jsonl` para jerarquía; aislar el parser en `src/`
módulo propio.
