# crest

crest es un agente de código al estilo de Claude Code, escrito en Rust. Se abre en una ventana a pantalla completa que parece una terminal y trabaja sobre un directorio, el *workspace*: lee, busca, edita archivos y ejecuta comandos con tu aprobación.

Lo que lo distingue:

- **Timeline con bloques ricos.** El agente no solo responde con texto. Muestra gráficas (con su tabla, a una tecla `t`), tablas ordenables, diagramas Mermaid con pan y zoom, documentos markdown, diffs coloreados y bloques que definen tus propios plugins.
- **Modelos de Anthropic por rol.** Cada llamada pide un rol (`main`, `plan`, `explore`, `compact`, `title`, `component`, `deep`), y la configuración decide qué modelo lo atiende, con sus fallbacks.
- **Búsqueda con `rg` y `fzf`.** Las tools del agente y los selectores de la ventana usan los mismos procesos, así que tú ves lo mismo que ve el modelo.
- **Config y plugins en Lua**, con atajos modales al estilo Neovim (insert, normal y command). Desde Lua se añaden tools, hooks, comandos y bloques, todo dentro de un sandbox con límite de instrucciones y de memoria.
- **Flujos guiados.** El *investigativo* (X-Workflow, 14 nodos) va de un problema a un Spec con su plan. El *de implementación* ejecuta ese plan tarea por tarea, con un commit por tarea y un reporte final.
- **Arquitectura hexagonal.** El dominio y los casos de uso no conocen GPUI, HTTP, Lua ni SQLite, y `make arch` lo verifica.

## La pantalla

No hay capturas todavía. La ventana tiene tres zonas apiladas y un panel lateral opcional:

- **Timeline.** Ocupa casi toda la pantalla. Cada respuesta, llamada a una tool, diff o gráfica es un bloque, y se navega con `j`/`k`.
- **Input.** Es multilínea y está debajo del timeline. Muestra el modo actual (`INSERT`, `NORMAL` o `COMMAND`) y las teclas pendientes de una secuencia.
- **Barra de estado.** Está al fondo: modo, modelo del turno, modo de permisos (`ask`, `auto-edit`, `plan`), costo de la sesión y nombre del workspace. Los avisos se muestran unos segundos.
- **Árbol del workspace.** Panel opcional a la izquierda (`<Space>e`). Sobre todo lo anterior aparecen los selectores de archivo y texto, el menú de modelo y el diálogo de aprobación con el diff o el comando exacto.

El tema es oscuro y la fuente monoespaciada (JetBrains Mono si está instalada).

## Requisitos

| Componente | Versión | Notas |
| --- | --- | --- |
| Linux o macOS | — | Windows queda fuera de alcance. |
| Rust (rustup, rustfmt, clippy) | estable, ≥ 1.99 | Edición 2024. `rust-toolchain.toml` fija `stable`. |
| Compilador C, `pkg-config`, `cmake`, `clang` | — | En macOS, las Xcode Command Line Tools. |
| Librerías de GPUI en Linux | — | `libxkbcommon`, `libxkbcommon-x11`, `wayland`, `libxcb`, `libx11-xcb`, `fontconfig`, `freetype`. |
| Vulkan en Linux | — | El loader y un driver de la GPU (GPUI dibuja con Vulkan). |
| macOS | ≥ 11 | Xcode con el compilador Metal (`make doctor` lo verifica). |
| `ripgrep` (`rg`) | ≥ 13.0 | Tools `grep`, `list_files`, `find_file`, `tree`. |
| `fzf` | ≥ 0.36 | `find_file` (`--filter --scheme=path`). Sin `fzf`, cae a búsqueda por subcadena. |
| `git` | — | Datos del repo y flujos. |
| `jq` | — | Solo para `scripts/check-arch.sh`. |
| API key de Anthropic | — | `ANTHROPIC_API_KEY` o el llavero del sistema (servicio `crest`, entrada `ANTHROPIC_API_KEY`). |

SQLite se compila dentro del binario y TLS usa rustls, así que no hacen falta ni `libsqlite3` ni OpenSSL.

## Instalación

```bash
git clone <repo> crest && cd crest
make deps      # instala lo que falta (apt, dnf, yum, pacman, zypper, apk, xbps o Homebrew)
make doctor    # verifica sin instalar nada; sale con 1 si falta algo
make release   # target/release/crest (o `make build` para debug)
make install   # copia el binario a ~/.local/bin (PREFIX=/otra/ruta para cambiarlo)
```

`make deps` usa `scripts/install-deps.sh`, que acepta:

- `--check`: solo verifica;
- `--yes`: no pregunta antes de instalar;
- `--dev`: añade `cargo-insta` y `cargo-nextest`;
- `--no-rust`: no verifica ni instala Rust.

Si el `rg` o el `fzf` de tu distro son más viejos que el mínimo (el `fzf` de Ubuntu 22.04 es 0.29), el script descarga los binarios oficiales a `~/.local/bin`. `make deps-dev` es lo mismo que `make deps` con `--dev`.

Después, configura la clave:

```bash
export ANTHROPIC_API_KEY=sk-ant-...
```

