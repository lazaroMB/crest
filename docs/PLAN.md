# Plan: agente de código tipo Claude Code en Rust

Oct 17, 2026 · @Lazaro Millares

## Contexto y objetivo

Construir en Rust una app de escritorio tipo Claude Code: se abre a pantalla completa, se ve como una terminal y trabaja sobre un directorio (el workspace). La diferencia está en el timeline: en lugar de texto plano, el agente muestra componentes ricos (gráficas con vista de tabla, diagramas Mermaid renderizados, markdown con buen formato).

Los LLM son exclusivamente de Anthropic vía su API. La app permite elegir qué modelo usa cada tipo de tarea. Las búsquedas de archivos y contenido se delegan a `ripgrep` y `fzf`. Se configura y extiende con Lua (plugins, keymaps estilo Neovim). La arquitectura es hexagonal (puertos y adaptadores).

Plataformas objetivo: **Linux y macOS**. Windows queda fuera de alcance.

**Cómo usar este plan (para el agente ejecutor):** ejecuta las fases en orden. No pases a la siguiente fase hasta que se cumplan todos sus criterios de aceptación y `cargo check`, `cargo clippy -- -D warnings` y `cargo test` pasen en todo el workspace. Lee la sección final de reglas antes de empezar.

## Decisiones técnicas y stack

El stack es GPUI para la UI, Anthropic para los modelos y binarios externos (`rg`, `fzf`) para búsqueda, todo detrás de puertos. Las versiones exactas las fija el agente consultando crates.io y docs.rs al momento de implementar.

| Área | Elección | Notas |
| --- | --- | --- |
| Lenguaje y build | Rust estable, edición 2024, Cargo workspace | Un crate por capa (ver arquitectura) |
| Runtime async | `tokio` | GPUI tiene su propio executor; la comunicación con el core va por canales |
| UI | `gpui` + `gpui-component` | Pre-1.0: dependencias por git, fijar commit en `Cargo.lock` |
| Gráficas, tablas, markdown | `gpui-component` (Chart, DataTable, Markdown) | Resaltado de código vía Tree-sitter incluido |
| Mermaid | `rusty-mermaid` (feature `gpui`) | Alternativa: `mermaid-rs-renderer` o `ariel-rs` a SVG |
| Cliente LLM | Adaptador propio sobre `reqwest` contra la Messages API | No hay SDK oficial de Anthropic para Rust; ver selección de modelos |
| Streaming | SSE con `reqwest-eventsource` o parser propio | Necesario para mostrar texto y tool calls en vivo |
| MCP | `rmcp` (cliente) | Fase posterior |
| Búsqueda de contenido | `ripgrep` (`rg --json`) | Binario externo requerido |
| Búsqueda difusa de archivos | `rg --files` + `fzf --filter` | Binario externo requerido; modo no interactivo |
| Lectura de árbol | `rg --files` (respeta `.gitignore`) | Fallback: crate `ignore` si `rg` no existe |
| Vigilancia de archivos | `notify` | Refrescar árbol y contexto |
| Diffs | `similar` | Mostrar cambios antes de aplicar ediciones |
| Lua | `mlua` (Lua 5.4, features `async`, `serialize`, `vendored`) | Config y plugins |
| Persistencia de sesiones | `rusqlite` o `sqlx` con SQLite | Historial y estado local |
| Config | `~/.config/crest/init.lua` + `<workspace>/.crest/init.lua` | El del workspace sobreescribe al global |
| Errores | `thiserror` en librerías, `anyhow` solo en el binario |  |
| Observabilidad | `tracing` + `tracing-opentelemetry` + `opentelemetry-langfuse` | Opcional, detrás de un puerto |
| Tests | `insta` (snapshots), `wiremock` (API falsa), `tempfile` |  |

## Arquitectura hexagonal

El dominio y los casos de uso no conocen GPUI, HTTP, Lua ni procesos externos. Todo eso vive en adaptadores que implementan traits (puertos) definidos en el núcleo; el binario es el único lugar que los conecta.

_[diagrama embebido: arquitectura hexagonal · núcleo, puertos y adaptadores]_

Las flechas indican quién llama a quién; las dependencias de código apuntan siempre hacia el dominio.

### Estructura del workspace

```text
crates/
  domain/        tipos y reglas puras: Session, Turn, Block, ModelPolicy,
                 Workspace, PermissionPolicy. Sin tokio, sin I/O (solo serde, thiserror)
  app/           casos de uso + definición de puertos (traits async)
  adapters/
    anthropic/   LlmPort y ModelCatalogPort sobre la Messages API
    search/      FileSearchPort y ContentSearchPort con rg y fzf
    fs/          FileSystemPort con guardas de workspace
    shell/       ShellPort (tokio::process)
    store/       SessionStorePort sobre SQLite
    lua/         PluginHostPort con mlua
    mcp/         McpPort con rmcp (fase posterior)
    telemetry/   TelemetryPort con tracing / Langfuse
  ui/            GPUI: vistas, bloques, input modal
  bin/           composition root: lee config, construye adaptadores, arranca la UI
```

Reglas de dependencia: `domain` no depende de ningún crate del proyecto; `app` solo de `domain`; cada adaptador y `ui` solo de `app` y `domain`; únicamente `bin` depende de todo. `make arch` lo verifica al cerrar cada fase.

