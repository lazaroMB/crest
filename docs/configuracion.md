# Configuración

crest se configura con archivos Lua (`init.lua`). Puedes no tener ninguno: sin config arranca con los valores por defecto que se describen abajo. Para escribir plugins (tools, hooks, bloques) mira [plugins.md](plugins.md). Esta guía cubre las opciones declarativas.

## Archivos y rutas

| Qué | Ruta | Notas |
| --- | --- | --- |
| Config global | `$XDG_CONFIG_HOME/crest/init.lua`, o `~/.config/crest/init.lua` | Un `XDG_CONFIG_HOME` relativo se ignora. Se usa la misma ruta en macOS (no `~/Library`). |
| Plugins | `<dir de la config global>/plugins/<nombre>/init.lua` | Se cargan en orden alfabético, y después los que liste `app.plugins`. |
| Config del workspace | `<workspace>/.crest/init.lua` | Es opcional. Puede venir con el repositorio. |
| Base de datos | `$XDG_DATA_HOME/crest/sessions.db`, o `~/.local/share/crest/sessions.db` | SQLite. Guarda las sesiones del chat y los checkpoints de los flujos. |
| Servidores MCP aprobados | `$XDG_DATA_HOME/crest/mcp-trust.json`, o `~/.local/share/crest/mcp-trust.json` | Se escribe con permisos `0600`. |
| Specs del flujo investigativo | `<workspace>/.crest/specs/NNNN-slug/` | Ver [flujos.md](flujos.md). |

### Orden de carga y precedencia

Los archivos se ejecutan en este orden:

1. La config global.
2. Los plugins, primero los de `plugins/` en orden alfabético y luego los de `app.plugins`.
3. La config del workspace.

Cada archivo corre en su propio entorno y tiene su propia tabla `app`. Un archivo no ve las variables globales ni la tabla `app` de los demás.

Al combinar las capas, la precedencia de menor a mayor es: plugins, config global, config del workspace. La config global le gana a sus plugins y el workspace les gana a ambos. El merge se hace así:

| Elemento | Cómo se combina |
| --- | --- |
| `app.opt.*` | Clave por clave. |
| `app.models.roles` | Rol por rol y campo por campo. Por ejemplo, si el workspace solo pone `max_tokens`, conserva el `id` y el `thinking` de la capa global. `fallbacks` se reemplaza entero. |
| `app.models.aliases` | Clave por clave. |
| Keymaps | Por `(modo, teclas)`: gana la última definición. |
| Comandos, tools y servidores MCP | Por nombre: gana la capa de mayor precedencia. |
| Hooks | Se conservan todos, en el orden en que corrieron los archivos (global, plugins, workspace). |
| `app.permissions.sensitive_paths` | La config global (o un plugin) puede **reemplazar** la lista por defecto. El workspace solo puede **añadir** patrones, así que un repositorio clonado no puede quitar la protección. |
| `app.plugins` | Solo se lee en la config global. En el workspace se ignora con un aviso. |

