# Documentación de crest

Para la presentación general del proyecto, la instalación y el inicio rápido, ver el [README principal](../README.md).

| Guía | Para quién | Contenido |
| --- | --- | --- |
| [uso.md](uso.md) | Quien usa crest | La ventana, los modos, los atajos, los comandos `:`, las aprobaciones, los bloques, la CLI, el modo headless, las sesiones y los servidores MCP. |
| [configuracion.md](configuracion.md) | Quien configura crest | Rutas y precedencia de `init.lua`, la API key, `app.opt`, los modelos por rol, los permisos, las variables de entorno, Langfuse y un ejemplo completo. |
| [plugins.md](plugins.md) | Quien escribe plugins | El sandbox de Lua, la tabla `app` (keymaps, comandos, tools, hooks, `app.ui`, MCP), los diagnósticos y ejemplos paso a paso. |
| [flujos.md](flujos.md) | Quien usa los flujos | El flujo investigativo (14 nodos, compuertas, `.crest/specs/`) y el flujo de implementación (pasos, compuertas, commits, `report.md`). |
| [arquitectura.md](arquitectura.md) | Quien lee el código | Las capas hexagonales, los puertos y adaptadores, el loop de un turno, los eventos, los hilos, la persistencia y las reglas de dependencia. |
| [desarrollo.md](desarrollo.md) | Quien contribuye | Los tests y los fakes, cómo añadir tools, bloques, adaptadores o nodos, las convenciones de commits y la CI. |