### Puertos

| Puerto | Responsabilidad | Adaptador inicial |
| --- | --- | --- |
| `LlmPort` | Enviar mensajes con tools y recibir un stream de eventos (texto, tool\_use, usage) | `adapters/anthropic` |
| `ModelCatalogPort` | Listar modelos disponibles para validar la configuración | `adapters/anthropic` (`GET /v1/models`) |
| `FileSearchPort` | Encontrar archivos por nombre con coincidencia difusa | `rg --files` + `fzf --filter` |
| `ContentSearchPort` | Buscar texto o regex dentro de archivos | `rg --json` |
| `FileSystemPort` | Leer, listar y escribir dentro del workspace | `std::fs` / `tokio::fs` con guardas |
| `ShellPort` | Ejecutar comandos con aprobación | `tokio::process` |
| `ApprovalPort` | Pedir permiso al usuario antes de acciones con efectos | UI (diálogo modal) |
| `EventSink` | Emitir eventos de dominio hacia la UI | canal `tokio::sync::mpsc` → GPUI |
| `SessionStorePort` | Guardar y reanudar sesiones | SQLite |
| `PluginHostPort` | Cargar config, keymaps, tools y bloques de Lua | `mlua` |
| `TelemetryPort` | Trazas por turno y por llamada al modelo | `tracing` (+ Langfuse opcional) |
| `McpPort` | Exponer tools de servidores MCP | `rmcp` (fase 8) |

Cada puerto tiene un adaptador falso en `app/src/testing/` para probar los casos de uso sin red, sin `rg` y sin UI.

## Selección de modelos por rol

Cada llamada al LLM pide un **rol**, no un modelo; una política pura del dominio (`ModelPolicy`) resuelve el rol a un modelo concreto según la configuración. Así se cambia qué modelo hace qué cosa sin tocar código.

| Rol | Para qué se usa | Modelo por defecto |
| --- | --- | --- |
| `main` | Conversación y loop agente principal (leer, editar, usar tools) | `claude-sonnet-5-5` |
| `plan` | Planes, diseño de arquitectura, refactors grandes | `claude-opus-5-5` |
| `explore` | Subagentes de búsqueda y lectura amplia del proyecto | `claude-haiku-5-5` |
| `compact` | Resumir el historial cuando se acerca al límite de contexto | `claude-haiku-5-5` |
| `title` | Título de la sesión y nombres cortos | `claude-haiku-5-5` |
| `component` | Generar datos estructurados para bloques (gráficas, Mermaid, tablas) | `claude-sonnet-5-5` |
| `deep` | Opcional, tareas muy difíciles bajo demanda | `claude-fable-5-1` |

Los ids son valores por defecto de configuración, no constantes en código. Al arrancar, el adaptador consulta `GET /v1/models`; si un rol apunta a un modelo que no existe, la app lo avisa en la barra de estado y usa el primer fallback válido.

### Orden de resolución

1. Override del turno: comando `/model opus` o un atajo de teclado.
2. Rol fijado por una tool o un plugin (por ejemplo, un subagente pide `explore`).
3. Config del workspace: `<workspace>/.crest/init.lua`.
4. Config global: `~/.config/crest/init.lua`.
5. Defaults incluidos en el binario.

### Qué define cada rol

`ModelSpec { id, max_tokens, thinking, fallbacks }`. `thinking` controla el razonamiento extendido (desactivado o con presupuesto de tokens). `fallbacks` es la lista a probar ante errores 529 (sobrecarga) o 5xx, con reintento y backoff exponencial antes de cambiar de modelo.

```lua
-- ~/.config/crest/init.lua
app.models.roles = {
  main    = { id = "claude-sonnet-5-5", max_tokens = 16000,
              thinking = { budget = 8000 }, fallbacks = { "claude-haiku-5-5" } },
  plan    = { id = "claude-opus-5-5", max_tokens = 32000, thinking = { budget = 16000 } },
  explore = { id = "claude-haiku-5-5", max_tokens = 8000 },
  compact = { id = "claude-haiku-5-5", max_tokens = 4000 },
}
app.models.aliases = { opus = "claude-opus-5-5", sonnet = "claude-sonnet-5-5", haiku = "claude-haiku-5-5" }
```

### Costo y caché

- Registrar `usage` (input, output, cache read/write) de cada respuesta y mostrar el acumulado por sesión en la barra de estado.
- Los precios por modelo van en config, porque cambian; nunca hardcodeados.
- Usar prompt caching en el system prompt y en las definiciones de tools, que se repiten en cada turno.
- Tope opcional de gasto por sesión: al alcanzarlo, la app pausa y pide confirmación.

La API key se lee de `ANTHROPIC_API_KEY` o del llavero del sistema (crate `keyring`); nunca se guarda en `init.lua`.

## Tools de archivos con ripgrep y fzf

Toda búsqueda pasa por `rg` y `fzf` como procesos externos, siempre sin shell y con el workspace como directorio de trabajo. `fzf` se usa en modo no interactivo (`--filter`), que imprime las coincidencias ordenadas por puntuación y termina.

