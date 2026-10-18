# Uso

Esta guía cubre la ventana, la línea de comandos, el modo headless, las sesiones y los servidores MCP. Los flujos investigativo y de implementación tienen su propia guía: [flujos.md](flujos.md).

## La ventana

```bash
crest                 # workspace = directorio actual
crest ~/src/proyecto  # o --workspace ~/src/proyecto
```

La ventana abre a pantalla completa. En X11 abre en ventana normal y pide pantalla completa después del primer cuadro, y si el compositor se niega queda centrada a 1280×800. El título es `crest — <directorio>`. Usa tema oscuro y una sola fuente monoespaciada: JetBrains Mono si está instalada; si no, la monoespaciada del sistema.

```text
┌──────────────┬───────────────────────────────────────────────┐
│ árbol        │ timeline: bloques (texto, tools, diffs,       │
│ (<Space>e,   │ gráficas, tablas, Mermaid, markdown...)       │
│  oculto por  │                                               │
│  defecto)    │                                               │
│              ├───────────────────────────────────────────────┤
│              │ input multilínea                              │
│              │ INSERT  <teclas pendientes>                   │
│              ├───────────────────────────────────────────────┤
│              │ INSERT │ modelo │ ask │ $0.00 │ proyecto  ⟨aviso⟩│
└──────────────┴───────────────────────────────────────────────┘
```

La barra de estado muestra, de izquierda a derecha:

- el modo del input;
- el modelo del turno actual o del último;
- el modo de permisos;
- el costo de la sesión;
- el workspace.

Los avisos (por ejemplo «retrying in 2.0s», «x failed, using y» o un fallback de modelo) se quedan 6 s.

Cada prompt se ejecuta como un **turno**, y solo corre uno a la vez. Si envías un prompt mientras corre un turno, queda **en cola**: la barra muestra cuántos esperan, y se envía cuando el turno termina. `Ctrl+C` interrumpe el turno y vacía la cola. `Ctrl+Q` (en macOS `Cmd+Q`) o `:q` cierran crest.

### Modos

| Modo | Para qué | Cómo se entra |
| --- | --- | --- |
| **insert** | Escribir el prompt (editor de varias líneas, hasta 8 filas antes de hacer scroll). | Modo inicial. `i` desde normal, o un clic en el editor. |
| **normal** | Navegar el timeline y usar los atajos `<leader>`. | `Esc` desde insert o command. |
| **command** | Escribir un comando `:` en una línea. | `:` desde normal. `Esc`, o `Backspace` con la línea vacía, vuelve a normal. |

En insert, las teclas que no tienen atajo van al editor. Mientras esperas la segunda tecla de una secuencia (`g` de `gg`, `<Space>` de `<Space>ff`), la línea bajo el editor muestra lo que llevas escrito. Pasado `timeoutlen` (1 s por defecto), se ejecuta el atajo exacto si existe; si no, las teclas se descartan (en insert se escriben como texto).

### Atajos por defecto

