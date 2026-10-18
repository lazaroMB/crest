# Desarrollo

Esta guía es para quien cambia el código de crest. Antes de tocar nada, lee [arquitectura.md](arquitectura.md).

## Preparar el entorno

```bash
make deps-dev   # dependencias + cargo-insta y cargo-nextest
make doctor     # verifica sin instalar
make build
```

El toolchain es Rust estable (`rust-toolchain.toml`), edición 2024, con `rust-version = 1.99`. Si Rust no viene de rustup, `make doctor` avisa de que `rust-toolchain.toml` no se aplicará.

## Antes de cada commit

```bash
make check   # fmt-check + lint + test + arch + test-arch + test-deps
```

| Objetivo | Qué corre |
| --- | --- |
| `make fmt` / `make fmt-check` | `cargo fmt --all` |
| `make lint` | `cargo clippy --workspace --all-targets --all-features -- -D warnings` |
| `make test` | `cargo nextest run --workspace` más los doctests, si nextest está instalado. Si no, `cargo test --workspace`. |
| `make test-live` | Los tests `#[ignore]` cuyo nombre contiene `live`, con `ANTHROPIC_LIVE_TEST=1`. Necesita `ANTHROPIC_API_KEY` en el entorno. |
| `make arch` / `make test-arch` | `scripts/check-arch.sh` y sus fixtures. |
| `make test-deps` | Los tests de `scripts/install-deps.sh` (helpers y una simulación de macOS). |

Reglas del código, que imponen los lints del workspace:

- `unsafe` está prohibido.
- `unwrap()` y `expect()` están denegados fuera de los tests.
- Los errores se tipan con `thiserror` en las librerías; `anyhow` solo se usa en el binario.
- La API key nunca aparece en logs, tests ni archivos. `crates/adapters/anthropic/tests/no_key_in_logs.rs` lo verifica.

Los tests no usan la red, ni `rg`, ni ventana, salvo donde son tests de integración del adaptador:

- Los casos de uso se prueban con los fakes de `application::testing`.
- La API de Anthropic se prueba con `wiremock`.
- Los tests de GPUI usan el `TestAppContext` de `gpui` (feature `test-support`), que no abre ventanas.
- Los snapshots de los JSON schemas de las tools se guardan con `insta` (`crates/app/src/tools/snapshots/`). Si cambias un schema, revisa el snapshot con `cargo insta review`.

## Fakes de `application::testing`

Para usar los fakes desde otro crate, activa la feature: `application = { workspace = true, features = ["testing"] }` en `[dev-dependencies]`.

| Módulo | Fakes |
| --- | --- |
| `testing::llm` | `ScriptedLlm` (responde un guion de `ScriptedResponse`: texto, tool uses, errores), `FakeModelCatalog`, `FakeMcp`. |
| `testing::fs` | `InMemoryFs` (`FileSystemPort`, `FileSearchPort` y `ContentSearchPort` en memoria, con `with_file`), `ScriptedShell`. |
| `testing::system` | `RecordingSink` (graba los `DomainEvent`), `ScriptedApproval` (respuestas programadas), `InMemorySessionStore`, `FakePluginHost`, `RecordingTelemetry`. |

Los tests de integración de `crates/bin/tests/` levantan el composition root real con un modelo guionado mediante `root::Overrides`, que permite reemplazar el LLM, el catálogo, las rutas de config, la base de datos y el archivo de confianza MCP:

- `headless_demo.rs`
- `resume.rs`
- `mcp_servers.rs`
- `ui_session.rs`
- `research_demo.rs`
- `implement_demo.rs`

Para la UI hay ejemplos ejecutables en `crates/ui/examples/`, por ejemplo `cargo run -p ui --example chart_blocks`.

## Añadir una tool

1. Escribe un tipo que implemente `TypedTool` (`crates/app/src/tools/registry.rs`). El input es un `struct` con `Deserialize + JsonSchema`, y el schema se deriva con `schemars`. Si necesitas control total sobre el schema, implementa `Tool` directamente.
   - `name`: debe cumplir `^[a-zA-Z0-9_-]{1,128}$` y ser único en el registro.
   - `description`: está escrita para el modelo; di cuándo usarla y cuándo no.
   - `effect(&input)`: devuelve `ToolEffect::{Read, Write, Execute, Network}`. Es lo que decide la política de permisos.
   - `prepare(input, ctx)`: valida y devuelve un `PreparedCall::new(subject, future)`. `subject` es lo que se muestra y lo que cubre una regla «siempre» (una ruta, una línea de comando). Opcionalmente añade `.with_preview(ApprovalPreview::…)` y un bloque extra (como el `Diff` de `edit_file`). El future hace el trabajo y devuelve un `ToolOutput`.
2. Si la tool necesita I/O, pide un puerto y no un adaptador. Si no existe ninguno adecuado, amplía o crea uno en `crates/app/src/ports/`, con su fake en `testing`.
3. Regístrala en `crates/bin/src/root.rs::build_app` con `tools.register_typed(...)` o `tools.register(Arc::new(...))`. Las tools de archivos se registran en `register_file_tools` y las de componentes en `register_component_tools`.
4. Añade un snapshot `insta` de su definición, como en `tools/files.rs` y `tools/components.rs`, y tests con los fakes.

Si una tool debe mostrar algo rico en el timeline, devuelve `ToolOutput::text(confirmación).with_block_update(BlockKind::…)`. El modelo recibe solo la confirmación, como hacen `show_chart` y compañía.

## Añadir un tipo de bloque