| Tool | Entrada (JSON) | Implementación |
| --- | --- | --- |
| `list_files` | `glob?`, `limit` (def. 500) | `rg --files --hidden --glob '!.git' [--glob <glob>]` |
| `find_file` | `query`, `limit` (def. 20) | `rg --files --hidden --glob '!.git'` → stdin de `fzf --filter <query> --scheme=path`; tomar las primeras `limit` líneas |
| `grep` | `pattern`, `path?`, `glob?`, `fixed?`, `ignore_case?`, `context` (def. 2), `limit` (def. 100) | `rg --json -n -C <context> [-F] [-i] [--glob <glob>] -- <pattern> [path]`; parsear eventos `match` y `context` línea por línea |
| `tree` | `path?`, `depth` (def. 3) | Árbol construido a partir de `rg --files` |
| `read_file` | `path`, `offset?`, `limit?` (def. 2000 líneas) | `tokio::fs`; devolver con números de línea; rechazar binarios y archivos > 2 MB salvo rango explícito |
| `edit_file` | `path`, `old`, `new` | Reemplazo exacto que debe coincidir una sola vez; diff con `similar`; requiere aprobación |
| `write_file` | `path`, `content` | Crear o sobrescribir; diff si existe; requiere aprobación |
| `run_command` | `command`, `args[]`, `timeout_s` (def. 120) | `tokio::process` con `cwd` = workspace; requiere aprobación |

### Detalles de implementación

- Encadenar `rg` → `fzf` con dos `tokio::process::Command` y un pipe; nunca `sh -c`.
- Poner `--` antes del patrón de `rg` para que un patrón que empieza con `-` no se lea como flag.
- Al arrancar, verificar `rg --version` y `fzf --version`. Si falta alguno, mostrar un bloque de error con cómo instalarlo; si falta `fzf`, `find_file` cae a coincidencia por subcadena sobre `rg --files`.
- Truncar cada resultado a un presupuesto de tokens configurable y terminar con una línea "N resultados más; refina la búsqueda".
- Timeout de 30 s por búsqueda; cancelar el proceso si el usuario interrumpe el turno.
- La UI usa los mismos puertos para su propio selector de archivos (`<leader>ff`), así que el usuario ve los mismos resultados que el agente.

### Seguridad del workspace

- El root se canonicaliza al abrir el workspace y no cambia durante la sesión.
- Cada ruta recibida se une al root y se canonicaliza (el directorio padre, si el archivo aún no existe). Si el resultado no empieza con el root, la tool falla con `PathOutsideWorkspace`. Esto también bloquea symlinks que apuntan fuera.
- Lista de rutas sensibles negadas por defecto para lectura (`.env*`, `*.pem`, `id_rsa*`, `.git/`), configurable desde Lua.
- Modos de permiso, alternables con un atajo: **ask** (pedir aprobación para escribir o ejecutar), **auto-edit** (editar sin preguntar, ejecutar preguntando) y **plan** (solo lectura).
- Cada aprobación muestra el diff o el comando exacto; las reglas de "permitir siempre" se guardan por workspace.

## Bloques de UI, plugins Lua y keymaps

El timeline es una lista de bloques tipados del dominio; el agente crea los bloques ricos llamando a tools de componentes, y la UI decide cómo dibujar cada tipo.

### Bloques

| Bloque | Origen | Cómo se ve |
| --- | --- | --- |
| `Text` | Respuesta del modelo (markdown en streaming) | Markdown de `gpui-component` |
| `Thinking` | Razonamiento extendido | Colapsado por defecto |
| `ToolCall` | Cualquier tool | Una línea con nombre, argumentos clave y estado; expandible |
| `Diff` | `edit_file`, `write_file` | Diff coloreado con botones aprobar / rechazar |
| `Chart` | Tool `show_chart` | Gráfica `Chart`; la tecla `t` alterna con `DataTable` de los mismos datos |
| `Mermaid` | Tool `show_mermaid` | Diagrama renderizado con pan y zoom; si no parsea, código fuente + error |
| `Table` | Tool `show_table` | `DataTable` virtualizada, ordenable |
| `Markdown` | Tool `show_markdown` o al escribir un `.md` | Vista de documento con tipografía cuidada; alternable a fuente |
| `Error` | Fallos de API o tools | Mensaje y acción sugerida |
| `Custom` | Plugins Lua | Árbol declarativo (ver abajo) |

Las tools de componentes reciben JSON validado con un schema derivado por `schemars` (por ejemplo, `ShowChart { title, kind, labels, series }`). Devuelven al modelo una confirmación corta, no los datos, para no gastar contexto. El system prompt explica cuándo usar cada una.

### Pantalla

- Ventana a pantalla completa, fuente monoespaciada, tema oscuro por defecto.
- Timeline virtualizado arriba, input de varias líneas abajo, barra de estado con modelo activo, modo de permisos, costo de la sesión y workspace.
- Panel lateral opcional con el árbol del workspace (`<leader>e`).

### Keymaps estilo Neovim

Tres modos: **insert** (escribir el prompt), **normal** (navegar el timeline) y **command** (`:` para comandos). Un parser de secuencias con `timeoutlen` configurable traduce teclas a acciones de GPUI.

