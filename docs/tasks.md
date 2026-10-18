# Estado de tareas

Estados: pendiente · en curso · en revisión · integrada. Lo mantiene el orquestador.

| ID | Estado | Depende de | Alcance |
| --- | --- | --- | --- |
| T0.1 ★ | integrada | — | raíz, `crates/*/Cargo.toml` |
| T0.2 | integrada | T0.1 | `Makefile` |
| T0.3 | integrada | — | `scripts/install-deps.sh` |
| T0.4 | integrada | T0.1 | `scripts/check-arch.sh`, `tests/arch/` |
| T0.5 | integrada | T0.2, T0.3, T0.4 | `.github/workflows/` |
| T1.1 ★ | integrada | T0.1 | `domain/src/conversation.rs` |
| T1.2 | integrada | T0.1 | `domain/src/block.rs` |
| T1.3 | integrada | T0.1 | `domain/src/model.rs` |
| T1.4 | integrada | T0.1 | `domain/src/workspace.rs` |
| T1.5 | integrada | T0.1 | `domain/src/permission.rs` |
| T1.6a ★ | integrada | T1.1, T1.3 | `application/src/ports/llm.rs`, `mcp.rs` |
| T1.6b | integrada | T1.4 | `application/src/ports/{search,fs,shell}.rs` |
| T1.6c | integrada | T1.1, T1.2, T1.5 | `application/src/ports/{approval,events,store,plugin,telemetry}.rs` |
| T1.7a ★ | integrada | T1.6a | `application/src/testing/llm.rs` |
| T1.7b | integrada | T1.6b | `application/src/testing/fs.rs` |
| T1.7c | integrada | T1.6c | `application/src/testing/system.rs` |
| T2.A1 ★ | integrada | T1.7a | `application/src/tools/registry.rs` |
| T2.A2 ★ | integrada | T2.A1, T1.7c | `application/src/usecases/run_turn.rs` |
| T2.A3 ★ | integrada | T2.A2, T1.5 | mismo archivo + `approval.rs` |
| T2.A4 | integrada | T2.A2 | `run_turn.rs` |
| T2.A5 | integrada | T2.A2 | `application/src/tools/budget.rs` |
| T2.B1 ★ | integrada | T1.6a | `adapters/anthropic/src/wire.rs` |
| T2.B2 ★ | integrada | T2.B1 | `stream.rs` |
| T2.B3 | integrada | T2.B1 | `client.rs` |
| T2.B4 | integrada | T2.B3, T1.3 | `retry.rs` |
| T2.B5 | integrada | T2.B3 | `catalog.rs` |
| T2.B6 | integrada | T2.B2 | `wire.rs`, `request.rs` |
| T2.B7 | integrada | T2.B4 | `tests/live.rs` |
| T2.C1 | integrada | T1.6b | `adapters/search/src/process.rs` |
| T2.C2 | integrada | T2.C1 | `detect.rs` |
| T2.C3 | integrada | T2.C1 | `files.rs` |
| T2.C4 | integrada | T2.C1 | `grep.rs` |
| T2.C5 | integrada | T2.C2, T2.C3 | `find.rs` |
| T2.D1 | integrada | T1.6b, T1.4 | `adapters/fs/src/guard.rs` |
| T2.D2 | integrada | T2.D1 | `read.rs` |
| T2.D3 | integrada | T2.D1 | `write.rs` |
| T2.D4 | integrada | T1.6b | `adapters/shell/` |
| T2.E1 | integrada | T1.6c | `adapters/store/migrations/` |
| T2.E2 | integrada | T2.E1 | `adapters/store/src/session.rs` |
| T2.F1 | integrada | T1.6c | `adapters/lua/src/host.rs` |
| T2.F2 | integrada | T2.F1 | `sandbox.rs` |
| T2.F3 | integrada | T2.F1, T1.3 | `config.rs` |
| T2.F4 | integrada | T0.1 | `application/src/keymap.rs` |
| T2.G1 | integrada | T1.6c | `adapters/telemetry/` |
| T2.G2 | integrada | T2.G1 | mismo crate |
| T2.H1 ★ | integrada | T0.1 | `ui/src/app.rs` |
| T2.H2 ★ | integrada | T2.H1, T1.2, T1.7c | `ui/src/timeline/` |
| T2.H3 | integrada | T2.H1, T2.F4 | `ui/src/input.rs` |
| T2.H4 | integrada | T2.H1 | `ui/src/status.rs` |
| T2.I1 | integrada | T1.1 | `domain/src/research.rs` |
| T2.I2 | integrada | T2.I1 | `domain/src/plan.rs` |
| T3.1 ★ | integrada | T2.A3, T2.B4, T2.C5, T2.D3, T2.D4 | `crates/bin/` |
| T3.2 | integrada | T2.A1, T2.C5, T2.C4, T2.D3 | `application/src/tools/files.rs` |
| T3.3 ★ | integrada | T3.1, T2.H2, T2.H3 | `ui/src/session.rs` |
| T3.4 | integrada | T2.H2, T2.A3 | `ui/src/blocks/diff.rs`, `ui/src/approval.rs` |
| T3.5 | integrada | T2.H1, T2.C3 | `ui/src/tree.rs` |
| T3.6 | integrada | T2.H1, T2.C5, T2.C4 | `ui/src/picker.rs` |
| T3.7 | integrada | T2.A1, T1.2 | `application/src/tools/components.rs` |
| T3.8 | integrada | T2.H2 | `ui/src/blocks/chart.rs` |
| T3.9 | integrada | T2.H2 | `ui/src/blocks/table.rs` |
| T3.10 | integrada | T2.H2 | `ui/src/blocks/mermaid.rs` |
| T3.11 | integrada | T2.H2 | `ui/src/blocks/markdown.rs` |
| T3.12 | integrada | T2.H2 | `ui/src/blocks/thinking.rs` |
| T4.1 | integrada | T2.F3, T3.3 | `adapters/lua/src/api/keymap.rs` |
| T4.2 | integrada | T2.F3, T2.A1 | `api/tools.rs` |
| T4.3 | integrada | T2.F3, T2.A3 | `api/hooks.rs` |
| T4.4 | integrada | T2.F1, T2.H2 | `api/ui.rs`, `ui/src/blocks/custom.rs` |
| T4.5 | integrada | T2.F3, T3.3 | `crates/bin/` |
| T4.6 | integrada | T2.E2, T3.3 | `application/src/usecases/resume.rs` |
| T4.7 | integrada | T2.A2, T1.3 | `application/src/usecases/compact.rs` |
| T4.8 | integrada | T2.B2, T2.H4 | `application/src/cost.rs`, `ui/src/status.rs` |
| T5.1 ★ | integrada | T2.E1 | `adapters/store/src/graphflow.rs` |
| T5.2 | integrada | T2.B2 | `adapters/anthropic/src/server_tools.rs` |
| T5.3 ★ | integrada | T2.I1, T3.3, T5.1 | `application/src/flows/research/mod.rs` |
| T5.4 | integrada | T2.H2, T1.2 | `ui/src/blocks/` |
| T5.5a–f | integrada | T5.3 (Datos también T5.2) | `flows/research/nodes/<nodo>.rs` |
| T5.6 | integrada | T5.3 | `nodes/define.rs` |
| T5.7a–d | en curso | T5.3 (Investigación también T5.2) | `nodes/<nodo>.rs` |
| T5.8 ★ | integrada | T5.3, T2.I2 | `nodes/spec.rs`, `nodes/plan.rs` |
| T5.9 ★ | en curso | T5.8 | `nodes/review.rs` |
| T5.10 ★ | pendiente | T5.4–T5.9 | `flows/research/graph.rs` |
| T6.1 ★ | en curso | T2.I2, T5.3 | `application/src/flows/implement/load.rs` |
| T6.2 ★ | en curso | T6.1 | `nodes/implement.rs` |
| T6.3 ★ | en curso | T6.2 | `nodes/test.rs` |
| T6.4 | en curso | T6.2 | `nodes/commit.rs` |
| T6.5 | en curso | T6.1, T3.8 | `nodes/verify.rs` |
| T6.6 | en curso | T6.5 | `nodes/report.rs` |
| T6.7 ★ | pendiente | T6.3–T6.6, T5.10 | `flows/implement/graph.rs` |
| T7.1 | integrada | T1.6a, T2.A1 | `adapters/mcp/` |
| T7.2 | integrada | T7.1, T2.F3 | `adapters/lua/src/api/mcp.rs` |
| T7.3 | integrada | T2.A2 | `application/src/tools/task.rs` |
| T7.4 | integrada | T0.5, T3.3 | `.github/workflows/release.yml` |

Nota del orquestador: los carriles de la ola 2 cuyas tareas comparten crate y archivos (A1–A5, C2–C5, E1–E2…) se ejecutan en serie dentro de una sola rama por carril, con un commit `Task: <id>` por tarea, para evitar conflictos en `lib.rs`/`Cargo.toml` del crate.
