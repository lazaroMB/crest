# Implementation plan

Este plan convierte las 11 fases del documento principal en tareas pequeñas con dependencias explícitas, para que varios agentes trabajen al mismo tiempo y cada cambio llegue en commits mínimos y verificables.

## Modelo de ejecución

Un agente **orquestador** reparte tareas a varios agentes **worker**; cada worker trabaja en su propio `git worktree` y su propia rama, así varios pueden avanzar a la vez sin pisarse.

- **Orquestador**: lee este plan, mantiene el estado de cada tarea en `docs/tasks.md` (pendiente, en curso, en revisión, integrada), lanza workers para toda tarea cuyas dependencias ya estén integradas en `main`, revisa e integra sus ramas. Es el único que modifica los archivos compartidos (ver Integración).
- **Worker**: recibe una sola tarea (su fila de la tabla más las secciones del documento principal que cita), la implementa con commits pequeños en `t/<id>-<slug>` y avisa al terminar. No toca archivos fuera de los que su tarea declara.
- **Concurrencia**: hasta 6 workers a la vez. Si hay más tareas listas, el orquestador prioriza las de la ruta crítica (marcadas con ★ en las tablas).
- **Worktrees**: `git worktree add ../wt/<id> -b t/<id>-<slug> main`. Se borran al integrar la rama.
- **Fakes primero**: como la ola 1 entrega un adaptador falso por puerto, la UI, los adaptadores y los casos de uso se pueden construir en paralelo sin esperarse entre sí.

Cada tarea está terminada cuando:

1. Su criterio de "Listo cuando" se cumple.
2. `make check` pasa en su rama, rebasada sobre el `main` actual.
3. Cada commit de la rama compila y pasa los tests por sí solo.
4. No toca archivos fuera de su alcance declarado, salvo con aprobación del orquestador.

## Reglas de commits

Un commit es el cambio lógico más pequeño que deja el repo compilando y con los tests en verde; una tarea normalmente produce entre 3 y 8 commits.