| Modo | Tecla | Acción |
| --- | --- | --- |
| insert | `Esc` | Pasar a normal |
| insert | `Enter` / `Shift+Enter` | Enviar / nueva línea |
| normal | `i` | Volver a insert |
| normal | `j` / `k` | Bloque siguiente / anterior |
| normal | `gg` / `G` | Primer / último bloque |
| normal | `Enter` | Expandir o colapsar bloque |
| normal | `t` | Alternar gráfica y tabla |
| normal | `y` | Copiar el contenido del bloque |
| normal | `<leader>ff` | Buscar archivo (fzf) |
| normal | `<leader>fg` | Buscar texto (rg) |
| normal | `<leader>m` | Elegir modelo para el siguiente turno |
| normal | `<leader>p` | Ciclar modo de permisos |
| cualquiera | `Ctrl+C` | Interrumpir el turno en curso |

### API de Lua

```lua
local app = require("app")

app.opt.leader = " "
app.opt.theme = "tokyonight"

app.keymap.set("n", "<leader>r", "session.resume")
app.keymap.set("n", "<leader>x", function() app.cmd("clear") end)

app.commands.register("review", function(args)
  app.send("Revisa los cambios sin commitear y lista riesgos", { role = "plan" })
end)

app.tools.register({
  name = "todo_list",
  description = "Muestra tareas pendientes del proyecto",
  schema = { type = "object", properties = { items = { type = "array" } } },
  handler = function(input)
    return app.ui.block("todo", app.ui.list(input.items))
  end,
})

app.on("before_tool", function(ev)
  if ev.name == "run_command" and ev.input.command == "rm" then return { deny = true } end
end)
```

Reglas del host Lua:

- Lua no toca GPUI. Los bloques `Custom` se describen con un árbol de primitivas (`text`, `row`, `col`, `list`, `table`, `chart`, `code`, `markdown`) que la UI convierte en componentes.
- La VM de Lua vive en su propio hilo; el resto de la app le habla por canales.
- Sin `io`, `os.execute` ni `require` de rutas arbitrarias por defecto. Las capacidades extra (red, shell) se declaran en el plugin y se aprueban una vez.
- Límite de instrucciones por llamada para que un plugin colgado no congele la app.
- Plugins en `~/.config/crest/plugins/<nombre>/init.lua`, cargados en orden alfabético, más los listados en `app.plugins`.

## Flujo investigativo (X-Workflow para software)

