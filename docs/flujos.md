# Flujos: investigación e implementación

crest trae dos flujos guiados que corren dentro del chat:

- El **flujo investigativo** sigue las etapas del [X-Workflow de Hashlabs](https://www.hashlabs.com/guides/en/methodology/X-Workflow.html). Parte de un problema descrito en lenguaje natural y termina en un **Spec** con su plan de implementación.
- El **flujo de implementación** toma el `plan.json` de un Spec y lo ejecuta tarea por tarea, con un commit por tarea.

Los dos están hechos con [graph-flow](https://crates.io/crates/graph-flow). Cada paso es un checkpoint en SQLite, en `~/.local/share/crest/sessions.db`. En las **compuertas**, el flujo se detiene y espera tu respuesta en el mismo input del chat (en la ventana) o en stdin (en headless).

Mientras un flujo está activo, **cada línea que envías va al flujo** y no inicia un turno normal. Los comandos `:` de la ventana (`:model`, `:mode`, `:clear`, `:q`...) siguen funcionando. Si hay un flujo de implementación y uno investigativo activos a la vez, las líneas van primero al de implementación.

> Limitación: `Ctrl+C` no detiene un paso en curso de ningún flujo. En la ventana, el token de cancelación no llega al flujo. En headless, mata el proceso, y al reanudar el paso interrumpido vuelve a correr. Para detener un flujo de forma ordenada, responde `/abort` en una compuerta.

## Flujo investigativo

### Iniciar, consultar y reanudar

| Dónde | Comando | Qué hace |
| --- | --- | --- |
| Ventana | `:research <descripción del problema>` | Inicia un flujo nuevo y lo corre hasta la primera compuerta. Si había otro, lo **reemplaza** sin preguntar: el viejo queda guardado, pero ya no se puede retomar. |
| Ventana | `:research resume` (también `retomar` o `reanudar`) | Retoma el último flujo del workspace. |
| Ventana | `:research` | Dice en qué etapa va el flujo. |
| Headless | `crest --headless --research "<problema>"` | Corre el flujo en la terminal y lee las respuestas de stdin. |
| Headless | `crest --headless --research resume` | Retoma el flujo pendiente. |

Al abrir crest con un flujo a medias aparece el aviso «hay un flujo investigativo pendiente (…); `:research resume` para seguir». El flujo **no** se activa solo: hasta que escribas `:research resume`, tus líneas van al chat normal.

Al retomar:

- si el flujo esperaba una respuesta, crest vuelve a mostrar la pregunta de la compuerta;
- si se había detenido por un error, sigue desde el último checkpoint;
- si estaba terminado, queda activo solo para que puedas usar `/back <nodo>`, y cualquier otra línea lo libera;
- si estaba abortado, no se puede retomar.

Solo hay un flujo rastreado por workspace: el último que iniciaste.

### Los 14 nodos

| # | Nodo | Etapa | Rol | Compuerta | Qué hace | Bloques |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | Intake | Descubrimiento | `main` | sí | Reformula el pedido, lo clasifica (feature, bug, performance, refactor) y lista preguntas abiertas. | Text |
| 2 | Observación | Descubrimiento | `explore` | | Recorre el repo con un agente de solo lectura (árbol, archivos clave, `git log`) y dibuja la arquitectura actual. | Mermaid, Table, Sources |
| 3 | Datos | Descubrimiento | `explore` | | Reúne evidencia: commits de los archivos implicados (hasta 20), hallazgos y, si hay librerías, documentación en la web. **Opcional.** | Chart (commits por semana), Table, Markdown, Sources |
| 4 | Análisis | Descubrimiento | `main` | | Contrasta ideas con la evidencia; cada hallazgo cita evidencia existente. | Table, Chart |
| 5 | Hipótesis | Descubrimiento | `plan` | sí | Hipótesis de una oración, diagrama de causas (Ishikawa) y tabla priorizada. | Mermaid, Table |
| 6 | Experimentos | Descubrimiento | `main` | | Diseña y corre experimentos con el agente del chat (con sus tools y aprobaciones). Una hipótesis solo queda «apoyada» o «refutada» si el comando de verdad corrió. | Table, Chart (más los ToolCall y Diff del agente) |
| 7 | Definición y restricciones | Definición | `plan` | sí | Define el problema en una oración **medible** (puede haber varias; cada una da un Spec) y la tabla de restricciones. | Markdown, Table |
| 8 | Investigación | Búsqueda de soluciones | `explore` | | Busca soluciones previas en el repo y en la web (hasta 5 búsquedas y 3 páginas). | Sources, Table |
| 9 | Lluvia de ideas | Búsqueda de soluciones | `plan` | | Genera opciones sin filtrar. | Table |
| 10 | Modelado | Búsqueda de soluciones | `plan` | sí | Descarta las opciones que violan restricciones y arma la matriz de decisión (puntajes 1-5) y la caja negra de la opción elegida. | Table, Mermaid, Markdown |
| 11 | Prototipo | Búsqueda de soluciones | `main` | pregunta | Spike opcional en un `git worktree` temporal. **Opcional.** | ToolCall, Diff, Markdown |
| 12 | Spec | Especificación | `plan` | | Redacta el Spec (`spec.md`). | Markdown, Sources |
| 13 | Plan | Especificación | `plan` | | Divide el Spec en tareas (`plan.json` y `plan.md`) y valida que no haya ciclos. | Mermaid, Table |
| 14 | Revisión crítica | Especificación | `plan` | sí | Primero corre chequeos deterministas; luego un subagente con contexto limpio busca huecos. Si hay huecos graves, regresa a la etapa donde falta información. | Markdown, Table |

Cada vez que entras en un nodo aparece un bloque StageHeader («Etapa 5/14 · Hipótesis») con una barra de progreso. Los modelos de cada rol se configuran en [configuracion.md](configuracion.md#modelos-por-rol). Las tools de búsqueda y lectura web del servidor de Anthropic se añaden a las llamadas de los roles `explore` y `plan`, salvo en Spec, Plan y Revisión crítica.

**Agentes**:

- Observación, Datos e Investigación usan un agente de **solo lectura**. Tiene `list_files`, `find_file`, `tree`, `grep`, `read_file`, `run_command` (con la política de solo lectura, así que solo pasan los `git` de lectura) y las tools MCP declaradas con `effect = "read"`. Nunca pide aprobación y tiene un máximo de 30 llamadas.
- Experimentos y Prototipo usan el agente del chat, con tus permisos y aprobaciones.

### Compuertas y respuestas

Al terminar un nodo con compuerta, el flujo pregunta:

> **Hipótesis** terminó. Responde «sí» para seguir con Experimentos (6/14), escribe correcciones para rehacer esta etapa o usa `/back <nodo>` para regresar.

| Respuesta | Efecto |
| --- | --- |
| Aprobación: `sí`/`si`, `s`, `ok`, `okay`, `vale`, `dale`, `listo`, `sigue`, `seguir`, `continua`/`continuar`, `adelante`, `de acuerdo`, `aprobado`, `y`, `yes`, `go`, `lgtm`, o **una línea vacía** | Sigue al nodo siguiente. Se ignoran mayúsculas, acentos y un `.`, `!` o `,` al final. |
| Cualquier otro texto, por ejemplo «agrega una hipótesis sobre la caché» | Rehace la etapa con ese texto como corrección y vuelve a preguntar. «sí, pero agrega H3» cuenta como corrección, no como aprobación. |
| `/stage`, `/back`, `/skip`, `/abort` | Ver la tabla siguiente. |

Si Intake deja preguntas abiertas, contéstalas escribiéndolas en la compuerta: tu texto se suma a las respuestas y Intake se rehace.

También hay compuertas de **pregunta**: cualquier texto que escribas, incluido «sí», se toma como respuesta y el nodo se vuelve a ejecutar con ella. Aparecen en tres casos:

- **Definición**, cuando tras un reintento la definición sigue sin ser medible. Te pide qué se observa y cuánto (por ejemplo, «a 500 requests por segundo el tiempo de respuesta sube de 10 ms a 500 ms») o una condición comprobable («al ejecutar X, el test Y falla»).
- **Prototipo**, antes de crear el worktree.
- Cuando una etapa pidió **regresar más de 3 veces** a otra. El flujo te pregunta qué falta.

### Comandos dentro del flujo

| Comando | Qué hace |
| --- | --- |
| `/stage` | Muestra la posición, por ejemplo «Etapa 5/14: Hipótesis (Descubrimiento) · esperando tu respuesta». |
| `/back <nodo>` | Vuelve a ese nodo y lo corre de inmediato. `<nodo>` puede ser el número (1-14), el nombre en español (sin importar acentos ni mayúsculas: `hipótesis`, `observacion`, `Definición y restricciones`), el id (`intake`, `observation`, `data`, `analysis`, `hypotheses`, `experiments`, `definition`, `research`, `brainstorm`, `modeling`, `prototype`, `spec`, `plan`, `critical_review`) o un prefijo único de al menos 3 letras del nombre en español (`hip`, `defin`, `lluvia`). Solo admite nodos anteriores al actual o el actual, salvo con el flujo terminado, que admite cualquiera. |
| `/skip` | Salta el nodo actual. Solo vale para **Datos (3)** y **Prototipo (11)**. En la práctica, Datos solo se puede saltar después de que falle, porque no tiene compuerta. |
| `/abort` | Aborta el flujo. El estado y el diario quedan guardados y el chat vuelve a la normalidad. |

Un comando `/` desconocido (por ejemplo `/model`) **no** da error: se toma como respuesta a la compuerta. Los mensajes de error de estos comandos están en inglés, por ejemplo `usage: /back <stage>, by number (1-14) or name, e.g. /back 5 or /back hipótesis`.

**Si un nodo falla**, el timeline muestra un bloque `Error` «<Etapa>: <error>» y el flujo queda en su último checkpoint. El texto libre ya no se acepta (devuelve «the research flow is not waiting for input»). Para reintentar, usa `/back <etapa actual>` o `:research resume`.

### Regresos automáticos

Un nodo regresa a una etapa anterior cuando le falta información, en vez de inventarla. Cada regreso aparece como aviso («Spec regresa a Modelado: …») y queda en el diario. Los principales son:

| Nodo | Regresa a | Cuándo |
| --- | --- | --- |
| Observación, Datos | Intake / Observación | No hay brief o no hay reporte de observación. |
| Análisis, Hipótesis, Definición | Datos u Observación | No hay evidencia. |
| Experimentos, Definición | Hipótesis | No hay hipótesis. |
| Investigación, Lluvia de ideas | Definición | No hay definición del problema. |
| Modelado | Definición / Lluvia de ideas | No hay restricciones, no hay opciones, o todas las opciones violan alguna restricción. |
| Prototipo | Modelado | No hay decisión, o el spike resultó inviable. |
| Spec | Definición / Observación / Modelado | Falta la definición, la evidencia o la decisión. |
| Plan | Spec | No hay borrador de Spec. |
| Revisión crítica | la etapa que falta | Hay citas `[E…]` desconocidas, secciones vacías, antecedentes sin cita o archivos citados que no existen (Observación); una restricción sin prueba (Definición); un plan inválido, sin `hypothesis_tests` o sin prueba para una restricción (Plan); o huecos graves que encontró el subagente. |

Tras 3 regresos desde la misma etapa, el cuarto se convierte en una pregunta para ti.

### Prototipo (spike)

1. Si apruebas el spike, crest corre `git worktree add -b crest/spike-<id> .crest/spikes/<id> HEAD` dentro del workspace, con la tool `run_command` y sus aprobaciones. Antes te muestra los comandos exactos, incluida la limpieza, y pregunta «¿Lo creo? Responde «sí» para seguir, cualquier otra cosa para seguir sin prototipo, o /skip.».
2. El agente del chat escribe el spike en el worktree y emite un veredicto (factible o no), que se registra como evidencia y se muestra en un bloque «Resultado del spike».
3. La limpieza (`git worktree remove --force` y `git branch -D`) siempre corre. Si el spike no es factible, el flujo regresa a Modelado.

Requiere `git` y un repositorio. Si `git worktree add` falla, el nodo falla, y `/skip` deja seguir sin prototipo. Mientras el worktree existe, `.crest/` aparece como no rastreado en `git status`, salvo que lo ignores.

### Lo que escribe: `.crest/specs/NNNN-slug/`

Se crea una carpeta por cada definición del problema. Los archivos se escriben sin pedir aprobación.

```text
.crest/specs/0007-cache-de-sesiones/
  spec.md        Spec para personas (10 secciones)
  plan.md        plan legible, generado desde plan.json (no editar a mano)
  plan.json      plan que consume el flujo de implementación
  journal.md     diario de investigación (lo reescribe el nodo Plan)
  sources.json   evidencia citada: id, afirmación, fuente (archivo, commit, URL o comando) y fecha
  review.md      resultado de la revisión crítica
  evidence/
    chart-1-commits-por-semana.csv   datos de cada gráfica (label,<series...>)
```

- **`NNNN`** es el mayor prefijo de 4 dígitos que ya existe en `.crest/specs/`, más 1. Si `.crest/` está en `.gitignore`, crest no ve las carpetas existentes y la numeración vuelve a 0001.
- **`slug`** sale del título: minúsculas, sin acentos, con `-` en lugar de cualquier otro carácter y un máximo de 48 caracteres. Si queda vacío, es `spec`.
- En `evidence/` solo se escriben los CSV de las gráficas. No se guardan imágenes ni salidas de comandos.
- El diario se reescribe por última vez en el nodo Plan, así que las entradas de la Revisión crítica no llegan a `journal.md`.

Las secciones de `spec.md` son:

1. Problema
2. Antecedentes (con citas)
3. Hipótesis
4. Propuesta de solución (con la caja negra en Mermaid)
5. Especificación funcional (con `### Secuencia` y `### Estados`)
6. Especificación técnica
7. Restricciones (tabla Id, Tipo, Restricción, Prueba)
8. Protocolo de pruebas
9. Fuera de alcance y riesgos
10. Fuentes (`[E1]`, ..., y enlaces a los CSV)

#### `plan.json`

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

| Campo | Obligatorio | Valores |
| --- | --- | --- |
| `spec` | sí | Nombre de la carpeta (el nodo Plan lo fija). |
| `tasks[].id`, `title`, `description` | sí | Strings. Los ids deben ser únicos. |
| `tasks[].depends_on` | sí | Ids de otras tareas, sin ciclos. |
| `tasks[].files` | sí | Al menos una ruta. Un directorio que termina en `/` cubre todo lo que tiene debajo. |
| `tasks[].acceptance` | sí | `{ kind: "test" \| "command", ref: "<línea de comando>" }`. |
| `tasks[].role` | no (`main`) | `main`, `plan`, `explore`, `compact`, `title`, `component` o `deep`. |
| `tasks[].risk` | sí | `low`, `medium` o `high`. |
| `hypothesis_tests` | no (`[]`) | `{ kind, ref, expect }`. `expect` es texto, por ejemplo `"p95 < 20ms"`. |
| `constraint_tests` | no (`[]`) | `{ constraint: "C<n>", kind, ref }`. |

No se admiten campos desconocidos. Un plan sin tareas, con ids repetidos, con dependencias desconocidas, con tareas sin archivos o con ciclos se rechaza (por ejemplo, `tasks form a dependency cycle: T1 -> T2 -> T1`).

## Flujo de implementación

### Iniciar

| Dónde | Comando |
| --- | --- |
| Ventana | `:implement <destino>` |
| Headless | `crest --headless --implement <destino>` |

`<destino>` puede ser:

| Forma | Ejemplo | Plan que se usa |
| --- | --- | --- |
| Un nombre sin `/` | `0007-cache-de-sesiones` | `.crest/specs/0007-cache-de-sesiones/plan.json` |
| Una ruta con `/` | `.crest/specs/0007-cache-de-sesiones/` | `<ruta>/plan.json` |
| Una ruta a un `.json` | `otro/plan.json` | ese archivo |
| `.` | `.` | `plan.json` en la raíz |

No hay búsqueda por prefijo: `0007` solo funciona si existe una carpeta llamada exactamente `0007`. Las rutas absolutas deben estar dentro del workspace.

Abrir de nuevo el mismo plan **reanuda** el flujo si no había terminado, con el aviso «flujo de implementación de … reanudado». Un flujo terminado o abortado empieza de cero. Al reanudar, la pregunta pendiente **no** se vuelve a mostrar: responde directamente, o escribe `/stage` para verla. En la ventana solo puede haber un flujo de implementación activo a la vez.

Requisitos:

- `git` y un repositorio. La rama se crea desde el `HEAD` actual. crest no comprueba que el árbol de trabajo esté limpio, así que los cambios sin commitear pasan a la rama.
- La base de datos de crest.
- La API key, porque cada tarea la implementa el agente.

### Pasos

| # | Paso | Qué hace |
| --- | --- | --- |
| 1 | Cargar plan | Valida `plan.json` y ordena las tareas por dependencias (ante un empate, por id natural: T2 antes que T10). Muestra una tabla y el grafo en Mermaid. **Compuerta**, y después crea la rama `spec/<spec>` con `git switch -c` (si ya existe, se cambia a ella). Si el plan no es válido, vuelve al Spec sin preguntar. |
| 2 | Implementar tarea | Un turno del agente con el **rol de la tarea**. El prompt lista los archivos permitidos y los criterios de aceptación. El agente termina con `STATUS: done` o `STATUS: blocked: <motivo>`, y las líneas `IDEA: …` van al reporte. |
| 3 | Pruebas y corrección | Corre los criterios de aceptación. Si alguno falla, pide al agente que lo corrija, **hasta 3 veces**. Si siguen fallando, **compuerta**. |
| 4 | Commit | Agrega **solo** los archivos de la tarea que cambiaron (más los aprobados fuera de alcance) y hace commit. Si no cambió nada, no hay commit y queda un desvío en el diario. |
| 5 | Verificar hipótesis | Cuando terminan todas las tareas, corre `hypothesis_tests` y `constraint_tests` una vez y muestra una tabla «Prueba / Comando / Esperado / Obtenido / Cumple» y, si hay mediciones, una gráfica «Esperado contra obtenido». |
| 6 | Reporte y futuro | Escribe `report.md` en la carpeta del Spec. **Compuerta final.** |

Los pasos 2 a 4 se repiten para cada tarea. Cada paso tiene un límite de 1 hora.

**Mensaje de commit**:

```text
<tipo>(<scope>): <título de la tarea>

Task: <id>
```

- El `tipo` sale de la primera palabra del título:
  - `fix` para correg-, corrig-, arregl-, fix, repar-;
  - `refactor` para refactor, mover, move, renombr-, rename, extra-;
  - `test` para test, prueb-, prob-;
  - `docs` para document-, doc;
  - `feat` en cualquier otro caso.
- El `scope` es el nombre del paquete del `Cargo.toml` más cercano al primer archivo de la tarea. Si no hay ninguno, es el directorio de primer nivel, y si el archivo está en la raíz no lleva scope.
- El commit usa tu configuración de git, incluidos sus hooks.

**Criterios y pruebas**:

- Cada `ref` se ejecuta en la raíz del workspace, directamente, sin pasar por aprobaciones.
- Si la línea contiene `| & ; < > $ \` * ? ( ) ~` fuera de comillas, se ejecuta con `sh -c`.
- Pasa si sale con código 0. El límite es de 30 minutos por prueba.
- `kind` (`test` o `command`) no cambia cómo se ejecuta; solo se usa como etiqueta.
- En `hypothesis_tests`, `expect` puede ser una comparación con número y unidad (`p95 < 20ms`, `throughput >= 500 req/s`). En ese caso la hipótesis se cumple si el comando sale con 0 **y** en su salida aparece un valor que cumple la comparación. crest busca el número que sigue a la última mención de la métrica, convirtiendo entre `ns`, `us`, `ms`, `s` y `min`. Si `expect` es texto libre, basta con que el comando salga con 0.

### Compuertas y respuestas

Las respuestas se interpretan sin importar mayúsculas ni acentos:

| Tipo | Palabras |
| --- | --- |
| continuar | Las aprobaciones del flujo investigativo (`sí`, `ok`, `continuar`, ...) y **también una línea vacía**. |
| reintentar | `reintentar`, `reintenta`, `otra vez`, `retry` |
| saltar | `saltar`, `salta`, `saltar tarea`, `saltar la tarea`, `skip` |
| abortar | `abortar`, `aborta`, `abort`, `cancelar`, `detener` |
| volver al spec | `volver al spec`, `volver a spec`, `volver al nodo spec`, `spec`, `back spec`, `back` |
| otro texto | Instrucciones libres para el agente (donde se aceptan). |

Además, en cualquier compuerta funcionan `/stage` (posición, por ejemplo «Paso 3/6: Pruebas y corrección · tarea T1 (1/3) · esperando tu respuesta») y `/abort`.

| Compuerta | Cuándo | Respuestas |
| --- | --- | --- |
| Plan | Antes de crear la rama | **continuar**: crea la rama y empieza. **abortar**. **volver al spec**. Cualquier otra cosa repite la pregunta. |
| Rama | `git switch -c` falló | **continuar** reintenta (`reintentar` no lo hace). **abortar**. |
| Tarea bloqueada | El agente respondió `STATUS: blocked` | **reintentar**: otro intento. **otro texto**: otro intento con tus instrucciones. **continuar**: la da por implementada y pasa a pruebas. **saltar**: sin commit. **abortar**. **volver al spec**. |
| Pruebas fallando | Se agotaron los 3 intentos de corrección | **reintentar** u **otro texto**: 3 intentos más (con tus instrucciones, si las diste). **continuar**: hace commit con los criterios fallando. **saltar**. **abortar**. **volver al spec**. |
| Commit fallido | `git commit` falló | **reintentar** u **otro texto**: reintenta el commit. **continuar** o **saltar**: sigue sin commit. **abortar**. **volver al spec**. |
| Reporte | Después de escribir `report.md` | **continuar**: cierra el flujo. **otro texto**: lo añade al reporte bajo «Notas del usuario» y vuelve a preguntar. **abortar**. **volver al spec**. |

> Cuidado: una línea vacía (solo `Enter`) cuenta como **continuar** en todas las compuertas. En «Pruebas fallando» eso hace commit con los criterios fallando, y en «Reporte» cierra el flujo.

En headless, cada compuerta muestra `› ` y lee una línea de stdin. Las aprobaciones de archivos fuera de alcance usan el mismo stdin (`[y]es / [a]lways / [n]o`). `--yes` aprueba esas acciones, pero **no** responde las compuertas. Si stdin se cierra, el flujo queda en esa compuerta («el mismo comando lo reanuda») y crest sale con 1.

### Restricción de archivos

Cada tarea solo puede tocar sus `files` (un directorio con `/` cubre su contenido) con `edit_file` y `write_file`:

- Para escribir **fuera** de esos archivos, siempre se pide aprobación, en cualquier modo de permisos, con el texto «La tarea X quiere cambiar P, que no está entre sus archivos (…)».
- Si apruebas, la ruta queda permitida para el resto de la tarea, se incluye en su commit y se anota como desvío en el diario.
- Si niegas, el agente recibe el rechazo y también queda anotado.

`run_command` **no** está restringido por tarea. Un comando puede modificar cualquier archivo, sujeto solo a la política de permisos normal, pero el commit solo incluye los archivos de la tarea.

### `report.md`

Se escribe junto a `plan.json` y se muestra en el timeline. Contiene:

- **Encabezado**: plan, rama, tareas hechas (X de N) y pruebas que se cumplen.
- **Tareas**: tabla con estado y commit de cada una.
- **Logros**, **Fallos** y **Desvíos del plan**.
- **Verificación**: la tabla de hipótesis y restricciones.
- **Ideas para siguientes Specs**: las líneas `IDEA:`, las tareas que quedaron sin hacer y las pruebas que no se cumplen.
- **Notas del usuario**: lo que escribiste en la compuerta final.

### Volver al Spec

Si el plan resulta inviable, porque respondiste «volver al spec» o porque `plan.json` no es válido, el flujo **no improvisa**:

1. Anota el desvío y se detiene con el mensaje «el plan resultó inviable (…)».
2. **No** cambia de flujo automáticamente. Para corregir el plan:
   1. `:research resume` reabre el último flujo investigativo del workspace (sigue activo aunque haya terminado).
   2. `/back spec` vuelve al nodo Spec, que regenera el Spec y, después, el plan.
   3. `:implement <spec>` vuelve a lanzar la implementación.

En headless, salir por «volver al spec» o por «abortar» termina con código 1.