1. **Un propósito por commit**: un tipo nuevo, una función con sus tests, un caso de error, un ajuste de config. Si el mensaje necesita "y", son dos commits.
2. **Siempre verde**: antes de cada commit corre `cargo check` y los tests del crate tocado (`cargo test -p <crate>`); `make check` completo antes de pedir integración. Así `git bisect` funciona en cualquier punto.
3. **Tamaño**: apunta a menos de 150 líneas cambiadas, sin contar snapshots ni archivos generados. Un commit de más de 300 líneas necesita una justificación en el cuerpo del mensaje.
4. **Test junto al código**: el test que prueba un comportamiento va en el mismo commit que el comportamiento. Para corregir un bug, primero un commit con el test que falla marcado `#[ignore]`, después el arreglo que lo quita.
5. **Refactor separado**: mover o renombrar código va en su propio commit, sin cambio de comportamiento.
6. **Formato separado**: nunca mezclar `cargo fmt` de archivos ajenos con cambios reales.
7. **Dependencias separadas**: agregar un crate a `[workspace.dependencies]` es su propio commit (y lo hace el orquestador; ver Integración).
8. **Mensaje**: [Conventional Commits](https://www.conventionalcommits.org/) con el crate como scope, en imperativo, y la tarea como trailer.

```text
feat(search): parse rg --json match events

Convierte cada evento "match" en un GrepHit con ruta, línea y
rango de columnas. Los eventos "context" se agregan al hit anterior.

Task: T2.C4
```

Tipos permitidos: `feat`, `fix`, `test`, `refactor`, `docs`, `build`, `ci`, `chore`. Las ramas se integran con rebase, **sin squash**, para conservar los commits pequeños en el historial de `main`.

## Mapa de paralelismo

La ruta crítica es T0.1 → T1.1 → T1.6a → T1.7a → T2.A1 → T2.A2 → T2.A3 → T3.1 → T3.3 → T5.3 → T5.8 → T5.9 → T5.10 → T6.1 → T6.2 → T6.3 → T6.7; todo lo demás corre en paralelo a ella.

_[contenido embebido omitido]_

Una ola no es una barrera: cada tarea arranca apenas sus dependencias están integradas, aunque otras tareas de la ola anterior sigan en curso.

## Ola 0 y Ola 1: esqueleto y contratos

La ola 1 define los tipos del dominio y los puertos; al terminar T1.6a, T1.6b y T1.6c los contratos quedan **congelados**, y desde ahí casi todo el resto puede ir en paralelo. ★ = ruta crítica.

| ID | Tarea | Depende de | Alcance | Listo cuando |
| --- | --- | --- | --- | --- |
| T0.1 ★ | Cargo workspace con todos los crates vacíos, `rust-toolchain.toml`, `.gitignore`, `[workspace.dependencies]` base | — | raíz, `crates/*/Cargo.toml` | `cargo build` pasa; `crates/app` se llama `application` |
| T0.2 | `Makefile` con los objetivos de la sección Build | T0.1 | `Makefile` | `make help` lista todos; `make check` corre |
| T0.3 | `scripts/install-deps.sh` según sus requisitos | — | `scripts/install-deps.sh` | `--check` correcto en Ubuntu, Fedora y Arch (contenedores) y macOS |
| T0.4 | `scripts/check-arch.sh` | T0.1 | `scripts/check-arch.sh`, `tests/arch/` | Un workspace de prueba con una dependencia prohibida hace fallar el script |
| T0.5 | CI en Linux y macOS que corre `make ci` | T0.2, T0.3, T0.4 | `.github/workflows/` | CI verde en ambos sistemas |
| T1.1 ★ | Tipos de conversación: ids, `Session`, `Turn`, `Message`, `Role`, `ContentPart` | T0.1 | `domain/src/conversation.rs` | Tests de ida y vuelta con serde |
| T1.2 | `Block` y sus payloads (`ChartSpec`, `TableSpec`, `MermaidSpec`, `DiffSpec`, `Custom`…) | T0.1 | `domain/src/block.rs` | Snapshots de los JSON schemas |
| T1.3 | `ModelSpec`, roles y `ModelPolicy::resolve` | T0.1 | `domain/src/model.rs` | Tests de los 5 niveles y de fallbacks |
| T1.4 | `WorkspacePath`: normalización léxica, rechaza `..` y rutas absolutas ajenas | T0.1 | `domain/src/workspace.rs` | Tests de casos borde |
| T1.5 | `PermissionPolicy`: modos ask, auto-edit, plan y reglas | T0.1 | `domain/src/permission.rs` | Tests en forma de tabla de decisión |
| T1.6a ★ | Puertos de modelo: `LlmPort`, `ModelCatalogPort`, `McpPort`, tipo `LlmEvent` | T1.1, T1.3 | `application/src/ports/llm.rs`, `mcp.rs` | Compila; doc comments en cada método |
| T1.6b | Puertos de archivos: `FileSearchPort`, `ContentSearchPort`, `FileSystemPort`, `ShellPort` | T1.4 | `application/src/ports/{search,fs,shell}.rs` | Compila; errores tipados |
| T1.6c | Puertos de sistema: `ApprovalPort`, `EventSink`, `SessionStorePort`, `PluginHostPort`, `TelemetryPort`, tipo `DomainEvent` | T1.1, T1.2, T1.5 | `application/src/ports/{approval,events,store,plugin,telemetry}.rs` | Compila |
| T1.7a ★ | Fakes de los puertos de modelo (LLM con guion de respuestas) | T1.6a | `application/src/testing/llm.rs` | Test que reproduce un guion de 3 respuestas |
| T1.7b | Fakes de los puertos de archivos (sistema de archivos en memoria) | T1.6b | `application/src/testing/fs.rs` | Tests propios |
| T1.7c | Fakes de los puertos de sistema (EventSink que graba, Approval programable) | T1.6c | `application/src/testing/system.rs` | Tests propios |

Paralelismo: T0.3 no depende de nada y arranca junto con T0.1. Tras T0.1 corren a la vez T0.2, T0.4 y T1.1–T1.5 (7 tareas listas; con el límite de 6 workers, la última espera el primer hueco).

## Ola 2: adaptadores, loop agente y UI base en paralelo

Nueve carriles independientes, cada uno en su propio crate; ninguno espera a otro porque todos programan contra los puertos y sus fakes.

| ID | Carril | Tarea | Depende de | Alcance | Listo cuando |
| --- | --- | --- | --- | --- | --- |
| T2.A1 ★ | A · núcleo | `ToolRegistry`: nombre, descripción, schema y handler async | T1.7a | `application/src/tools/registry.rs` | Despacha por nombre; error tipado si la tool no existe |
| T2.A2 ★ | A · núcleo | `run_turn`: modelo → tools → modelo hasta `end_turn` | T2.A1, T1.7c | `application/src/usecases/run_turn.rs` | Test con LLM falso que pide 2 tools; eventos en orden |
| T2.A3 ★ | A · núcleo | Aprobaciones dentro de `run_turn` | T2.A2, T1.5 | mismo archivo + `approval.rs` | Tests de los 3 modos |
| T2.A4 | A · núcleo | Cancelación del turno | T2.A2 | `run_turn.rs` | La tool en curso se cancela y la sesión queda consistente |
| T2.A5 | A · núcleo | Truncado de resultados por presupuesto | T2.A2 | `application/src/tools/budget.rs` | Agrega la línea "N resultados más" |
| T2.B1 ★ | B · Anthropic | Tipos de request y response de la Messages API | T1.6a | `adapters/anthropic/src/wire.rs` | Snapshots de JSON |
| T2.B2 ★ | B · Anthropic | Parser SSE → `LlmEvent` (texto, thinking, `tool_use` parcial, usage) | T2.B1 | `stream.rs` | Tests con streams grabados |
| T2.B3 | B · Anthropic | Cliente HTTP con `reqwest` + rustls; API key de env o llavero | T2.B1 | `client.rs` | 401 da un error claro; la key no aparece en logs |
| T2.B4 | B · Anthropic | Reintentos con backoff y fallback de modelo | T2.B3, T1.3 | `retry.rs` | `wiremock`: 429, 529 y 5xx |
| T2.B5 | B · Anthropic | `ModelCatalogPort` con `GET /v1/models` | T2.B3 | `catalog.rs` | Avisa de roles con modelos inexistentes |
| T2.B6 | B · Anthropic | Prompt caching y parámetros de thinking | T2.B2 | `wire.rs`, `request.rs` | Snapshots de requests |
| T2.B7 | B · Anthropic | Test real ignorado por defecto | T2.B4 | `tests/live.rs` | Pasa con `ANTHROPIC_LIVE_TEST=1` |
| T2.C1 | C · búsqueda | Procesos sin shell: pipe entre dos, timeout, cancelación | T1.6b | `adapters/search/src/process.rs` | Tests de timeout y de pipe |
| T2.C2 | C · búsqueda | Detección de `rg` y `fzf` con versión mínima | T2.C1 | `detect.rs` | Mensaje claro si faltan |
| T2.C3 | C · búsqueda | `list_files` y `tree` con `rg --files` | T2.C1 | `files.rs` | Respeta `.gitignore` en un workspace temporal |
| T2.C4 | C · búsqueda | `grep` con `rg --json` | T2.C1 | `grep.rs` | Patrón que empieza con `-` funciona |
| T2.C5 | C · búsqueda | `find_file`: `rg --files` → `fzf --filter`, fallback por subcadena | T2.C2, T2.C3 | `find.rs` | Orden por puntuación; fallback sin fzf |
| T2.D1 | D · archivos | `FileSystemPort` con canonicalización y `PathOutsideWorkspace` | T1.6b, T1.4 | `adapters/fs/src/guard.rs` | Bloquea symlinks que escapan |
| T2.D2 | D · archivos | `read_file` con rango, números de línea, binarios y tope de tamaño | T2.D1 | `read.rs` | Tests por cada caso |
| T2.D3 | D · archivos | `edit_file` y `write_file` con diff de `similar` | T2.D1 | `write.rs` | `old` repetido falla; diff correcto |
| T2.D4 | D · archivos | `ShellPort` con timeout y `cwd` del workspace | T1.6b | `adapters/shell/` | Mata el proceso al vencer el timeout |
| T2.E1 | E · storage | Esquema SQLite y migraciones | T1.6c | `adapters/store/migrations/` | Migra una base vacía y una de la versión anterior |
| T2.E2 | E · storage | `SessionStorePort`: guardar y cargar sesión con timeline | T2.E1 | `adapters/store/src/session.rs` | Ida y vuelta de una sesión con 100 bloques |
| T2.F1 | F · Lua | Host `mlua` en hilo propio con canal request/response | T1.6c | `adapters/lua/src/host.rs` | Llamadas concurrentes no bloquean al llamador |
| T2.F2 | F · Lua | Sandbox y límite de instrucciones | T2.F1 | `sandbox.rs` | Bucle infinito cortado; sin `io` ni `os.execute` |
| T2.F3 | F · Lua | Carga y merge de config global y del workspace | T2.F1, T1.3 | `config.rs` | El workspace sobreescribe campo por campo |
| T2.F4 | F · Lua | Parser de secuencias de teclas: modos, leader, `timeoutlen` | T0.1 | `application/src/keymap.rs` | Tests de `<leader>ff`, `gg` y timeout |
| T2.G1 | G · trazas | Spans por turno y por llamada al modelo con `tracing` | T1.6c | `adapters/telemetry/` | Spans anidados en un test |
| T2.G2 | G · trazas | Exportador a Langfuse detrás de una feature | T2.G1 | mismo crate | Compila con y sin la feature |
| T2.H1 ★ | H · UI | Ventana GPUI a pantalla completa, tema y fuente mono | T0.1 | `ui/src/app.rs` | Abre en Linux y macOS |
| T2.H2 ★ | H · UI | Timeline virtualizado con `Text`, `ToolCall` y `Error` | T2.H1, T1.2, T1.7c | `ui/src/timeline/` | Fluido con 1000 bloques de un EventSink falso |
| T2.H3 | H · UI | Input multilínea con modos insert, normal y command | T2.H1, T2.F4 | `ui/src/input.rs` | Navegación modal funciona |
| T2.H4 | H · UI | Barra de estado | T2.H1 | `ui/src/status.rs` | Muestra modelo, modo, costo y workspace |
| T2.I1 | I · investigación | Tipos `Evidence`, `Hypothesis`, `Constraint`, `Decision`, `JournalEntry` | T1.1 | `domain/src/research.rs` | Serde y schemas en snapshot |
| T2.I2 | I · investigación | Schema de `plan.json`, validación y detección de ciclos | T2.I1 | `domain/src/plan.rs` | Rechaza un plan con ciclo |

Pico de paralelismo: tras los contratos hay unas 15 tareas listas a la vez (la primera de cada carril más varias independientes), así que los 6 workers siempre tienen trabajo.

## Ola 3: integración headless, UI real y bloques ricos

T3.1 junta los carriles A–D en el binario y cierra la fase 3 del documento principal (modo headless); los bloques ricos (T3.7–T3.12) solo necesitan el timeline y van en paralelo desde antes.

| ID | Tarea | Depende de | Alcance | Listo cuando |
| --- | --- | --- | --- | --- |
| T3.1 ★ | Composition root y modo `--headless` | T2.A3, T2.B4, T2.C5, T2.D3, T2.D4 | `crates/bin/` | Demo headless: encuentra una función, la edita con aprobación y explica |
| T3.2 | Definiciones de las tools de archivos para el modelo (descripciones y schemas) | T2.A1, T2.C5, T2.C4, T2.D3 | `application/src/tools/files.rs` | Snapshots de los schemas |
| T3.3 ★ | UI conectada al `AgentService` real: streaming e interrupción con `Ctrl+C` | T3.1, T2.H2, T2.H3 | `ui/src/session.rs` | Turno completo desde la UI |
| T3.4 | Bloque `Diff` y diálogo de aprobación | T2.H2, T2.A3 | `ui/src/blocks/diff.rs`, `ui/src/approval.rs` | Aprobar y rechazar funcionan |
| T3.5 | Panel lateral con el árbol del workspace | T2.H1, T2.C3 | `ui/src/tree.rs` | Se refresca con `notify` |
| T3.6 | Selectores de archivo y texto (`<leader>ff`, `<leader>fg`) | T2.H1, T2.C5, T2.C4 | `ui/src/picker.rs` | Mismos resultados que las tools del agente |
| T3.7 | Tools de componentes: `show_chart`, `show_table`, `show_mermaid`, `show_markdown` | T2.A1, T1.2 | `application/src/tools/components.rs` | Snapshots de schemas; devuelven confirmación corta |
| T3.8 | Bloque `Chart` que alterna con su tabla (`t`) | T2.H2 | `ui/src/blocks/chart.rs` | Barras, líneas, área y pie |
| T3.9 | Bloque `Table` con `DataTable` | T2.H2 | `ui/src/blocks/table.rs` | Ordenable; 10 000 filas fluidas |
| T3.10 | Bloque `Mermaid` con `rusty-mermaid`, pan y zoom | T2.H2 | `ui/src/blocks/mermaid.rs` | Un diagrama inválido muestra código y error, sin panic |
| T3.11 | Bloque `Markdown` de documento, alternable a fuente | T2.H2 | `ui/src/blocks/markdown.rs` | Encabezados, tablas y código con resaltado |
| T3.12 | Bloque `Thinking` colapsable | T2.H2 | `ui/src/blocks/thinking.rs` | Colapsado por defecto |

Con T3.3, T3.7 y T3.8 integrados se corre la demo de la fase 5: "analiza el tamaño de los archivos por extensión y muéstralo en una gráfica".

## Ola 4 y Ola 5: Lua, sesiones y flujo investigativo

La ola 4 completa las fases 6 y 7; la ola 5 construye el flujo X-Workflow. Los 14 nodos de investigación son tareas separadas: una vez que existe el marco de nodos (T5.3), se escriben en paralelo.

### Ola 4

| ID | Tarea | Depende de | Alcance | Listo cuando |
| --- | --- | --- | --- | --- |
| T4.1 | API Lua: `keymap.set` y `commands.register` | T2.F3, T3.3 | `adapters/lua/src/api/keymap.rs` | Atajo definido en Lua ejecuta una acción |
| T4.2 | API Lua: `tools.register` | T2.F3, T2.A1 | `api/tools.rs` | Tool de Lua usable por el modelo |
| T4.3 | Hooks `before_tool` y `after_tool` | T2.F3, T2.A3 | `api/hooks.rs` | Un hook puede negar una tool |
| T4.4 | Bloques `Custom`: árbol declarativo y su render | T2.F1, T2.H2 | `api/ui.rs`, `ui/src/blocks/custom.rs` | Lista y tabla desde Lua |
| T4.5 | Errores de `init.lua` como bloque `Error`, arranque con defaults | T2.F3, T3.3 | `crates/bin/` | Error de sintaxis no impide abrir |
| T4.6 | Reanudar la última sesión del workspace | T2.E2, T3.3 | `application/src/usecases/resume.rs` | Cerrar y abrir conserva el timeline |
| T4.7 | Compactación con el rol `compact` | T2.A2, T1.3 | `application/src/usecases/compact.rs` | El turno continúa tras compactar |
| T4.8 | Costo por sesión y tope de gasto | T2.B2, T2.H4 | `application/src/cost.rs`, `ui/src/status.rs` | Al tope, pausa y pide confirmación |

### Ola 5: flujo investigativo

| ID | Tarea | Depende de | Alcance | Listo cuando |
| --- | --- | --- | --- | --- |
| T5.1 ★ | `SessionStorage` de graph-flow sobre SQLite | T2.E1 | `adapters/store/src/graphflow.rs` | Retoma un grafo tras reiniciar |
| T5.2 | Tools de búsqueda y lectura web del servidor de Anthropic | T2.B2 | `adapters/anthropic/src/server_tools.rs` | Citas llegan como `Evidence` |
| T5.3 ★ | Marco de nodos: wrapper de `Task`, gates en el chat, regresos condicionales, `/stage`, `/back`, `/skip`, `/abort` | T2.I1, T3.3, T5.1 | `application/src/flows/research/mod.rs` | Grafo de 3 nodos falsos con un gate y un regreso |
| T5.4 | Bloques `Sources`, `Image` y `StageHeader` | T2.H2, T1.2 | `ui/src/blocks/` | Cita archivo:línea abre el archivo |
| T5.5a–f | Nodos de descubrimiento: Intake, Observación, Datos, Análisis, Hipótesis, Experimentos (una tarea por nodo) | T5.3 (Datos también T5.2) | `flows/research/nodes/<nodo>.rs` | Test por nodo con LLM falso |
| T5.6 | Nodo Definición y restricciones | T5.3 | `nodes/define.rs` | Rechaza definiciones no medibles |
| T5.7a–d | Nodos de soluciones: Investigación, Lluvia de ideas, Modelado, Prototipo en worktree (una tarea por nodo) | T5.3 (Investigación también T5.2) | `nodes/<nodo>.rs` | Test por nodo |
| T5.8 ★ | Nodos Spec y Plan: escriben la carpeta del Spec | T5.3, T2.I2 | `nodes/spec.rs`, `nodes/plan.rs` | `plan.json` válido y `plan.md` generado desde él |
| T5.9 ★ | Revisión crítica con subagente y regreso a la etapa que falta | T5.8 | `nodes/review.rs` | Rechaza una afirmación sin evidencia |
| T5.10 ★ | Ensamblar el grafo completo y demo | T5.4–T5.9 | `flows/research/graph.rs` | De un bug en el chat a un Spec con gráfica, Mermaid y fuentes |

Los 11 nodos de T5.5–T5.7 son el segundo pico de paralelismo: todos dependen solo de T5.3.

## Ola 6 y Ola 7: flujo de implementación, MCP y release

La ola 6 cierra la ruta crítica; la ola 7 no depende de ella y cada tarea arranca en cuanto sus dependencias están integradas.

| ID | Tarea | Depende de | Alcance | Listo cuando |
| --- | --- | --- | --- | --- |
| T6.1 ★ | Cargar `plan.json`, crear la rama del Spec y ordenar tareas | T2.I2, T5.3 | `application/src/flows/implement/load.rs` | Orden topológico correcto |
| T6.2 ★ | Nodo Implementar tarea, restringido a sus archivos | T6.1 | `nodes/implement.rs` | Archivo ajeno pide aprobación y queda en el diario |
| T6.3 ★ | Nodo Pruebas con hasta 3 reintentos y gate | T6.2 | `nodes/test.rs` | Al agotar reintentos se detiene y pregunta |
| T6.4 | Commit por tarea con el id en el mensaje | T6.2 | `nodes/commit.rs` | Un commit por tarea del plan |
| T6.5 | Verificar hipótesis y restricciones: tabla y gráfica | T6.1, T3.8 | `nodes/verify.rs` | Esperado contra obtenido |
| T6.6 | `report.md` en la carpeta del Spec | T6.5 | `nodes/report.rs` | Lista logros, fallos, desvíos e ideas |
| T6.7 ★ | Ensamblar el grafo y demo de punta a punta | T6.3–T6.6, T5.10 | `flows/implement/graph.rs` | El Spec de T5.10 implementado con su reporte |
| T7.1 | Adaptador `McpPort` con `rmcp` (stdio y HTTP) | T1.6a, T2.A1 | `adapters/mcp/` | Tools de un servidor de ejemplo en el registro |
| T7.2 | Servidores MCP declarados en Lua | T7.1, T2.F3 | `adapters/lua/src/api/mcp.rs` | Configurar uno en `init.lua` lo conecta |
| T7.3 | Tool `task`: subagente con rol `explore` y solo lectura | T2.A2 | `application/src/tools/task.rs` | No puede escribir ni ejecutar |
| T7.4 | Builds de release para Linux y macOS en CI | T0.5, T3.3 | `.github/workflows/release.yml` | Artefactos de ambos sistemas |

## Integración

El orquestador integra una rama a la vez en `main`, en orden de dependencias; `main` siempre compila y pasa `make check`.

### Flujo por rama

1. El worker avisa que terminó y deja su rama rebasada sobre `main`.
2. El orquestador revisa: alcance de archivos, tamaño de commits, mensajes, y que cada commit pase por sí solo (`git rebase -x 'cargo check && cargo test -p <crate>' main`).
3. Integra con `git rebase` + fast-forward, sin merge commits ni squash.
4. Corre `make check` sobre `main`; si falla, revierte la integración (`git reset` al commit anterior, solo local antes de publicar) y devuelve la tarea al worker.
5. Marca la tarea como integrada en `docs/tasks.md` y lanza las tareas que acaban de quedar desbloqueadas.

### Archivos compartidos

Solo el orquestador los modifica, cada cambio en su propio commit directo a `main`, y los workers rebasan después:

- `Cargo.toml` raíz (miembros y `[workspace.dependencies]`).
- Listas de módulos que varias tareas tocarían (`mod` en `lib.rs` de `application`, registro de bloques en `ui`).
- `docs/tasks.md`.

Un worker que necesita una dependencia nueva o registrar un módulo lo pide al orquestador en su reporte, en lugar de editarlo.

### Contratos congelados

Desde que se integran T1.6a–c, cambiar un trait de puerto o un tipo público del dominio requiere:

1. Que el worker se detenga y describa el cambio y por qué.
2. Que el orquestador lo aplique en un commit propio en `main`, actualizando a la vez fakes y adaptadores afectados.
3. Que los workers en curso rebasen.

Agregar un método con implementación por defecto o un tipo nuevo no rompe a nadie y no necesita este proceso.

### Conflictos

- Cada tarea declara su alcance de archivos; el orquestador no lanza en paralelo dos tareas cuyos alcances se crucen (por ejemplo, T2.A3, T2.A4 y T2.A5 tocan `run_turn.rs` y van en serie).
- Si aun así hay conflicto al rebasar, lo resuelve el worker dueño de la rama, nunca el orquestador sobre código ajeno.

## Prompts para los agentes

Exporta el documento completo (las dos pestañas) a Markdown y colócalo en el repo como `docs/PLAN.md` y `docs/IMPLEMENTATION_PLAN.md`; los prompts los citan por esas rutas.

### Orquestador

```text
Eres el orquestador de la implementación de este proyecto.

Lee docs/PLAN.md y docs/IMPLEMENTATION_PLAN.md completos antes de actuar.

1. Crea docs/tasks.md con todas las tareas de las tablas (id, estado,
   dependencias, alcance) y haz commit.
2. Repite hasta terminar:
   a. Calcula las tareas listas: todas sus dependencias integradas en main
      y sin alcance de archivos cruzado con una tarea en curso.
   b. Prioriza las marcadas con ★ (ruta crítica).
   c. Mantén hasta 6 workers en paralelo. Para cada tarea lista:
      git worktree add ../wt/<id> -b t/<id>-<slug> main
      y lanza un worker con el prompt de worker, su fila de la tabla y
      las secciones de PLAN.md que necesita.
   d. Cuando un worker termine, integra su rama siguiendo la sección
      Integración. Si make check falla en main, revierte y devuélvela.
   e. Aplica tú mismo, en commits propios, los cambios a archivos
      compartidos y a contratos que pidan los workers.
   f. Actualiza docs/tasks.md en cada cambio de estado.
3. Al cerrar cada ola, verifica los criterios de la fase correspondiente
   en PLAN.md y reporta: tareas integradas, desvíos y riesgos nuevos.

Nunca escribas código de una tarea asignada a un worker. Si el plan
resulta inviable en algún punto, detente y explica antes de cambiarlo.
```

### Worker

```text
Eres un worker. Implementa solo la tarea <ID>: <título>.

Contexto: lee las secciones citadas de docs/PLAN.md y la sección
"Reglas de commits" de docs/IMPLEMENTATION_PLAN.md.

Tu worktree es ../wt/<ID> en la rama t/<ID>-<slug>.
Alcance: solo puedes modificar <archivos del alcance>.
Listo cuando: <criterio de la tabla>.

Cómo trabajar:
- Commits lo más pequeños posible: un propósito por commit, con su test.
  Antes de cada commit: cargo check y cargo test -p <crate>.
- Mensajes en Conventional Commits con scope del crate y trailer
  "Task: <ID>".
- Antes de declarar terminado: git rebase main, make check en verde y
  git rebase -x 'cargo check' main sin fallos.
- Revisa la API real de cada crate en docs.rs; no inventes firmas.
- No edites Cargo.toml raíz, listas de módulos compartidas, traits de
  puertos ni tipos públicos del dominio. Si los necesitas, detente y
  pídelo en tu reporte.

Al terminar, reporta: commits hechos (hash y mensaje), cómo verificaste
el criterio, y cualquier pedido al orquestador.
```