1. **Dominio**: añade la variante a `BlockKind` en `crates/domain/src/block.rs`, con su payload serializable y `JsonSchema`. Si tiene invariantes, añade un `validate`. Actualiza el `match` de `Block::validate` y los snapshots.
2. **Timeline**: `crates/ui/src/timeline/`.
   - `render.rs`: `kind_name` y el texto de `copy_text` que copia `y`.
   - `state.rs`: `has_alt_view` si `t` debe alternar, y `expanded_by_default`.
3. **Render**: crea `crates/ui/src/blocks/<tipo>.rs` con `pub fn render(entry, window, cx) -> AnyElement` y regístralo en `blocks::render` (`blocks/mod.rs`). Si necesitas estado de GPUI por bloque (scroll, zoom, orden), usa `blocks::block_state` como hacen `table` y `mermaid`.
4. **Headless**: decide cómo lo imprime `crates/bin/src/headless/sink.rs`.
5. Si el modelo debe poder crearlo, añade una tool de componentes en `crates/app/src/tools/components.rs` y menciónala en `COMPONENT_TOOLS_PROMPT`.

## Añadir un adaptador

1. Crea el crate en `crates/adapters/<nombre>/`. Las dependencias internas solo pueden ser `application` y `domain` (`make arch` lo comprueba).
2. Añádelo a `members` y a `[workspace.dependencies]` en el `Cargo.toml` raíz. Fija las versiones de los crates externos en `[workspace.dependencies]`.
3. Implementa el trait del puerto con errores tipados del puerto. Los métodos async no deben bloquear el runtime: usa `spawn_blocking` para las librerías síncronas, como hacen SQLite y el llavero.
4. Constrúyelo en `crates/bin/src/root.rs`. Si el adaptador falta o falla, el composition root debe degradar a un aviso, salvo que sin él no se pueda trabajar.
5. Escribe tests de integración propios del adaptador (`tempfile`, `wiremock`, procesos reales) y, si aplica, un test `#[ignore]` con `live` en el nombre.

## Añadir o cambiar un nodo de flujo

**Flujo investigativo** (`crates/app/src/flows/research/`):

- Las etapas y sus metadatos viven en `domain::research::Stage`: orden (`Stage::ALL`), nombre (`name_es`), rol, si tiene compuerta y fase. Los nodos opcionales están en `OPTIONAL_STAGES` (`flows/research/node.rs`).
- Cada nodo implementa `ResearchNode` (`node.rs`):

  ```rust
  #[async_trait]
  pub trait ResearchNode: Send + Sync {
      fn stage(&self) -> Stage;
      fn has_gate(&self) -> bool { self.stage().has_gate() }
      async fn run(&self, ctx: &mut NodeContext) -> Result<NodeOutcome, NodeError>;
  }
  ```

  `run` devuelve `NodeOutcome::Done`, `NeedsInput { prompt }` o `Back { to, reason }` (solo hacia una etapa anterior). Usa `ctx` para el estado tipado (evidencia, hipótesis, restricciones, decisiones, diario), los datos por nodo, `user_reply` (la respuesta a la compuerta), los modelos por rol, los eventos y los agentes (`agent` para actuar, `explore_agent` para solo lectura).
- El archivo del nodo va en `flows/research/nodes/<archivo>.rs` y se registra en `nodes::node_for(stage)`. El grafo se arma en `graph.rs::research_graph`, siguiendo `Stage::ALL`.
- Cada nodo tiene tests con `ScriptedLlm` que comprueban lo que deja en el contexto. Ver `nodes/*/tests.rs` y `flows/research/tests.rs`.

**Flujo de implementación** (`crates/app/src/flows/implement/`): los pasos implementan `ImplementNode` (`node.rs`), están en `nodes/` y se ensamblan en `graph.rs`. Las compuertas se interpretan con `gate.rs::parse_choice`. Si añades un paso, actualiza también `state.rs` (pasos y estados de tarea) y `report.rs`.

## Convenciones de commits

Usamos [Conventional Commits](https://www.conventionalcommits.org/) con el crate como scope (`domain`, `application`, `search`, `ui`, `crest`, ...; `docs`, `ci` o `build` para lo que no es código). Los mensajes van en imperativo:

```text
feat(search): parse rg --json match events

Convierte cada evento "match" en un GrepHit con ruta, línea y
rango de columnas. Los eventos "context" se agregan al hit anterior.
```

- Tipos: `feat`, `fix`, `test`, `refactor`, `docs`, `build`, `ci` y `chore`.
- Un propósito por commit. Si el mensaje necesita un «y», son dos commits.
- Cada commit compila y pasa los tests del crate que toca, para que `git bisect` funcione.
- El test va en el mismo commit que el comportamiento que prueba. Los refactors, el formato y las dependencias nuevas van en commits propios.
- Conviene que cada commit quede por debajo de unas 150 líneas cambiadas, sin contar snapshots.
- Las ramas se integran con rebase, sin squash.

## CI

`.github/workflows/`:

| Workflow | Cuándo | Qué hace |
| --- | --- | --- |
| `ci.yml` | `push` y `pull_request` a `main` | `make ci` (`doctor` + `check`) en `ubuntu-24.04` y `macos-15`, más `install-deps.sh --check` en contenedores (Ubuntu 24.04 y 22.04, Fedora, Arch). |
| `live.yml` | Manual (`workflow_dispatch`) | `make test-live` con el secreto `ANTHROPIC_API_KEY`. |
| `release.yml` | Tags `v*` (o manual) | Compila y empaqueta (`scripts/package.sh`) para `x86_64-unknown-linux-gnu` (en ubuntu-22.04, por la glibc) y para `aarch64`/`x86_64-apple-darwin`, sin firmar. Con un tag, publica una GitHub Release con checksums. |

Las acciones están fijadas por SHA de commit.