Para guardarla en el llavero en vez de usar la variable, ver [docs/configuracion.md](docs/configuracion.md#api-key-de-anthropic).

## Inicio rápido

```bash
cd ~/src/mi-proyecto
crest                         # o: crest ~/src/mi-proyecto, o make run WORKSPACE=~/src/mi-proyecto
```

1. Escribe un prompt, por ejemplo «¿qué hace src/main.rs?», y pulsa `Enter`. `Shift+Enter` inserta una nueva línea.
2. Cuando el agente quiera editar un archivo o ejecutar un comando, aparece el diálogo de aprobación:
   - `y` aprueba;
   - `a` aprueba siempre (ese mismo archivo o comando);
   - `n` rechaza.
3. `Esc` pasa a modo normal:
   - `j`/`k` mueven entre bloques;
   - `Enter` expande una llamada a una tool;
   - `t` alterna una gráfica con su tabla;
   - `<Space>ff` busca archivos;
   - `<Space>p` cambia el modo de permisos;
   - `:q` sale.
4. Al volver a abrir crest en el mismo directorio, se reanuda la última sesión. `crest --new` empieza una nueva.

Sin ventana (headless), crest corre un turno y lo imprime en la terminal, con las aprobaciones por stdin:

```bash
crest --headless "encuentra dónde se parsea la config y explícalo" --workspace .
crest --headless "arregla el test que falla" --mode auto-edit --yes
make run-headless PROMPT="lista los TODO" WORKSPACE=.
```

Para investigar un problema y luego implementarlo:

```text
:research los tests de add() fallan con números negativos
…responde «sí» en cada compuerta…
:implement 0001-<slug>
```

## Documentación

| Guía | Contenido |
| --- | --- |
| [docs/uso.md](docs/uso.md) | Ventana, modos, todos los atajos y comandos `:`, aprobaciones, bloques, CLI completa, headless, sesiones y MCP. |
| [docs/configuracion.md](docs/configuracion.md) | Rutas, precedencia, API key, `app.opt`, modelos por rol, permisos, variables de entorno, Langfuse y un `init.lua` completo. |
| [docs/plugins.md](docs/plugins.md) | Cómo escribir plugins Lua: sandbox, la tabla `app`, tools, hooks, bloques `Custom`, MCP y ejemplos. |
| [docs/flujos.md](docs/flujos.md) | Flujo investigativo (14 nodos, compuertas, Specs) y flujo de implementación (pasos, compuertas, commits, reporte). |
| [docs/arquitectura.md](docs/arquitectura.md) | Capas, puertos y adaptadores, el loop de un turno, eventos, hilos y persistencia. |
| [docs/desarrollo.md](docs/desarrollo.md) | Tests, fakes y cómo añadir tools, bloques, adaptadores o nodos; convenciones de commits y CI. |

## Arquitectura en resumen

```mermaid
flowchart TB
    bin["crates/bin · crest<br/>composition root: CLI, ventana o headless"]
    ui["crates/ui<br/>GPUI: timeline, bloques, input modal"]
    subgraph adapters["crates/adapters"]
        direction LR
        anthropic["anthropic"] --- search["search<br/>rg, fzf"] --- fs["fs"] --- shell["shell"]
        store["store<br/>SQLite"] --- lua["lua<br/>mlua"] --- mcp["mcp<br/>rmcp"] --- telemetry["telemetry"]
    end
    app["crates/app · application<br/>puertos, casos de uso, tools, flujos"]
    domain["crates/domain<br/>tipos y reglas puras"]
    bin --> ui & adapters & app & domain
    ui --> app & domain
    adapters --> app & domain
    app --> domain
```

- `domain` no depende de ningún crate del proyecto, y `application` solo de `domain`.
- Cada adaptador y `ui` dependen solo de `application` y `domain`. Solo `bin` los conecta.
- Los casos de uso hablan con el mundo a través de puertos (`LlmPort`, `FileSystemPort`, `ShellPort`, `ApprovalPort`, `EventSink`...), y cada puerto tiene un fake en `application::testing`.

Los detalles están en [docs/arquitectura.md](docs/arquitectura.md).

## Estructura del repo

```text
Cargo.toml              workspace, dependencias y lints compartidos
Makefile                objetivos de build, test y verificación
rust-toolchain.toml     toolchain estable con rustfmt y clippy
crates/
  domain/               tipos y reglas puras
  app/                  `application`: puertos, casos de uso, tools, flujos, fakes
  adapters/             anthropic, search, fs, shell, store, lua, mcp, telemetry
  ui/                   vistas GPUI (más examples/ ejecutables)
  bin/                  binario `crest` y tests de integración de punta a punta
scripts/
  install-deps.sh       instala o verifica dependencias (Bash 3.2+)
  check-arch.sh         reglas de la arquitectura hexagonal
  package.sh            arma el tarball de release
tests/
  arch/                 fixtures y tests de check-arch.sh
  install-deps/         tests de install-deps.sh
docs/                   guías de usuario y de desarrollo
.github/workflows/      ci.yml, live.yml, release.yml
```

## Objetivos del Makefile

| Objetivo | Qué hace |
| --- | --- |
| `make help` | Lista los objetivos (por defecto). |
| `make deps` / `make deps-dev` | Instala las dependencias (con herramientas de desarrollo en el segundo). |
| `make doctor` | Verifica las dependencias sin instalar. |
| `make build` / `make release` | Compila el workspace (debug) / el binario optimizado `target/release/crest`. |
| `make run WORKSPACE=<dir>` | Abre la ventana sobre un directorio (por defecto `.`). |
| `make run-headless PROMPT="..." [WORKSPACE=<dir>]` | Un turno sin ventana. |
| `make fmt` / `make fmt-check` | Formatea / verifica el formato. |
| `make lint` | Clippy estricto (`-D warnings`). |
| `make test` / `make test-live` | Tests sin red / tests contra la API real (necesita `ANTHROPIC_API_KEY`). |
| `make arch` / `make test-arch` | Reglas de arquitectura / sus tests. |
| `make test-deps` | Tests de `install-deps.sh`. |
| `make check` | `fmt-check`, `lint`, `test`, `arch`, `test-arch` y `test-deps`. |
| `make ci` | `doctor` + `check`. |
| `make package` | Tarball de release en `dist/`. |
| `make install` | Copia el binario a `$(PREFIX)/bin` (`PREFIX=~/.local`). |
| `make clean` | `cargo clean`. |

## Desarrollo

```bash
make deps-dev
make check        # lo que debe pasar antes de cada commit
make test-live    # opcional: tests contra la API real
```

- Los tests no usan la red: los casos de uso se prueban con los fakes de `application::testing`, la API con `wiremock` y la UI con el contexto de tests de GPUI.
- Los tests contra la API real están marcados `#[ignore]` y necesitan `ANTHROPIC_LIVE_TEST=1` (que pone `make test-live`) y `ANTHROPIC_API_KEY`.
- La CI (`.github/workflows/ci.yml`) corre `make ci` en Ubuntu 24.04 y macOS 15. `live.yml` corre `make test-live` a mano, y `release.yml` publica tarballs para Linux x86_64 y macOS (arm64 y x86_64) con cada tag `v*`.

Las convenciones y los puntos de extensión están en [docs/desarrollo.md](docs/desarrollo.md).

## Estado del proyecto y limitaciones conocidas

Las 11 fases del plan original están implementadas y `make check` pasa en Linux. Antes de confiar en crest para trabajo real, ten en cuenta lo que no se ha probado y lo que todavía falta:

**Sin verificar**

- No se ha probado contra la API real de Anthropic en el entorno de desarrollo: todos los tests usan modelos guionados o `wiremock`, y `make test-live` no se ha ejecutado aquí. Los ids de modelo por defecto (`claude-sonnet-5-5`, `claude-opus-5-5`, `claude-haiku-5-5`, `claude-fable-5-1`) se validan contra `GET /v1/models` al arrancar.
- macOS no se ha probado en una máquina real. El código y la CI lo contemplan, pero nada se ha ejecutado ahí.
- La CI nunca se ha ejecutado en GitHub: el repositorio no tiene remoto configurado.
- No se ha probado la lectura de la API key desde el llavero del sistema.

**Comportamiento a tener en cuenta**

- `Ctrl+C` no interrumpe un flujo en curso (`:research`, `:implement`). En la ventana no llega al paso que está corriendo, y en headless mata el proceso. Para detener un flujo de forma ordenada, usa `/abort` en una compuerta. Sí interrumpe los turnos normales, en la ventana y en headless.
- En las compuertas de los flujos, una línea vacía cuenta como «sí / continuar». En el flujo de implementación eso puede hacer commit con criterios fallando o cerrar el flujo.
- En el flujo de implementación, solo `edit_file` y `write_file` están restringidos a los archivos de la tarea: `run_command` puede tocar otros (aunque el commit solo incluye los de la tarea).
- crest no trae precios de modelos: hasta que configuras `app.opt.pricing`, el costo se muestra como `$?`. El costo vive en memoria, así que una sesión reanudada vuelve a empezar en `$0.00` frente al tope.

**Aún no disponible**

- `app.opt.theme` se lee pero no tiene efecto: no hay tema claro ni fuente configurable.
- `app.opt.leader` solo afecta a los atajos definidos en Lua; los integrados siempre usan el espacio. Acciones como `session.resume` no están implementadas.
- No hay un comando para listar o elegir sesiones antiguas: se reanuda siempre la última del workspace.
- La compactación supone una ventana de contexto de 200 000 tokens, sea cual sea el modelo, y su umbral no es configurable.
- `:mcp trust` surte efecto la próxima vez que abres crest; `--trust-mcp` arranca el servidor en la misma ejecución.
- La exportación a Langfuse requiere compilar con `--features langfuse`; los binarios de `make release` no la incluyen.
- Los binarios de macOS de `release.yml` no están firmados ni notarizados.
- Algunos mensajes (errores de los comandos `/` del flujo investigativo, diagnósticos de `init.lua`) están en inglés y el resto en español.