`<leader>` es la barra espaciadora en todos los atajos integrados. Si cambias `app.opt.leader`, el cambio solo afecta a los atajos definidos en Lua (ver [plugins.md](plugins.md#4-keymaps)).

| Modo | Tecla | Acción | Qué hace |
| --- | --- | --- | --- |
| insert | `Esc` | `mode.normal` | Pasa a normal. |
| insert | `Enter` | `submit` | Envía el prompt (si no está vacío). |
| insert | `Shift+Enter` | `newline` | Inserta una nueva línea. |
| normal | `i` | `mode.insert` | Vuelve a insert. |
| normal | `j` / `k` | `block.next` / `block.prev` | Bloque siguiente / anterior. |
| normal | `gg` / `G` | `block.first` / `block.last` | Primer / último bloque. |
| normal | `Enter` | `block.toggle` | Expande o colapsa el bloque (ToolCall, Thinking). |
| normal | `t` | `block.toggle_view` | Vista alternativa del bloque (ver [Bloques](#bloques)). |
| normal | `y` | `block.yank` | Copia el contenido del bloque al portapapeles. |
| normal | `<Space>ff` | `picker.files` | Selector de archivos (fzf). |
| normal | `<Space>fg` | `picker.grep` | Búsqueda de texto (rg). |
| normal | `<Space>e` | `panel.tree` | Muestra u oculta el árbol del workspace. |
| normal | `<Space>m` | `model.pick` | Elige el modelo del **siguiente** turno. |
| normal | `<Space>p` | `permission.cycle` | Cicla `ask` → `auto-edit` → `plan`. |
| normal | `:` | `mode.command` | Abre la línea de comandos. |
| command | `Esc` | `mode.normal` | Cancela el comando. |
| todos | `Ctrl+C` | `turn.interrupt` | Interrumpe el turno en curso. También funciona con un selector o un diálogo abiertos. |
| todos | `Ctrl+Q` / `Cmd+Q` | — | Cierra crest. |

Puedes redefinir o añadir atajos con `app.keymap.set` ([plugins.md](plugins.md#appkeymapsetmodo-teclas-acción)).

### Comandos `:`

| Comando | Qué hace |
| --- | --- |
| `:q`, `:quit`, `:qa` | Cierra crest. |
| `:model <alias\|rol\|id>` | Modelo del **siguiente** turno. Acepta un alias (`opus`), un nombre de rol (`plan`, que usa la spec entera de ese rol) o un id completo. Sin argumento vuelve al modelo por defecto. |
| `:mode <ask\|auto-edit\|plan>` | Cambia el modo de permisos. |
| `:clear` | Empieza una sesión nueva con el timeline vacío. La anterior queda guardada. No funciona mientras corre un turno. |
| `:mcp trust <nombre>` | Aprueba un servidor MCP del workspace. Arranca la próxima vez que abras crest. |
| `:implement <spec>` | Inicia o reanuda el flujo de implementación de un Spec ([flujos.md](flujos.md#flujo-de-implementación)). |
| `:research <problema>`, `:research resume`, `:research` | Inicia, reanuda o consulta el flujo investigativo ([flujos.md](flujos.md#flujo-investigativo)). |
| `:<nombre> <args>` | Un comando registrado con `app.commands.register`. |
| `:<acción.con.punto>` | Ejecuta una acción integrada, por ejemplo `:picker.files` o `:panel.tree`. |

Un comando desconocido muestra «comando desconocido :x (usa :model, :mode, :clear, :mcp, :implement, :research o :q)».

### Referencias a archivos con `@`

Cuando eliges un archivo en el árbol, en un selector o en un bloque `Sources`, se inserta `@ruta ` (o `@ruta:línea ` si es un resultado de grep) en el cursor del prompt, y el input pasa a insert. Es texto normal del prompt: el modelo lo lee como referencia y abre el archivo con sus tools.

### Selectores (`<Space>ff`, `<Space>fg`)

Usan los mismos puertos y los mismos valores por defecto que las tools `find_file` y `grep` del agente, así que ves lo mismo que vería el modelo:

- respetan `.gitignore`;
- excluyen `.git/`;
- aplican las rutas sensibles.

Los resultados se actualizan mientras escribes, con una espera de 120 ms. En `<Space>fg` el texto es una expresión regular de ripgrep.

| Tecla | Acción |
| --- | --- |
| `↑` / `↓`, `Ctrl+P` / `Ctrl+N` | Mover la selección. |
| `Enter` | Insertar la referencia `@...` en el prompt. |
| `Esc` | Cerrar. |

### Árbol del workspace (`<Space>e`)

Es un panel lateral de 280 px, oculto al abrir. Viene del mismo `tree` que usa el agente: muestra hasta 16 niveles y omite los archivos ignorados y `.git/`.

- Un clic en un directorio lo abre o lo cierra.
- Un clic en un archivo inserta `@ruta`.
- El árbol se refresca solo cuando cambian los archivos (vigila el directorio, con 200 ms de espera). Si la vigilancia no puede arrancar, aparece el aviso «el árbol no se refrescará solo».

### Menú de modelo (`<Space>m`)

Lista «default (rol main)» y todos los alias, los incorporados (`opus`, `sonnet`, `haiku`, `fable`) más los de tu config, con el modelo al que apunta cada uno.

| Tecla | Acción |
| --- | --- |
| `j` / `k`, `↓` / `↑`, `Tab` | Mover la selección. |
| `Enter` | Elegir. |
| `Esc` / `q` | Cerrar. |

La elección vale solo para el siguiente turno. Para todos los turnos usa `--model` al arrancar.

### Aprobaciones y modos de permiso

Cada tool declara su efecto (`read`, `write`, `execute` o `network`), y el modo de permisos decide qué pasa:

| Modo | Leer | Editar archivos | Ejecutar comandos / red |
| --- | --- | --- | --- |
| `ask` (defecto) | sí | pregunta | pregunta |
| `auto-edit` | sí | sí | pregunta |
| `plan` | sí | no (se niega) | no (se niega) |

Para cambiar de modo: `<Space>p`, `:mode <modo>` o `--mode` al arrancar.

`run_command` cuenta como lectura solo para `git log`, `git blame`, `git diff`, `git status` y `git show` sin opciones que escriban (`--output`, `--no-index`, `--contents`, `--ext-diff`). Cualquier otro comando cuenta como ejecución.

Cuando hace falta aprobar, se abre un diálogo con el diff coloreado (en ediciones) o con el comando exacto y su directorio (en `run_command`):

| Tecla | Respuesta |
| --- | --- |
| `y` | Aprobar esta vez. |
| `a` | Aprobar siempre: se crea una regla para **esa misma tool y ese mismo sujeto exacto** (la misma ruta o la misma línea de comando, nada más amplio). |
| `n` | Rechazar. Antes pide un motivo opcional, que llega al modelo: escríbelo y pulsa `Enter`, o `Esc` para volver. |
| `Esc` | Rechazar sin motivo. |

También hay botones «Approve (y)», «Always (a)» y «Reject (n)». Si llegan varias solicitudes, esperan en cola. Una tool negada (por ti, por el modo o por un hook) no se reintenta sola: el modelo recibe el motivo y decide.

Las reglas «siempre» se guardan con la sesión y vuelven al reanudarla. `--new` y headless sin `--continue` empiezan sin reglas, mientras que `:clear` conserva las de la ejecución en curso.

El mismo diálogo aparece cuando la sesión llega al tope de gasto (`app.opt.max_session_cost`). Ver [configuracion.md](configuracion.md#costo-y-tope-de-gasto).

### Bloques

| Bloque | Origen | Interacción |
| --- | --- | --- |
| Texto | Respuesta del modelo (markdown en streaming); tu prompt aparece citado con `>`. | — |
| Thinking | Razonamiento extendido. | Colapsado en una línea; `Enter` lo expande. |
| ToolCall | Cualquier tool: una línea con nombre, sujeto y estado (pendiente, esperando aprobación, corriendo, ok, falló, denegada, cancelada). | `Enter` muestra el input y la salida. |
| Diff | `edit_file`, `write_file` | Diff coloreado con su estado de aprobación. |
| Chart | Tool `show_chart` (barras, líneas, área, torta). | **`t` alterna con una tabla** de los mismos datos. |
| Table | Tool `show_table` | Virtualizada. Un clic en el icono de orden de una columna cicla descendente → ascendente → original. |
| Mermaid | Tool `show_mermaid` | Arrastrar mueve el diagrama; `Ctrl`/`Cmd` + rueda hace zoom; con el diagrama enfocado (clic), `+`/`-` hacen zoom y `0` lo reinicia. `t` muestra el código. Si no parsea, se ven el código y el error. |
| Markdown | Tool `show_markdown` | Tipografía de documento; `t` muestra la fuente. |
| Custom | Tools de plugins Lua | Ver [plugins.md](plugins.md#appui-bloques-custom). |
| Sources, StageHeader, Image | Flujo investigativo | En Sources, un clic (o `Enter`) en un archivo inserta `@ruta:línea`, y en una URL abre el navegador. |
| Error | Fallos de API, tools o config | Mensaje y acción sugerida. |

`t` solo tiene efecto en Chart, Markdown y Mermaid. `y` copia el contenido del bloque seleccionado.

### Costo

La barra muestra el costo acumulado de la sesión:

- `$0.00` antes de la primera respuesta;
- `$?` si el modelo usado no tiene precio configurado; crest no trae precios, así que este es el valor habitual hasta que configuras `app.opt.pricing`;
- si no, el importe, con 4 decimales por debajo de 1 USD y 2 a partir de ahí.

Ver [configuracion.md](configuracion.md#costo-y-tope-de-gasto).

## Línea de comandos

```text
crest [OPTIONS] [DIR]
```

| Flag | Valor | Qué hace |
| --- | --- | --- |
| `[DIR]` | directorio | Workspace a abrir. Por defecto, el directorio actual. |
| `--workspace <DIR>` | directorio | Igual que `[DIR]`. No se pueden usar los dos. |
| `--headless [<PROMPT>]` | texto | Corre un turno con ese prompt en la terminal, sin ventana. Sin prompt, va con `--implement` o `--research`. |
| `--implement <RUTA>` | Spec o `plan.json` | En headless, ejecuta el flujo de implementación. Requiere `--headless` y no se combina con `--research`. |
| `--research <PROBLEMA>` | texto o `resume` | En headless, ejecuta el flujo investigativo. Requiere `--headless`. |
| `--model <ALIAS\|ID>` | alias, rol o id | Override del modelo para **todos** los turnos de esta ejecución (como `:model`, pero permanente). |
| `--mode <MODE>` | `ask`, `auto-edit`, `plan` | Modo de permisos inicial. Por defecto `ask`. |
| `--yes` | — | En headless, aprueba todas las acciones sin preguntar. No responde las compuertas de los flujos. Requiere `--headless`. |
| `--new` | — | La ventana abre con una sesión nueva en vez de reanudar la última. No se combina con `--headless`. |
| `--continue` | — | En headless, continúa la última sesión del workspace, con su historial. Requiere `--headless`. |
| `--trust-mcp <NAME>` | nombre | Aprueba el servidor MCP `NAME` del `.crest/init.lua` del workspace, lo arranca y lo recuerda. Se puede repetir. |
| `-h`, `--help` / `-V`, `--version` | — | Ayuda / versión. |

Ejemplos:

```bash
crest                                        # ventana sobre el directorio actual, reanuda la última sesión
crest --new ~/src/api                        # ventana, sesión nueva
crest --mode plan --model opus .             # solo lectura, con Opus en cada turno
crest --headless "explica qué hace src/main.rs" --workspace ~/src/api
crest --headless "corre los tests y arregla el que falle" --mode auto-edit --yes
crest --headless "¿y el segundo fallo?" --continue
crest --headless --research "add() suma mal con negativos"
crest --headless --research resume
crest --headless --implement 0001-add-suma-negativos
crest --trust-mcp tickets --trust-mcp docs
```

Con `make` (`WORKSPACE` es por defecto `.`):

```bash
make run WORKSPACE=~/src/api
make run-headless PROMPT="lista los TODO del proyecto" WORKSPACE=~/src/api
```

### Códigos de salida

| Código | Ventana | Headless (turno) | Headless (flujos) |
| --- | --- | --- | --- |
| 0 | Se cerró bien. | El turno terminó. | El flujo se completó. |
| 1 | La ventana no pudo abrir. | El turno falló (error del modelo, más de 50 llamadas al modelo en un turno). | Abortado, vuelta al Spec, error de un paso o stdin cerrado en una compuerta. |
| 2 | El workspace no se puede abrir. | Config imposible (workspace, falta de API key) o argumentos inválidos. | No se pudo abrir el flujo (destino, base de datos) o argumentos inválidos. |
| 130 | — | Interrumpido con `Ctrl+C`. | — (ver abajo) |

## Modo headless

`crest --headless "<prompt>"` ejecuta **un** turno e imprime los eventos en la terminal:

- el texto, en streaming;
- el razonamiento, colapsado en una línea;
- una línea por tool y estado;
- los diffs coloreados;
- los avisos y errores;
- al final, el uso de tokens.

Cada aprobación muestra el diff o el comando y pregunta:

```text
¿Permitir edit_file src/lib.rs? [y]es / [a]lways / [n]o:
```

| Respuesta | Efecto |
| --- | --- |
| `y`, `yes`, `s`, `si`, `sí` | Aprobar. |
| `a`, `always`, `siempre` | Aprobar siempre (para este sujeto exacto). |
| `n`, `no`, o una línea vacía | Rechazar. |

Otra respuesta vuelve a preguntar. Si stdin se cierra, la acción se rechaza. Con `--yes` todo se aprueba y se anota «aprobado por --yes».

Las respuestas también pueden llegar por tubería: si stdin no es una terminal, crest repite cada línea leída para que quede en la transcripción.

```bash
printf 'y\n' | crest --headless "agrega un test para parse()" --workspace .
```

Comportamiento de `Ctrl+C` en un turno headless:

- el primero cancela el turno («^C interrumpiendo el turno (otra vez para salir)») y sale con 130 después de guardar la sesión;
- el segundo sale de inmediato con 130.

En `--research` y `--implement`, en cambio, `Ctrl+C` mata el proceso: no hay cancelación ordenada, y el flujo queda en su último checkpoint.

Sin API key, headless termina con el error de la key y el código 2. La ventana, en cambio, abre igual y muestra ese error como primer bloque.

## Sesiones y reanudación

- Cada sesión (historial que se envía al modelo, timeline y reglas «siempre») se guarda en SQLite (`~/.local/share/crest/sessions.db`) **después de cada turno**.
- **Ventana**: al abrir, reanuda la **última sesión del workspace** (la identifica por la ruta canónica del directorio). Redibuja su timeline y avisa «sesión anterior reanudada (N turnos); `crest --new` empieza una nueva». Con `--new` empieza en blanco, y `:clear` hace lo mismo sin cerrar.
- **Headless**: cada ejecución es una sesión nueva salvo con `--continue`, que retoma la última del workspace (la misma que abriría la ventana).
- Las sesiones viejas no se borran y no hay un comando para listarlas ni para elegir una que no sea la última.
- El costo acumulado no se guarda: una sesión reanudada empieza en `$0.00`.
- **Compactación**: cuando el contexto estimado supera el 80 % de 200 000 tokens, crest resume los turnos viejos con el rol `compact`, conserva los 2 más recientes y avisa «historial compactado: N turnos → resumen». Si la API responde que el contexto es demasiado largo, compacta y reintenta una vez. El umbral no es configurable.
- Un turno puede hacer como mucho 50 llamadas al modelo. Si las supera, falla.

## Tools del agente

| Tool | Input | Efecto |
| --- | --- | --- |
| `list_files` | `glob?`, `limit` (500) | read |
| `find_file` | `query`, `limit` (20) | read |
| `tree` | `path?`, `depth` (3) | read |
| `grep` | `pattern`, `path?`, `glob?`, `fixed`, `ignore_case`, `context` (2), `limit` (100) | read |
| `read_file` | `path`, `offset?`, `limit?` (2000 líneas; no lee binarios ni archivos de más de 2 MiB sin rango) | read |
| `edit_file` | `path`, `old`, `new` (`old` debe aparecer exactamente una vez) | write |
| `write_file` | `path`, `content` | write |
| `run_command` | `command`, `args[]`, `timeout_s` (120). Sin shell, en la raíz del workspace. | execute (read para algunos `git`) |
| `show_chart`, `show_table`, `show_mermaid`, `show_markdown` | datos del bloque | read |
| `task` | `description`, `prompt`: lanza un subagente con el rol `explore` que solo puede leer (máximo 25 llamadas) y devuelve un resumen | read |
| `mcp__<servidor>__<tool>` | el schema del servidor | el `effect` del servidor (por defecto execute) |
| tools de plugins | su `schema` | su `effect` (por defecto execute) |

Todas las rutas son relativas al workspace. Una ruta que sale de él, incluso a través de un symlink, falla, igual que una ruta sensible. Cada resultado se recorta a unos 32 KB, con la nota «N resultados más; refina la búsqueda».

## Servidores MCP y confianza

Los servidores se declaran en Lua ([plugins.md](plugins.md#appmcp-servidores-mcp)). Sus tools aparecen como `mcp__<servidor>__<tool>` y pasan por las mismas aprobaciones que el resto, según el `effect` del servidor.

- **Config global y plugins**: arrancan al abrir crest.
- **Config del workspace** (`.crest/init.lua`): **no arrancan** hasta que confías en ellos, porque ejecutarlos significa correr un programa o contactar una URL que eligió quien escribió el repositorio. Hasta entonces ves el aviso:

  ```text
  servidor MCP `tickets` del workspace no iniciado: conectaría con https://tickets.example.com/mcp.
  Si confías en él, arranca con `crest --trust-mcp tickets` (se recuerda para este workspace)
  ```

  Para confiar en uno:
  - `crest --trust-mcp tickets` lo aprueba y lo arranca en esta misma ejecución;
  - `:mcp trust tickets`, desde la ventana, lo aprueba y arranca la próxima vez que abras crest.

  La aprobación se guarda en `~/.local/share/crest/mcp-trust.json`, por workspace y con el transporte exacto (comando, argumentos, entorno, URL, headers). Si el repositorio cambia la declaración, vuelve a quedar pendiente. Para revocarla, borra la entrada del archivo.
- Al arrancar, crest espera hasta 5 s a que los servidores estén listos. Uno que falla o tarda se reporta como aviso, y crest sigue sin él.