El agente sigue las etapas del [X-Workflow de Hashlabs](https://www.hashlabs.com/guides/en/methodology/X-Workflow.html) (descubrimiento, definición, búsqueda de soluciones y especificación), implementadas como un grafo de graph-flow que corre dentro de la conversación. Cada nodo es un `Task`; los gates son `WaitForInput` y el usuario responde en el mismo input del chat.

_[diagrama embebido: flujo investigativo · 4 etapas, 14 nodos, 5 gates]_

El principio clave del método se vuelve una regla del grafo: si el Spec se atasca, falta algo en una etapa anterior, y el flujo regresa ahí en lugar de inventar.

### Nodos

| # | Nodo | Qué hace en un proyecto de software | Bloques que muestra | Rol de modelo | Gate |
| --- | --- | --- | --- | --- | --- |
| 1 | Intake | Reformula el pedido, lo clasifica (feature, bug, rendimiento, refactor) y pregunta lo que falte | Text | `main` | Sí |
| 2 | Observación | Recorre el repo: árbol, archivos clave, tests, docs, `git log`; dibuja la arquitectura actual | Mermaid, Table, Sources | `explore` |  |
| 3 | Datos | Reúne evidencia: historial de cambios, logs, métricas, issues (vía MCP), documentación de librerías en la web | Chart, Table, Sources | `explore` |  |
| 4 | Análisis | Contrasta las ideas con los datos: perfiles, benchmarks, conteos, líneas de tiempo | Chart, Table | `main` |  |
| 5 | Hipótesis | Escribe hipótesis de una oración, diagrama de causas (Ishikawa en Mermaid) y tabla priorizada | Mermaid, Table | `plan` | Sí |
| 6 | Experimentos | Diseña y corre protocolos reproducibles: tests que fallan, scripts de benchmark, pasos de reproducción | ToolCall, Chart, Diff | `main` |  |
| 7 | Definición y restricciones | Define el problema en una oración medible (puede haber varias, una por Spec) y la tabla de restricciones | Markdown, Table | `plan` | Sí |
| 8 | Investigación | Busca soluciones previas: librerías, artículos, issues, patrones ya presentes en el repo | Sources, Table | `explore` |  |
| 9 | Lluvia de ideas | Genera opciones sin filtrar | Table | `plan` |  |
| 10 | Modelado | Descarta lo que viola restricciones; matriz de decisión y diagrama de caja negra de la opción elegida | Table, Mermaid | `plan` | Sí |
| 11 | Prototipo | Spike opcional en un `git worktree` temporal que prueba que la pieza difícil se puede construir | Diff, ToolCall | `main` |  |
| 12 | Spec | Redacta el Spec con la plantilla de la siguiente sección | Markdown | `plan` |  |
| 13 | Plan | Divide el Spec en tareas con dependencias, archivos y pruebas | Mermaid, Table | `plan` |  |
| 14 | Revisión crítica | Un subagente con contexto limpio busca huecos; si los hay, regresa a la etapa donde falta información | Markdown | `plan` | Sí |

Un ejemplo del criterio de buena definición, tomado del método: "a 500 requests por segundo el tiempo de respuesta sube de 10 ms a 500 ms" es buena; "el tiempo de respuesta depende de la carga" es mala.

### Estado del flujo

El `Context` de graph-flow guarda estructuras tipadas, no solo mensajes:

- `Evidence { id, claim, source: Source, collected_at }`, donde `Source` es un archivo con líneas, un commit, una URL o la salida de un comando.
- `Hypothesis { id, statement, causes, priority, status }`.
- `Constraint { id, kind, statement, test }`.
- `Decision { id, chosen, discarded, rationale, evidence_ids }`.
- `journal: Vec<JournalEntry>`, el diario de investigación que el método pide llevar; se escribe en cada nodo.

Toda afirmación del Spec debe apuntar a ids de evidencia; la revisión crítica rechaza afirmaciones sin fuente.

### Herramientas y bloques adicionales

- **Investigación web**: las tools de búsqueda y lectura web del servidor de Anthropic (web search y web fetch), con citas; activables por rol.
- **Datos del repo**: `git log`, `git blame` y `git diff` vía `ShellPort` en modo solo lectura (sin aprobación).
- **Bloque `Sources`**: lista de fuentes del nodo; cada cita del texto enlaza a su entrada (archivo:línea abre el archivo, URL abre el navegador).
- **Bloque `Image`**: capturas, gráficas exportadas o imágenes de la web, con su fuente.
- **Bloque `StageHeader`**: marca en el timeline cuando el flujo entra a una etapa nueva, con progreso (por ejemplo, 5/14).

### Persistencia y control

- graph-flow trae storage en memoria y PostgreSQL; para la app local se implementa su trait `SessionStorage` sobre SQLite en `adapters/store`.
- Cada nodo terminado es un checkpoint: cerrar la app y reabrirla retoma en el nodo pendiente.
- Comandos en el chat: `/stage` (ver dónde va), `/back <nodo>` (regresar a mano), `/skip` (saltar un nodo opcional) y `/abort`.

## Salida: Spec + Implementation Plan

El flujo investigativo termina escribiendo una carpeta por Spec dentro del workspace; un Spec resuelve un solo problema, y si la definición produjo varios, se generan varias carpetas.

```text
.crest/specs/0007-cache-de-sesiones/
  spec.md        documento para personas
  plan.md        plan legible
  plan.json      plan que consume el flujo de implementación
  journal.md     diario de investigación de todo el flujo
  sources.json   fuentes citadas (archivos, commits, URLs, comandos)
  evidence/      imágenes, datos de gráficas (CSV), salidas de comandos
```

### Plantilla de `spec.md`

Las secciones siguen las del método; las tres últimas son la adaptación a software.

1. **Problema**: la definición de una oración, medible.
2. **Antecedentes**: resumen de la investigación, intentos previos y lo que se descartó, con citas.
3. **Hipótesis**: la oración en la que se basa la solución.
4. **Propuesta de solución**: descripción corta y diagrama de caja negra (Mermaid) con las piezas con las que interactúa.
5. **Especificación funcional**: estados, flujos, pantallas, comandos y contratos de API; diagramas de secuencia y de estados.
6. **Especificación técnica**: módulos y archivos a tocar, tipos y firmas, algoritmos, migraciones, configuración; suficiente para que otra persona lo implemente.
7. **Restricciones**: tabla de plataformas, rendimiento, compatibilidad y seguridad, cada una con su prueba.
8. **Protocolo de pruebas**: los tests y experimentos del descubrimiento que demuestran el problema y que deben pasar al final.
9. **Fuera de alcance** y **riesgos**.
10. **Fuentes**.

### Contenido de `plan.json`

```json
{
  "spec": "0007-cache-de-sesiones",
  "tasks": [
    {
      "id": "T1",
      "title": "Agregar SessionCache al puerto de storage",
      "description": "...",
      "depends_on": [],
      "files": ["crates/app/src/ports/store.rs"],
      "acceptance": [
        { "kind": "test", "ref": "cargo test -p app session_cache" }
      ],
      "role": "main",
      "risk": "low"
    }
  ],
  "hypothesis_tests": [{ "kind": "command", "ref": "make bench-sessions", "expect": "p95 < 20ms" }],
  "constraint_tests": [{ "constraint": "C2", "kind": "test", "ref": "cargo test -p store offline" }]
}
```

- El schema de `plan.json` vive en `domain` como tipos Rust con `serde` y `schemars`; el nodo Plan genera el JSON con salida estructurada y se valida antes de escribir.
- `plan.md` se genera desde `plan.json`, no al revés, para que nunca diverjan.
- El nodo Plan dibuja el grafo de dependencias de tareas en Mermaid y lo rechaza si tiene ciclos.

## Flujo de implementación

Es un segundo grafo de graph-flow que recibe un `plan.json` aprobado y lo ejecuta tarea por tarea. Siguiendo el método, aquí el agente deja de ser creativo: ejecuta, verifica y anota en el diario cualquier desvío.

1. **Cargar plan**: valida `plan.json`, crea una rama `spec/0007-cache-de-sesiones` y ordena las tareas por dependencias.
2. **Implementar tarea** (rol de la tarea, por defecto `main`): solo puede tocar los archivos listados; tocar otros requiere aprobación y queda en el diario.
3. **Pruebas y corrección**: corre los criterios de aceptación de la tarea; si fallan, corrige hasta 3 veces y luego se detiene a preguntar.
4. **Commit** por tarea, con el id de la tarea en el mensaje.
5. **Verificar hipótesis**: al terminar todas, corre `hypothesis_tests` y `constraint_tests` y muestra los resultados contra lo esperado en una tabla y, si hay mediciones, en una gráfica.
6. **Reporte y futuro**: escribe `report.md` en la carpeta del Spec con lo logrado, lo que falló, desvíos del plan e ideas para siguientes Specs.

Gates del flujo: antes de empezar (plan y rama), cuando una tarea agota sus reintentos y antes de cerrar con el reporte. Una desviación grande (el plan resulta inviable) no se resuelve improvisando: el flujo se detiene y ofrece volver al nodo Spec del flujo investigativo.

## Build y dependencias externas

En la fase 0 el agente crea un `Makefile` y dos scripts de shell que cumplan los requisitos de esta sección: `scripts/install-deps.sh` instala lo que falta según el sistema y el gestor de paquetes, y `scripts/check-arch.sh` hace cumplir las reglas de la arquitectura hexagonal. Ambos deben funcionar en Linux y macOS, con Bash 3.2 (el de macOS) como mínimo.

### Dependencias externas

SQLite se compila dentro del binario (`rusqlite` con la feature `bundled`) y TLS usa `rustls`, así que no hacen falta `libsqlite3` ni OpenSSL del sistema.

| Componente | Para qué | Linux | macOS | Versión mínima |
| --- | --- | --- | --- | --- |
| Rust (rustup, rustfmt, clippy) | Compilar | rustup | rustup | estable actual |
| Compilador C, `pkg-config`, `cmake`, `clang` | Crates con código nativo | paquete de la distro | Xcode Command Line Tools |  |
| Xcode completo (compilador Metal) | Shaders de GPUI |  | App Store |  |
| `libxkbcommon`, `libxkbcommon-x11` | Teclado (GPUI) | paquete de la distro |  |  |
| `wayland`, `libxcb`, `libx11-xcb` | Ventanas (GPUI) | paquete de la distro |  |  |
| `fontconfig`, `freetype` | Fuentes (GPUI) | paquete de la distro |  |  |
| Vulkan loader + driver de la GPU | Render (GPUI en Linux) | paquete de la distro |  |  |
| `ripgrep` (`rg`) | Tools `grep`, `list_files`, `find_file` | paquete o binario | Homebrew o binario | 13.0 |
| `fzf` | Tool `find_file` (`--filter --scheme=path`) | paquete o binario | Homebrew o binario | 0.36 |
| `git` | Datos del repo, ramas del flujo de implementación | paquete de la distro | Command Line Tools |  |
| `jq` | `scripts/check-arch.sh` | paquete de la distro | Homebrew |  |
| `cargo-insta`, `cargo-nextest` | Snapshots y tests más rápidos (opcional) | `cargo install` | `cargo install` |  |

### Requisitos de `scripts/install-deps.sh`

- Detecta el sistema con `uname` y `/etc/os-release`, y el gestor de paquetes: apt, dnf, yum, pacman, zypper, apk, xbps o Homebrew.
- Instala los paquetes de su lista en bloque; si un nombre no existe en esa versión de la distro, reintenta uno por uno y reporta los que fallaron.
- Si `rg` o `fzf` faltan o son más viejos que el mínimo (el `fzf` de Ubuntu 22.04 es 0.29), descarga el binario oficial de GitHub a `~/.local/bin`.
- En macOS verifica las Command Line Tools, el compilador Metal y Homebrew, y explica cómo instalarlos si faltan.
- Instala Rust con rustup si no existe (salvo `--no-rust`).
- Opciones: `--check` (solo verifica; sale con 1 si falta algo), `--yes`, `--dev`, `--no-rust`.

### Objetivos del Makefile

| Objetivo | Qué hace |
| --- | --- |
| `make deps` / `make deps-dev` | Corre el script de instalación (con herramientas de desarrollo en el segundo) |
| `make doctor` | Verifica dependencias sin instalar |
| `make build` / `make release` | Compila el workspace o el binario optimizado |
| `make run WORKSPACE=<dir>` | Abre la app sobre un directorio |
| `make run-headless PROMPT="..."` | Corre un prompt sin UI (fase 3) |
| `make test` / `make test-live` | Tests sin red / tests contra la API real |
| `make fmt`, `make lint`, `make arch` | Formato, clippy estricto y reglas de arquitectura |
| `make check` | Lo que debe pasar para cerrar una fase |
| `make ci` | `doctor` + `check` |
| `make install` | Copia el binario a `~/.local/bin` |

`scripts/check-arch.sh` clasifica cada crate por su ruta (`crates/domain`, `crates/app`, `crates/adapters/*`, `crates/ui`, `crates/bin`), revisa sus dependencias con `cargo metadata` y falla si alguna apunta hacia afuera del dominio o si `domain` o `app` usan crates de I/O (`tokio`, `reqwest`, `gpui`, `mlua`, etc.). El paquete de `crates/app` debe llamarse `application` para no chocar con el nombre del binario.

## Fases de ejecución

Son once fases; el núcleo y el agente funcionan en modo headless (fase 3) antes de escribir una sola vista, para que la UI se construya sobre algo probado.

El desglose en tareas paralelas, las reglas de commits y los prompts para los agentes están en Implementation plan.

### Fase 0 — Esqueleto del workspace

Crear el Cargo workspace con los crates de la arquitectura, vacíos pero compilando, y CI con `fmt`, `clippy` y `test`.

- [ ] `cargo build` y `cargo test` pasan con todos los crates.
- [ ] `Makefile`, `scripts/install-deps.sh` y `scripts/check-arch.sh` en el repo; `make doctor` y `make arch` pasan.
- [ ] `scripts/install-deps.sh --check` probado en Ubuntu, Fedora, Arch y macOS (en CI con contenedores donde se pueda).
- [ ] CI corre `make ci`.

### Fase 1 — Dominio, puertos y fakes

Tipos del dominio (`Session`, `Turn`, `Message`, `Block`, `ModelSpec`, `ModelPolicy`, `PermissionPolicy`, `WorkspacePath`), traits de todos los puertos y un adaptador falso por puerto. Caso de uso `run_turn`: el loop agente que llama al modelo, ejecuta tools, devuelve resultados y repite hasta `end_turn`.

- [ ] Tests de `ModelPolicy`: los 5 niveles del orden de resolución y los fallbacks.
- [ ] Tests de `WorkspacePath`: rechaza `..`, rutas absolutas fuera del root y symlinks que escapan.
- [ ] Test de `run_turn` con un `LlmPort` falso que pide dos tools y luego responde; los eventos emitidos salen en el orden correcto.
- [ ] Interrumpir un turno cancela la tool en curso y deja la sesión consistente.

### Fase 2 — Adaptador de Anthropic

`LlmPort` sobre la Messages API con streaming SSE, tools, razonamiento extendido, `usage`, prompt caching, reintentos con backoff y fallback de modelo. `ModelCatalogPort` con `GET /v1/models`.

- [ ] Tests con `wiremock` para: texto en streaming, `tool_use` con input JSON parcial, error 529 con fallback, error 401 con mensaje claro.
- [ ] Un test real contra la API, ignorado por defecto y activado con `ANTHROPIC_LIVE_TEST=1`.
- [ ] Ninguna clave aparece en logs ni en trazas.

### Fase 3 — Tools de archivos y modo headless

Adaptadores de búsqueda (`rg`, `fzf`), sistema de archivos con guardas, `edit_file` y `write_file` con diff, `run_command`, y aprobaciones. El binario acepta `--headless "<prompt>" --workspace <dir>` e imprime los eventos en la terminal, con aprobación `y/n` por stdin.

- [ ] Tests de integración sobre un workspace temporal con `tempfile`: `find_file`, `grep` (con y sin contexto), `list_files` respeta `.gitignore`, `read_file` con rango.
- [ ] Un patrón de `grep` que empieza con `-` funciona y no se interpreta como flag.
- [ ] Escribir fuera del workspace falla con `PathOutsideWorkspace`.
- [ ] Demo: en headless, el agente encuentra una función, la edita tras aprobación y explica el cambio.

### Fase 4 — UI base con GPUI

Ventana a pantalla completa, timeline con bloques `Text`, `Thinking`, `ToolCall`, `Diff` y `Error`, input de varias líneas, barra de estado, streaming en vivo, `Ctrl+C` para interrumpir y diálogo de aprobación.

- [ ] La UI solo consume `EventSink` y llama a puertos de entrada; no importa adaptadores.
- [ ] El timeline se mantiene fluido con 1000 bloques (lista virtualizada).
- [ ] Abrir la app con `crest <dir>` fija el workspace; sin argumento usa el directorio actual.

### Fase 5 — Bloques ricos

Tools `show_chart`, `show_table`, `show_mermaid` y `show_markdown` con sus bloques; alternar gráfica y tabla; render de Mermaid con fallback a código.

- [ ] Snapshots con `insta` de los JSON schemas de cada tool.
- [ ] Un Mermaid inválido muestra el código y el error, nunca hace panic.
- [ ] Prompt de demo: "analiza el tamaño de los archivos por extensión y muéstralo en una gráfica" produce un `Chart` con su tabla.

### Fase 6 — Lua: config, keymaps y plugins

Host `mlua` en su hilo, carga de config global y del workspace, roles de modelo, keymaps modales, comandos, tools y bloques `Custom` desde Lua, hooks `before_tool` y `after_tool`.

- [ ] Tests del parser de secuencias (`<leader>ff`, `gg`, timeout).
- [ ] La config del workspace sobreescribe a la global campo por campo.
- [ ] Un plugin con bucle infinito se corta por límite de instrucciones y la app sigue respondiendo.
- [ ] Un error de sintaxis en `init.lua` se muestra como bloque `Error` y la app arranca con defaults.

### Fase 7 — Sesiones, contexto y costo

Persistencia en SQLite, reanudar sesiones, compactación automática con el rol `compact`, conteo de costo por sesión y tope de gasto, trazas con `tracing` (Langfuse opcional).

- [ ] Cerrar y reabrir la app reanuda la última sesión del workspace con su timeline.
- [ ] Al pasar el umbral de contexto se compacta el historial y el turno continúa sin error.

### Fase 8 — MCP y subagentes

Servidores MCP declarados en Lua (stdio y HTTP) vía `rmcp`, con sus tools sujetas a las mismas aprobaciones. Tool `task` que lanza un subagente con el rol `explore` y solo tools de lectura, y devuelve un resumen.

- [ ] Un servidor MCP de ejemplo aparece en la lista de tools y se puede usar desde un turno.
- [ ] Un subagente no puede escribir ni ejecutar comandos.
- [ ] Build de release para macOS y Linux.

### Fase 9 — Flujo investigativo

Grafo de graph-flow con los 14 nodos del X-Workflow, gates como `WaitForInput` en el chat, regresos condicionales, tipos `Evidence`, `Hypothesis`, `Constraint` y `Decision`, diario de investigación, storage de sesiones sobre SQLite, bloques `Sources`, `Image` y `StageHeader`, y escritura de la carpeta del Spec.

- [ ] Cada nodo tiene un test con `LlmPort` falso que verifica su salida en el `Context`.
- [ ] Cerrar la app en el nodo 6 y reabrirla retoma en el nodo 6.
- [ ] La revisión crítica rechaza un Spec con una afirmación sin evidencia y regresa a la etapa correcta.
- [ ] `plan.json` generado valida contra su schema y su grafo de tareas no tiene ciclos.
- [ ] Demo sobre un repo real: de un bug descrito en el chat a un Spec con al menos una gráfica, un diagrama Mermaid y fuentes citadas.

### Fase 10 — Flujo de implementación

Grafo que consume `plan.json`: rama por Spec, una tarea a la vez restringida a sus archivos, pruebas con hasta 3 reintentos, commit por tarea, verificación de hipótesis y restricciones, y `report.md`.

- [ ] Tocar un archivo fuera de la tarea pide aprobación y queda en el diario.
- [ ] Una tarea que agota reintentos detiene el flujo y pregunta.
- [ ] La verificación final muestra esperado contra obtenido en una tabla.
- [ ] Demo: el Spec de la fase 9 se implementa de punta a punta con `report.md` al final.

## Reglas para el agente ejecutor, riesgos y preguntas abiertas

### Reglas

1. Antes de usar un crate, revisar su versión actual y su API en docs.rs. No inventar firmas; si un ejemplo de este plan no compila con la versión real, adaptar el código y anotar el cambio.
2. GPUI y `gpui-component` van por git: fijar el commit en `Cargo.lock` y no actualizarlos a mitad de una fase.
3. Respetar las reglas de dependencia entre crates. Si un caso de uso necesita algo de un adaptador, crear o ampliar un puerto.
4. Cada fase cierra con `cargo fmt --check`, `cargo clippy --workspace -- -D warnings` y `cargo test --workspace` en verde, y con sus criterios marcados.
5. Commits pequeños por tarea, con mensaje que diga qué y por qué.
6. Sin `unwrap()` ni `expect()` fuera de tests y del arranque del binario.
7. Nunca escribir la API key en archivos, logs ni tests; los tests usan `wiremock`.
8. Si algo del plan resulta inviable, detenerse, explicar la razón y proponer una alternativa antes de cambiar de rumbo.

### Riesgos

| Riesgo | Impacto | Mitigación |
| --- | --- | --- |
| GPUI es pre-1.0 y cambia seguido | Builds rotos al actualizar | Commit fijado; la UI aislada en su crate |
| Documentación escasa de GPUI | Avance lento en las fases 4 y 5 | Usar el código de Zed y la galería de `gpui-component` como referencia |
| Sin SDK oficial de Anthropic para Rust | Más código propio para streaming y tools | Adaptador mínimo sobre `reqwest`, cubierto con tests de `wiremock` |
| `rg` o `fzf` ausentes en la máquina del usuario | Tools de búsqueda no funcionan | Chequeo al arrancar, mensaje con instalación, fallback del crate `ignore` |
| Plugins Lua lentos o maliciosos | App congelada o acciones no deseadas | Hilo propio, límite de instrucciones, capacidades declaradas y aprobadas |
| Contexto que crece en sesiones largas | Costo alto y errores por límite | Truncado de resultados, compactación con rol `compact`, tope de gasto |

### Preguntas abiertas

- [ ] Nombre de la app (reemplaza `crest` en rutas y comandos).
- [ ] ¿Las sesiones deben sincronizarse entre máquinas o basta con SQLite local?
- [ ] ¿Debe existir un modo automático que salte los gates del flujo investigativo para cambios pequeños?

## Fuentes

- [X-Workflow, Hashlabs](https://www.hashlabs.com/guides/en/methodology/X-Workflow.html)
- [Zed: dependencias de build en Linux](https://github.com/zed-industries/zed/blob/main/docs/src/development/linux.md)

* [gpui-component](https://github.com/longbridge/gpui-component)
* [rusty-mermaid](https://docs.rs/crate/rusty-mermaid/latest)
* [rig-core: integración rmcp](https://docs.rs/rig-core/latest/rig/agent/struct.AgentBuilder.html)
* [opentelemetry-langfuse](https://docs.rs/opentelemetry-langfuse)
* [anthropic-sdk-rust (no oficial)](https://docs.rs/anthropic-sdk-rust)