> La config del workspace es código Lua del repositorio: corre en el mismo sandbox que el resto (ver [plugins.md](plugins.md#el-sandbox)), y puede registrar tools, hooks, keymaps y comandos. Los servidores MCP que declare no arrancan hasta que los apruebes (ver [Servidores MCP](#servidores-mcp)).

### Errores en `init.lua`

Un archivo que falla (por error de sintaxis, error en tiempo de ejecución, límite de instrucciones o memoria) **no aporta nada**, pero los demás se cargan igual. crest arranca de todos modos. Cada problema aparece:

- en la ventana, como un bloque `Error` al principio del timeline, con el texto `archivo:línea: mensaje` y la sugerencia «crest arrancó sin lo que define ese archivo (valores por defecto); corrígelo y vuelve a abrir crest»;
- en headless, en la salida.

Un campo mal escrito (por ejemplo `app.opt.timeoutlen = "500"`) se descarta solo, con un diagnóstico, y el resto del archivo sí se aplica. Algunos mensajes típicos:

```text
/home/yo/.config/crest/init.lua:3: syntax error: unexpected symbol near '='
/home/yo/.config/crest/init.lua: app.opt.timeoutlen must be a whole number of milliseconds
/home/yo/.config/crest/init.lua: app.models.roles: unknown role `mian`
/home/yo/.config/crest/init.lua:12: unknown mode `x` (use "n", "i" or "c")
/home/yo/.config/crest/plugins/hang/init.lua: instruction limit exceeded
```

Los cambios en `init.lua` se leen al arrancar, así que hay que reabrir crest para que tengan efecto.

## API key de Anthropic

crest busca la clave en este orden:

1. La variable de entorno `ANTHROPIC_API_KEY`, si no está vacía.
2. El llavero del sistema (crate `keyring`), en el **servicio `crest`** y la **entrada (usuario) `ANTHROPIC_API_KEY`**.

La clave **nunca** va en `init.lua`, y no aparece en logs ni en trazas.

Para guardarla en el llavero:

```bash
# Linux (libsecret / GNOME Keyring, KWallet vía Secret Service)
secret-tool store --label="crest" service crest username ANTHROPIC_API_KEY

# macOS (Keychain)
security add-generic-password -s crest -a ANTHROPIC_API_KEY -w
```

> Los comandos de arriba son las herramientas estándar de cada sistema. crest solo lee la entrada `crest` / `ANTHROPIC_API_KEY` mediante `keyring`, y esa lectura no se ha probado en este entorno con un llavero real.

Si no hay clave:

- la ventana abre igual, y el primer bloque es el error «falta la API key de Anthropic: exporta ANTHROPIC_API_KEY o guárdala en el llavero del sistema (servicio `crest`, entrada `ANTHROPIC_API_KEY`)»;
- headless termina con código 2.

La URL de la API es fija (`https://api.anthropic.com`): no hay variable para cambiarla.

## Opciones `app.opt`

| Opción | Tipo | Defecto | Efecto |
| --- | --- | --- | --- |
| `leader` | string en notación de teclas (`" "`, `","`, `"<Space>"`) | `" "` (espacio) | Lo que vale `<leader>` en los keymaps **definidos en Lua**. Los atajos integrados siempre usan el espacio (ver [uso.md](uso.md#atajos-por-defecto)). |
| `timeoutlen` | entero, ms | `1000` | Cuánto espera un prefijo (`g` de `gg`, `<leader>`) a la siguiente tecla. |
| `pricing` | tabla `{ [modelo o prefijo] = { input, output, cache_read, cache_write } }` | vacía | Precios en USD por millón de tokens. Sin precios, el costo se muestra como `$?`. |
| `max_session_cost` | número > 0 (USD) | sin tope | Tope de gasto por sesión: al alcanzarlo, crest pide confirmación antes de la siguiente llamada al modelo. |
| `theme` | string | — | Se lee y se combina entre capas, pero **todavía no tiene efecto**: la ventana usa siempre el tema oscuro. |

Cualquier otra clave de `app.opt` se acepta sin error y se guarda, pero crest no la usa. No hay opciones de fuente: la ventana usa JetBrains Mono a 14 px si está instalada, y si no, la monoespaciada del sistema.

### Costo y tope de gasto

```lua
app.opt.pricing = {
  -- La clave es un id exacto o un prefijo; gana la clave más larga que coincida.
  ["claude-sonnet-5"] = { input = 3, output = 15, cache_read = 0.3, cache_write = 3.75 },
}
app.opt.max_session_cost = 5
```

- Los números son solo un ejemplo: crest no trae precios incorporados, así que toma los vigentes del proveedor. Los cuatro campos son obligatorios, deben ser números ≥ 0 y no se admiten otros.
- El costo se suma por respuesta, incluidos los resúmenes de la compactación. Se muestra en la barra de estado: `$0.00` antes de usar el modelo, `$?` si hubo uso de un modelo sin precio y, si no, el acumulado (cuatro decimales por debajo de 1 USD).
- Al llegar al tope, crest pregunta «La sesión gastó $X de $Y; ¿continuar?» con el diálogo de aprobación (en headless, por stdin):
  - **aprobar** sube el tope en otro tanto;
  - **siempre** quita el tope para la sesión;
  - **rechazar** termina el turno («turno detenido por el tope de gasto»).
- El costo vive en memoria: una sesión reanudada vuelve a contar desde cero.

## Modelos por rol

Cada llamada al modelo pide un **rol**, y la configuración decide qué modelo atiende cada rol.

| Rol | Para qué | `id` | `max_tokens` | `thinking` | `fallbacks` |
| --- | --- | --- | --- | --- | --- |
| `main` | Conversación y loop del agente | `claude-sonnet-5-5` | 16000 | presupuesto 8000 | `claude-haiku-5-5` |
| `plan` | Planes, diseño, refactors grandes; nodos de razonamiento de los flujos | `claude-opus-5-5` | 32000 | presupuesto 16000 | `claude-sonnet-5-5` |
| `explore` | Subagente `task` y nodos de exploración | `claude-haiku-5-5` | 8000 | no | — |
| `compact` | Resumir el historial | `claude-haiku-5-5` | 4000 | no | — |
| `title` | Títulos y nombres cortos (reservado: hoy ningún componente lo usa solo) | `claude-haiku-5-5` | 1024 | no | — |
| `component` | Datos estructurados para bloques (reservado: las tools `show_*` las llama el modelo del turno) | `claude-sonnet-5-5` | 16000 | no | `claude-haiku-5-5` |
| `deep` | Tareas muy difíciles, bajo demanda (`--model deep`, `:model deep`, `app.send(…, { role = "deep" })` o el `role` de una tarea de `plan.json`) | `claude-fable-5-1` | 32000 | presupuesto 16000 | `claude-opus-5-5`, `claude-sonnet-5-5` |

Alias incorporados: `opus` → `claude-opus-5-5`, `sonnet` → `claude-sonnet-5-5`, `haiku` → `claude-haiku-5-5` y `fable` → `claude-fable-5-1`.

### `app.models.roles`

```lua
app.models.roles = {
  main    = { id = "sonnet", max_tokens = 16000, thinking = { budget = 8000 }, fallbacks = { "haiku" } },
  plan    = { id = "opus", thinking = "adaptive" },
  explore = "haiku",                    -- atajo de { id = "haiku" }
  compact = { thinking = false },       -- solo cambia ese campo
}
```

| Campo | Formas aceptadas |
| --- | --- |
| (el rol entero) | Una tabla con los campos de abajo, o un string, que equivale a `{ id = "..." }`. |
| `id` | Un id de modelo o un alias. |
| `max_tokens` | Un entero positivo. |
| `thinking` | `false`, `"disabled"` u `"off"` (sin razonamiento); `"adaptive"`; un número N (presupuesto de N tokens); `{ budget = N }` o `{ budget_tokens = N }`; `{ type = "disabled" }` o `{ type = "adaptive" }`. `true` no es válido. |
| `fallbacks` | Una lista de ids o alias. Se prueban, en orden, ante errores 529 o 5xx del modelo principal. |

Un rol desconocido o un campo desconocido produce un diagnóstico. Los alias se expanden una sola vez (no son recursivos) en `id` y en `fallbacks`.

### `app.models.aliases`

```lua
app.models.aliases = { rapido = "claude-haiku-5-5", opus = "claude-opus-5-5" }
```

Los alias se combinan con los incorporados (el workspace gana) y aparecen en el menú `<leader>m` de la ventana.

### Orden de resolución

1. **Override del turno**: `--model`, `:model <x>` o el menú `<leader>m`.
   - Si el valor es un **nombre de rol** (`plan`), se usa la spec completa de ese rol.
   - Si es un alias o un id, solo reemplaza el `id` y conserva `max_tokens`, `thinking` y `fallbacks` del rol.
   - Los alias se comprueban antes que los nombres de rol.
2. **Rol fijado** por una tool o un flujo (por ejemplo, el subagente `task` pide `explore`, y `app.send(..., { role = "plan" })` pide `plan`).
3. Config del workspace.
4. Config global, con sus plugins.
5. Valores incorporados (la tabla de arriba).

Al arrancar, crest consulta `GET /v1/models` y espera como mucho 5 s. Si un rol apunta a un modelo que no existe, se muestra un aviso y se usa el primer fallback disponible. Si el catálogo no responde, aparece el aviso «no pude validar los modelos configurados» y no se valida nada.

## Permisos y rutas sensibles

El modo de permisos inicial se elige con `--mode` (por defecto `ask`). No se configura en Lua. Cómo cambiarlo en la ventana está en [uso.md](uso.md#aprobaciones-y-modos-de-permiso).

| Efecto de la tool | `ask` | `auto-edit` | `plan` |
| --- | --- | --- | --- |
| `read` (leer, buscar, bloques, `task`, `git log/diff/status/show/blame`) | permite | permite | permite |
| `write` (`edit_file`, `write_file`) | pregunta | permite | niega |
| `execute` (`run_command`, tools de plugin o MCP sin `effect`) | pregunta | pregunta | niega |
| `network` | pregunta | pregunta | niega |

**Rutas sensibles**: son rutas que ninguna tool puede leer ni escribir.

- La lista por defecto es `.env*`, `*.pem`, `id_rsa*` y `.git/`.
- Cada patrón se compara con **cada componente** de la ruta: solo admite `*` y `?`, y no puede contener `/` salvo una `/` final, que marca un directorio. Por ejemplo, `.env*` niega `.env` y `config/.env.local`, y `.git/` niega todo lo que está dentro de `.git`.

```lua
-- Config global: reemplaza la lista por defecto (incluye las que quieras conservar).
app.permissions.sensitive_paths = { ".env*", "*.pem", "id_rsa*", ".git/", "*.key" }

-- Config del workspace: solo AÑADE a la lista vigente.
app.permissions.sensitive_paths = { "secrets/" }
```

Un patrón inválido (vacío, o con `/` en medio) hace que crest use la lista por defecto y muestre un aviso.

Las reglas «permitir siempre» que concedes en un diálogo de aprobación se guardan con la sesión y vuelven cuando la sesión se reanuda. No hay sintaxis Lua para declararlas.

## Servidores MCP

Se declaran en `app.mcp`, ya sea como tabla por nombre o con `app.mcp.add`. Todos los campos se detallan en [plugins.md](plugins.md#appmcp-servidores-mcp).

```lua
app.mcp.add("docs", { command = "mcp-docs", args = { "--root", "." }, effect = "read", timeout = 30 })
app.mcp = { tickets = { url = "https://tickets.example.com/mcp", headers = { Authorization = "Bearer …" } } }
```

- Los servidores de la **config global** (y de sus plugins) arrancan al abrir crest.
- Los del **workspace** quedan pendientes, con un aviso del estilo «servidor MCP `x` del workspace no iniciado: ejecutaría `…`». Arrancan cuando los apruebas con `crest --trust-mcp <nombre>` o con `:mcp trust <nombre>` (este último surte efecto la próxima vez que abras crest).
- La aprobación se guarda por workspace en `mcp-trust.json`, junto con el transporte exacto: si el comando, los argumentos, el entorno, la URL o los headers cambian, crest vuelve a pedirla. Para revocar una aprobación, borra su entrada del archivo.
- Al arrancar, crest espera hasta 5 s a que los servidores estén listos. El límite por llamada a una tool es `timeout` si lo declaras, o 120 s si no.

## Variables de entorno

| Variable | Efecto |
| --- | --- |
| `ANTHROPIC_API_KEY` | La API key. Tiene prioridad sobre el llavero. |
| `XDG_CONFIG_HOME` | Base de la config global y de los plugins (`$XDG_CONFIG_HOME/crest/`). Solo se usa si es absoluta. |
| `XDG_DATA_HOME` | Base de `sessions.db` y `mcp-trust.json` (`$XDG_DATA_HOME/crest/`). Solo se usa si es absoluta. |
| `CREST_LOG` | Filtro del log en sintaxis `EnvFilter` de `tracing`, por ejemplo `info`, `debug` o `warn,lua=info`. Por defecto es `warn`. El log va a stderr. |
| `LANGFUSE_PUBLIC_KEY`, `LANGFUSE_SECRET_KEY` | Activan la exportación de trazas a Langfuse, **solo en un binario compilado con `--features langfuse`**. Hacen falta las dos: si falta una, crest avisa y no exporta. |
| `LANGFUSE_HOST` | Instancia de Langfuse. Por defecto `https://cloud.langfuse.com`. |
| `ANTHROPIC_LIVE_TEST` | Solo para desarrollo: `make test-live` la pone en `1`. |

El `print()` de Lua va al log con nivel `info` y target `lua`. Para verlo: `CREST_LOG=warn,lua=info crest`.

## Telemetría y Langfuse

crest emite spans de `tracing` por turno, por llamada al modelo y por tool. Por defecto solo se registran en el log, según `CREST_LOG`. Para enviarlos a Langfuse:

```bash
cargo build --release -p crest --features langfuse
export LANGFUSE_PUBLIC_KEY=pk-lf-...
export LANGFUSE_SECRET_KEY=sk-lf-...
# export LANGFUSE_HOST=https://mi-langfuse.example.com
./target/release/crest
```

`make release` y los binarios de release se compilan **sin** esa feature. La telemetría nunca impide arrancar: cualquier problema se muestra como aviso.

## Ejemplo completo

Una config global, comprobada contra la implementación actual (carga sin diagnósticos, salvo el plugin listado si su carpeta no existe):

```lua
-- ~/.config/crest/init.lua
local app = require("app")

-- Teclas -----------------------------------------------------------------
app.opt.leader = " "        -- <leader> de los keymaps de este archivo
app.opt.timeoutlen = 600    -- ms de espera entre teclas de una secuencia

-- Costo (USD por millón de tokens; ejemplo, usa los precios vigentes) ----
app.opt.pricing = {
  ["claude-sonnet-5"] = { input = 3, output = 15, cache_read = 0.3, cache_write = 3.75 },
  ["claude-haiku-5"]  = { input = 1, output = 5,  cache_read = 0.1, cache_write = 1.25 },
}
app.opt.max_session_cost = 5  -- pausa y pregunta al llegar a 5 USD

-- Modelos por rol --------------------------------------------------------
app.models.aliases = {
  rapido = "claude-haiku-5-5",
}
app.models.roles = {
  main    = { id = "sonnet", max_tokens = 16000, thinking = { budget = 8000 },
              fallbacks = { "haiku" } },
  plan    = { id = "opus", thinking = "adaptive" },
  explore = "rapido",                 -- atajo de { id = "rapido" }
  compact = { max_tokens = 4000, thinking = false },
}

-- Rutas que las tools no pueden leer (reemplaza la lista por defecto) -----
app.permissions.sensitive_paths = { ".env*", "*.pem", "id_rsa*", ".git/", "*.key" }

-- Servidores MCP de la config global: arrancan sin pedir confianza --------
app.mcp.add("docs", {
  command = "npx",
  args = { "-y", "@modelcontextprotocol/server-filesystem", "/home/yo/notas" },
  effect = "read",
  timeout = 30,
})

-- Plugins fuera de ~/.config/crest/plugins/ ------------------------------
app.plugins = { "~/src/mis-plugins/revisor" }

-- Atajos y comandos ------------------------------------------------------
app.keymap.set("n", "<leader>x", function() app.cmd("clear") end)
app.keymap.set({ "n", "i" }, "<C-p>", "picker.files")

app.commands.register("review", function(args)
  app.send("Revisa los cambios sin commitear y lista riesgos. " .. args, { role = "plan" })
end)
```

Y una config de workspace que la ajusta:

```lua
-- <workspace>/.crest/init.lua
local app = require("app")

app.opt.max_session_cost = 2          -- más estricto en este repo
app.models.roles = { main = { max_tokens = 8000 } }  -- solo cambia max_tokens
app.permissions.sensitive_paths = { "secrets/" }     -- se AÑADE a la lista global

-- Un servidor del repo: queda pendiente hasta `crest --trust-mcp tickets`
app.mcp = {
  tickets = { url = "https://tickets.example.com/mcp", headers = { Authorization = "Bearer x" } },
}

app.commands.register("tests", function()
  app.send("Corre `cargo test --workspace` y resume los fallos en una tabla")
end)
```

Con las dos juntas queda así:

- `main` usa `claude-sonnet-5-5` con `max_tokens = 8000`, presupuesto de razonamiento 8000 y fallback `claude-haiku-5-5`.
- El tope de gasto es 2 USD.
- Las rutas sensibles son `.env*`, `*.pem`, `id_rsa*`, `.git/`, `*.key` y `secrets/`.
- `docs` arranca y `tickets` queda pendiente de aprobación.
