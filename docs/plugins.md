# Escribir plugins

Un plugin es un `init.lua` que usa la tabla `app` para añadir cosas a crest:

- tools que el modelo puede llamar;
- hooks alrededor de cada tool;
- comandos `:`;
- atajos de teclado;
- bloques `Custom` en el timeline;
- servidores MCP.

La config global y la del workspace usan la misma API. Las opciones declarativas (`app.opt`, `app.models`, permisos) están en [configuracion.md](configuracion.md).

## Dónde viven y orden de carga

```text
~/.config/crest/                 ($XDG_CONFIG_HOME/crest/)
  init.lua                       config global
  plugins/
    guardia/init.lua             plugin "guardia"
    todo/init.lua                plugin "todo"
<workspace>/.crest/init.lua      config del workspace
```

1. Primero corre la config global.
2. Después, cada `plugins/<nombre>/init.lua`, en orden alfabético por nombre de carpeta.
3. Luego las carpetas que liste `app.plugins` en la config global y que no estuvieran ya en `plugins/`. Cada entrada puede empezar con `~/`; si no, se resuelve relativa al directorio de la config global. En cada carpeta se carga su `init.lua`.
4. Al final corre la config del workspace.

Los plugins solo se cargan desde la config global: un `app.plugins` en el workspace se ignora, con un aviso. Aun así, el `init.lua` del workspace puede usar toda la API (tools, hooks, keymaps, comandos), porque es otra capa más. Cuando algo choca, la precedencia es workspace > config global > plugins (ver [configuracion.md](configuracion.md#orden-de-carga-y-precedencia)).

El nombre del plugin es el de su carpeta. Las tools, los hooks y los bloques quedan atribuidos a ese nombre, y la config global y la del workspace aparecen como `config` y `workspace`.

Cada archivo corre **una vez al arrancar**. Las funciones que registra (handlers, hooks, callbacks) se quedan vivas en la VM y se llaman más tarde. Si cambias un plugin, reabre crest.

## El sandbox

Todo el Lua (la config y los plugins) corre en una única VM Lua 5.4. Esa VM vive en su propio hilo, `crest-lua`, y atiende las llamadas de una en una.

| Disponible | No disponible |
| --- | --- |
| `string` (sin `string.dump`), `table`, `math`, `utf8`, `coroutine` | `io`, `debug`, `package` |
| `os.time`, `os.clock`, `os.date`, `os.difftime` | `os.execute`, `os.remove`, `os.rename`, `os.exit`, `os.getenv`, `os.tmpname`, `os.setlocale` |
| `load` solo con texto | `dofile`, `loadfile`, bytecode (un `init.lua` binario se rechaza) |
| `require("app")`, que devuelve la tabla `app` del archivo | `require` de cualquier otro módulo o ruta |
| `print(...)`, que va al log (`CREST_LOG=warn,lua=info` para verlo) | Red, shell y sistema de archivos |

Hay dos límites:

- **Instrucciones**: cada llamada (la carga de un archivo, un handler, un hook, un callback) puede ejecutar como mucho **100 000 000** instrucciones de la VM. Si se pasa, se corta con el error `instruction limit exceeded`. `pcall`, `xpcall` y `coroutine.resume` vuelven a lanzar ese error, así que un plugin no puede atraparlo y seguir en el bucle.
- **Memoria**: toda la VM tiene un tope de **256 MiB**. Si se excede, el error es `out of memory`.

Cada archivo tiene su propio entorno: sus variables globales no se ven desde los demás archivos. Dentro de un mismo archivo, las variables `local` capturadas por tus funciones sí persisten entre llamadas. La tabla `app` es propia de cada archivo, así que leer `app.opt.leader` solo devuelve lo que puso ese archivo.

## La tabla `app`

| Campo | Firma | Cuándo usarla |
| --- | --- | --- |
| `app.opt` | tabla | Opciones (ver [configuracion.md](configuracion.md#opciones-appopt)). |
| `app.models.roles`, `app.models.aliases` | tablas | Modelos por rol. |
| `app.permissions.sensitive_paths` | lista de strings | Rutas que las tools no pueden leer. |
| `app.plugins` | lista de strings | Plugins extra (solo en la config global). |
| `app.keymap.set(modo, teclas, acción)` | ver abajo | Al cargar. |
| `app.commands.register(nombre, fn)` | ver abajo | Al cargar. |
| `app.tools.register(def)` | ver abajo | Al cargar. |
| `app.on(evento, fn)` | ver abajo | Al cargar. |
| `app.mcp` / `app.mcp.add(nombre, def)` | ver abajo | Al cargar. |
| `app.cmd(nombre)` | `string → nil` | Dentro de un callback de keymap o comando. |
| `app.send(texto [, { role = ... }])` | `string, tabla? → nil` | Dentro de un callback de keymap o comando. |
| `app.ui.*` | constructores de nodos | Dentro de un handler de tool (o de un callback). |

Un argumento mal formado en `keymap.set`, `commands.register`, `tools.register`, `app.on` o `mcp.add` **no** detiene el archivo: genera un diagnóstico con la línea de la llamada y se ignora solo esa llamada (ver [Errores y diagnósticos](#errores-y-diagnósticos)).

### `app.keymap.set(modo, teclas, acción)`

```lua
app.keymap.set("n", "<leader>r", "picker.grep")               -- acción integrada
app.keymap.set({ "n", "i" }, "<C-x>", function() app.cmd("clear") end)
```

- **`modo`**: un modo o una lista de modos. Cada modo puede ser `"n"`/`"normal"`, `"i"`/`"insert"` o `"c"`/`"command"`.
- **`teclas`**: la secuencia en notación de Neovim:
  - caracteres sueltos (`gg`);
  - `<leader>`;
  - modificadores `<C-x>` (Ctrl), `<A-x>` o `<M-x>` (Alt), `<S-x>` (Shift), `<D-x>` (Cmd);
  - nombres de tecla `<CR>`/`<Enter>`, `<Esc>`, `<Tab>`, `<BS>`, `<Del>`, `<Space>`, `<lt>`, `<gt>`, `<Bar>`, `<Bslash>`, `<Up>`, `<Down>`, `<Left>`, `<Right>`, `<Home>`, `<End>`, `<PageUp>`, `<PageDown>` y `<F1>`–`<F24>`.

  `<leader>` se expande al `app.opt.leader` vigente cuando el keymap se aplica.
- **`acción`**: el nombre de una acción integrada o una función Lua. La función se llama sin argumentos (`nil`) y puede usar `app.cmd` y `app.send`. Si devuelve `app.ui.block(...)`, ese bloque se añade al timeline.

Un keymap reemplaza al que tenga las mismas teclas en el mismo modo, incluidos los integrados. Estas son las acciones integradas válidas como string:

| Acción | Qué hace |
| --- | --- |
| `mode.normal`, `mode.insert`, `mode.command` | Cambia de modo. |
| `submit`, `newline` | Envía el prompt / inserta una nueva línea (insert). |
| `block.next`, `block.prev`, `block.first`, `block.last` | Mueve la selección del timeline. |
| `block.toggle` | Expande o colapsa el bloque seleccionado. |
| `block.toggle_view` | Vista alternativa (gráfica ↔ tabla, documento ↔ fuente). |
| `block.yank` | Copia el bloque al portapapeles. |
| `picker.files`, `picker.grep` | Abre un selector. |
| `panel.tree` | Muestra u oculta el árbol. |
| `model.pick` | Menú de modelo del siguiente turno. |
| `permission.cycle` | Cicla el modo de permisos. |
| `turn.interrupt` | Interrumpe el turno. |

Cualquier otro nombre se acepta al cargar, pero al pulsarlo solo muestra «la acción … todavía no hace nada». Por ejemplo, `session.resume` no está implementado.

### `app.commands.register(nombre, fn)`

```lua
app.commands.register("review", function(args)
  app.send("Revisa los cambios sin commitear. " .. args, { role = "plan" })
end)
```

- El comando se usa como `:review solo src/`, y `fn` recibe el resto de la línea como string (`"solo src/"`, o `""` si no hay nada).
- Si el nombre empieza con `/`, se ignora esa barra.
- Los comandos integrados `q`, `quit`, `qa`, `model`, `mode`, `clear`, `implement`, `research` y `mcp` se evalúan antes, así que un comando con alguno de esos nombres nunca se ejecuta.
- Como en los keymaps, si `fn` devuelve `app.ui.block(...)`, se muestra el bloque. Cualquier otro valor de retorno se ignora.

### `app.cmd(nombre)` y `app.send(texto, opts)`

Solo tienen efecto dentro de un callback de keymap o de comando. Dentro de un handler de tool o de un hook se descartan sin error. Las acciones se acumulan en orden y la ventana las ejecuta cuando el callback termina.

- **`app.cmd("clear")`** ejecuta un comando `:` exactamente como si lo escribieras: un integrado, uno registrado o un nombre con punto, que corre esa acción (`app.cmd("picker.files")`). Un comando puede llamar a otro hasta 4 niveles de anidación; más allá se ignora con un aviso.
- **`app.send(texto, { role = "plan" })`** envía `texto` como prompt de un turno nuevo, con ese rol de modelo (`main`, `plan`, `explore`, `compact`, `title`, `component` o `deep`). Sin `role`, el turno usa `main`. Si ya hay un turno en curso, el prompt queda en cola. Un rol desconocido es un error de Lua.

### `app.tools.register(def)`

```lua
app.tools.register({
  name = "todo_list",                 -- obligatorio, ^[a-zA-Z0-9_-]{1,128}$
  description = "Muestra tareas pendientes del proyecto",
  schema = { type = "object", properties = { items = { type = "array" } } },
  effect = "read",                    -- "read" | "write" | "execute" (defecto) | "network"
  handler = function(input) ... end,  -- obligatorio
})
```

| Campo | Tipo | Notas |
| --- | --- | --- |
| `name` | string | El nombre con el que el modelo ve la tool. No puede repetir el de una tool integrada (`read_file`, `grep`, `show_chart`, `task`...): una tool así se ignora, con el aviso «tool de plugin `x` ignorada». Entre capas Lua gana la de mayor precedencia. |
| `description` | string | Lo que lee el modelo para decidir cuándo usarla. |
| `schema` | tabla JSON Schema | El input que espera. Por defecto `{ type = "object" }`. El input del modelo debe ser siempre un objeto. |
| `effect` | string | Lo que usa la política de permisos (ver la tabla en [configuracion.md](configuracion.md#permisos-y-rutas-sensibles)). **Por defecto es `"execute"`**, así que la tool pide aprobación en `ask` y `auto-edit` y se niega en `plan`. Una tool que solo lee debe declarar `effect = "read"`. |
| `handler` | función | Recibe el input del modelo como tabla. |

El diálogo de aprobación muestra `plugin `<plugin>`: <tool> <input JSON>`, y «permitir siempre» se aplica a ese input exacto.

**Valor de retorno del handler**:

| Devuelve | El modelo recibe | El usuario ve |
| --- | --- | --- |
| `nil` | `(no output)` | El bloque ToolCall. |
| un string | el string tal cual | El ToolCall, con el texto al expandirlo. |
| un número, booleano o tabla | su JSON | Ídem. |
| `app.ui.block(nombre, nodo)` | `[shown to the user as a block]` | Un bloque `Custom` en el timeline. |
| un error de Lua (`error("...")`) o el límite de instrucciones | un error (`is_error`) con el mensaje | El ToolCall en estado fallido. |

El resultado pasa por el mismo presupuesto que las tools integradas (unos 32 KB de texto). Si es más largo, se recorta con una nota.

### `app.on(evento, fn)`: hooks

Hay dos eventos: `"before_tool"` y `"after_tool"`. Cada hook se aplica a **todas** las tools: las integradas, las de plugins, las MCP y `task`. Varios hooks del mismo evento corren en el orden en que se cargaron los archivos.

**`before_tool`** recibe `{ type = "before_tool", name = <tool>, input = <tabla> }` y puede devolver:

| Retorno | Efecto |
| --- | --- |
| `nil` (o cualquier cosa que no sea una de las dos de abajo) | Sigue sin cambios. |
| `{ deny = true, reason = "..." }` | La tool no corre. El modelo recibe `denied by hook: <reason>` y el ToolCall queda como denegado. Los hooks siguientes no corren. |
| `{ input = { ... } }` | La tool corre con este input, que **reemplaza entero** al original. Los hooks siguientes ya ven el input nuevo. |

El hook corre **antes** de la política de permisos y de la aprobación. Un input reescrito se muestra y se aprueba como si lo hubiera enviado el modelo. Un hook no puede saltarse una aprobación: solo puede negar o cambiar el input.

**`after_tool`** recibe `{ type = "after_tool", name, input, output = { text = "...", is_error = bool } }` cuando la tool terminó (también si falló). Su valor de retorno se ignora. Las llamadas negadas, ya sea por un hook, por la política o por el usuario, no llegan a `after_tool`.

Si un hook lanza un error o agota el límite de instrucciones, aparece el aviso «before_tool hook failed for `x`: …» y la llamada sigue con su input original.

Los inputs de las tools integradas que más se suelen filtrar son estos (la referencia completa está en [uso.md](uso.md#tools-del-agente)):

```text
run_command  { command = "git", args = { "push" }, timeout_s = 120 }   -- sin shell: command es el programa
edit_file    { path = "src/a.rs", old = "...", new = "..." }
write_file   { path = "notas.md", content = "..." }
read_file    { path = "src/a.rs", offset = 1, limit = 2000 }
```

### `app.ui`: bloques Custom

Los constructores devuelven tablas planas que describen nodos. Lua nunca toca GPUI: la ventana convierte el árbol en componentes. Donde se espera un nodo, un string o un número se convierte en un nodo de texto.

| Constructor | Nodo | Detalles |
| --- | --- | --- |
| `app.ui.text(s)` | texto | `s` es un string o un número (no una tabla). |
| `app.ui.row(hijos)` | fila horizontal | `hijos` es una lista de nodos o strings. |
| `app.ui.col(hijos)` | columna vertical | Ídem. |
| `app.ui.list(items)` | lista con viñetas | Ídem. |
| `app.ui.table({ columns = ..., rows = ... })` | tabla | `columns` es una lista no vacía de nombres o de `{ name = "x", align = "left" \| "center" \| "right" }`. `rows` es una lista de filas, y cada fila es una lista de celdas escalares (string, número, booleano). Toda fila debe tener una celda por columna. |
| `app.ui.chart({ title, kind, labels, series })` | gráfica | `kind` es `"bar"` (por defecto), `"line"`, `"area"` o `"pie"`. `labels` es una lista de strings y `series` es `{ { name = "x", values = { 1, 2 } }, ... }`. Hace falta al menos una serie, cada serie tiene un valor finito por etiqueta y `pie` lleva exactamente una serie. |
| `app.ui.code(lang, src)` | código resaltado | `lang` puede ser `nil`. |
| `app.ui.markdown(src)` | markdown renderizado | |
| `app.ui.block(nombre, nodo)` | el bloque `Custom` | Valida el árbol entero y lo envuelve como bloque del plugin. Hay que **devolverlo** desde el handler. |

Un argumento inválido (una fila de tabla de otro ancho, una serie de otra longitud...) lanza un error de Lua con el prefijo `app.ui.<función>:`, y la tool falla con ese mensaje. Si anidas más de lo que la ventana dibuja, el exceso se corta con una marca.

### `app.mcp`: servidores MCP

Hay dos formas equivalentes de declararlos:

```lua
app.mcp = {
  git  = { command = "mcp-git", args = { "--repo", "." }, env = { RUST_LOG = "warn" } },
  docs = { url = "https://example.test/mcp", headers = { Authorization = "Bearer ..." } },
}
-- o, uno a uno:
app.mcp.add("git", { command = "mcp-git", effect = "read", timeout = 30 })
```

| Campo | Para | Notas |
| --- | --- | --- |
| `command` | stdio | Un proceso hijo que habla MCP por stdin/stdout. |
| `args` | stdio | Lista de strings. |
| `env` | stdio | Tabla de strings. |
| `url` | HTTP | Streamable HTTP. Debe empezar con `http://` o `https://`. |
| `headers` | HTTP | Tabla de strings. |
| `effect` | ambos | Igual que en las tools de plugin. Por defecto `"execute"`. Se aplica a **todas** las tools del servidor. |
| `timeout` | ambos | Segundos (> 0) por llamada. Por defecto 120. |

- Cada servidor lleva `command` **o** `url`, nunca los dos.
- Un campo desconocido genera un diagnóstico.
- El nombre solo puede tener letras, dígitos, `-` y `_` sueltos (no `__`), porque las tools se exponen como `mcp__<servidor>__<tool>`.
- Si un mismo archivo usa las dos formas con el mismo nombre, gana `app.mcp.add`.

**Seguridad**:

- Los servidores de la config global y de los plugins arrancan al abrir crest.
- Los del workspace no arrancan hasta que el usuario los aprueba con `crest --trust-mcp <nombre>` o `:mcp trust <nombre>` (ver [uso.md](uso.md#servidores-mcp-y-confianza)). La aprobación se repite si cambia la declaración.

## Errores y diagnósticos

| Problema | Qué pasa |
| --- | --- |
| Error de sintaxis o de ejecución al cargar un archivo | Ese archivo no aporta nada y los demás se cargan. Se muestra un bloque `Error` con `archivo:línea: mensaje`. |
| Bucle infinito o exceso de memoria al cargar | Como el caso anterior, con `instruction limit exceeded` o `out of memory`. |
| Argumento inválido en una función de registro | Diagnóstico con la línea de la llamada. Solo se ignora esa llamada. |
| Campo declarativo inválido (`app.opt.x`, `app.models...`) | Diagnóstico sin línea. Se ignora solo ese campo. |
| Carpeta de `app.plugins` sin `init.lua` | Diagnóstico «cannot read the file». |
| Error en un handler de tool | El modelo recibe el error y el ToolCall queda fallido. |
| Error en un hook | Aviso en la barra de estado. La tool sigue. |
| Error en un callback de keymap o comando | Bloque `Error` con `Lua: <mensaje>`. |
| Keymap con notación inválida | Aviso «keymap `x` ignorado: …» al abrir la ventana. |

En headless los diagnósticos salen por la terminal. Ejecuta `crest --headless "hola"` para ver si tu config carga limpia.

## Ejemplos paso a paso

Los cuatro ejemplos se cargaron con la implementación actual (`LuaHost` real, sin diagnósticos), y se comprobó la respuesta de cada handler, hook y callback.

### 1. Una tool `todo_list` con bloque Custom

`~/.config/crest/plugins/todo/init.lua`:

```lua
local app = require("app")

app.tools.register({
  name = "todo_list",
  description = "Muestra al usuario una lista de tareas pendientes con su estado. "
    .. "Úsala cuando el usuario pida ver o resumir pendientes.",
  schema = {
    type = "object",
    properties = {
      items = {
        type = "array",
        items = {
          type = "object",
          properties = {
            tarea = { type = "string" },
            hecha = { type = "boolean" },
          },
          required = { "tarea" },
        },
      },
    },
    required = { "items" },
  },
  effect = "read",  -- solo muestra datos: corre sin pedir aprobación
  handler = function(input)
    local rows, hechas = {}, 0
    for i, item in ipairs(input.items) do
      local hecha = item.hecha == true
      if hecha then hechas = hechas + 1 end
      rows[i] = { item.tarea, hecha }
    end
    return app.ui.block("todo", app.ui.col({
      app.ui.text(string.format("%d de %d hechas", hechas, #input.items)),
      app.ui.table({ columns = { "tarea", { name = "hecha", align = "center" } }, rows = rows }),
      app.ui.chart({
        title = "Avance",
        kind = "pie",
        labels = { "hechas", "pendientes" },
        series = { { name = "tareas", values = { hechas, #input.items - hechas } } },
      }),
    }))
  end,
})
```

Para probarlo, abre crest y pide: «muestra con todo_list las tareas: parser (hecha), docs, release». El timeline muestra el ToolCall `todo_list` y debajo el bloque con el texto «1 de 3 hechas», la tabla y la gráfica de torta. El modelo solo recibe `[shown to the user as a block]`.

### 2. Un hook que bloquea `rm` y fuerza `git push --dry-run`

`~/.config/crest/plugins/guardia/init.lua`:

```lua
local app = require("app")

local prohibidos = { rm = true, shred = true }

app.on("before_tool", function(ev)
  if ev.name ~= "run_command" then return nil end
  if prohibidos[ev.input.command] then
    return { deny = true, reason = "`" .. ev.input.command .. "` está prohibido por el plugin guardia" }
  end
  -- git push siempre en seco
  if ev.input.command == "git" and ev.input.args and ev.input.args[1] == "push" then
    local args = { "push", "--dry-run" }
    for i = 2, #ev.input.args do args[#args + 1] = ev.input.args[i] end
    return { input = { command = "git", args = args, timeout_s = ev.input.timeout_s } }
  end
end)

app.on("after_tool", function(ev)
  if ev.output.is_error then
    print("falló " .. ev.name .. ": " .. ev.output.text)
  end
end)
```

Así responde el hook a cada input:

| Input | Resultado |
| --- | --- |
| `{ command = "rm", args = { "-rf", "x" } }` | Se niega con el motivo «`rm` está prohibido por el plugin guardia». |
| `{ command = "git", args = { "push", "origin", "main" } }` | Corre como `git push --dry-run origin main`, y la aprobación muestra ese comando. |
| `{ command = "ls" }` | Sigue sin cambios. |

Para probarlo, pide «borra el directorio target con rm -rf». El ToolCall de `run_command` queda denegado sin que aparezca el diálogo de aprobación. Para ver los `print`, arranca con `CREST_LOG=warn,lua=info`.

Recuerda que `run_command` no usa shell: `command` es el programa y `args` sus argumentos, así que un `sh -c "rm …"` llega como `command = "sh"`. Si quieres bloquear eso también, revisa `ev.input.args`.

### 3. Un comando que lanza un turno con el rol `plan`

En `init.lua` (global o del workspace):

```lua
local app = require("app")

app.commands.register("review", function(args)
  app.send("Revisa los cambios sin commitear y lista riesgos. " .. args, { role = "plan" })
end)
```

Para usarlo, escribe `Esc`, luego `:review solo src/` y luego `Enter`. Se envía el prompt «Revisa los cambios sin commitear y lista riesgos. solo src/», que resuelve el modelo del rol `plan` (por defecto `claude-opus-5-5`). La barra de estado muestra el modelo del turno.

### 4. Keymaps

```lua
local app = require("app")

app.opt.leader = ","

-- ,x en normal: sesión nueva
app.keymap.set("n", "<leader>x", function() app.cmd("clear") end)

-- Ctrl+P en normal e insert: selector de archivos
app.keymap.set({ "n", "i" }, "<C-p>", "picker.files")

-- ,t en normal: pide al agente que corra los tests
app.keymap.set("n", "<leader>t", function()
  app.send("Corre los tests del workspace y resume los fallos en una tabla")
end)
```

Con `leader = ","`, los atajos de este archivo usan la coma, pero los integrados (`<Space>ff`, `<Space>m`...) siguen con el espacio. Si quieres moverlos, vuelve a mapearlos:

```lua
app.keymap.set("n", "<leader>ff", "picker.files")
app.keymap.set("n", "<leader>fg", "picker.grep")
```

### Cómo probar un plugin

1. Arranca en headless sobre un directorio cualquiera: `crest --headless "lista los archivos" --workspace /tmp/prueba`. Los diagnósticos de carga salen en la terminal.
2. Abre la ventana y revisa que no haya bloques `Error` al principio del timeline.
3. Para tools: pide algo que las use, expande el ToolCall con `Enter` (en normal) y comprueba el input y el output.
4. Para hooks: usa `print` y `CREST_LOG=warn,lua=info`.
5. Si escribes Rust, los tests de `crates/adapters/lua/tests/` (`plugin_tools.rs`, `plugin_hooks.rs`, `plugin_ui.rs`) muestran cómo cargar un `init.lua` en un `LuaHost` real y correrlo dentro del loop del agente con un modelo guionado (`application::testing`).
