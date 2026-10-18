//! Step 6, Reporte y futuro: renders `report.md` from the journal and the
//! results: achievements, failures, deviations from the plan and ideas for
//! later Specs.

use std::fmt::Write as _;

use super::verify::check_label;
use crate::flows::implement::state::{
    EntryKind, ImplementState, JournalEntry, Step, TaskRecord, TaskStatus,
};

/// Name of the report in the Spec folder.
pub const REPORT_FILE: &str = "report.md";

fn status_es(status: TaskStatus) -> &'static str {
    match status {
        TaskStatus::Pending => "pendiente",
        TaskStatus::Implemented => "implementada sin probar",
        TaskStatus::Passed => "probada, sin commit",
        TaskStatus::Failed => "con criterios fallando",
        TaskStatus::Skipped => "saltada",
        TaskStatus::Committed => "hecha",
    }
}

/// A table cell: one line, pipes escaped.
fn cell(text: &str) -> String {
    text.replace('\n', " ").replace('|', "\\|")
}

fn entry_line(entry: &JournalEntry) -> String {
    let text = entry.text.trim();
    let first = text.lines().next().unwrap_or_default();
    let rest: Vec<_> = text.lines().skip(1).collect();
    let mut line = match &entry.task {
        Some(task) if !first.contains(task.as_str()) => format!("- **{task}**: {first}"),
        _ => format!("- {first}"),
    };
    if !rest.is_empty() {
        line.push('\n');
        for l in rest {
            let _ = write!(line, "\n  {l}");
        }
    }
    line
}

fn section(out: &mut String, title: &str, lines: &[String], empty: &str) {
    let _ = write!(out, "\n## {title}\n\n");
    if lines.is_empty() {
        let _ = writeln!(out, "{empty}");
    } else {
        for line in lines {
            let _ = writeln!(out, "{line}");
        }
    }
}

fn lines_of(state: &ImplementState, kind: EntryKind) -> Vec<String> {
    state.entries(kind).map(entry_line).collect()
}

/// Ideas from the journal plus the follow-ups the results call for: the
/// tasks left undone and the checks that do not hold.
fn ideas(state: &ImplementState) -> Vec<String> {
    let mut ideas = lines_of(state, EntryKind::Idea);
    for id in &state.order {
        let record = state.record(id).cloned().unwrap_or_default();
        if matches!(
            record.status,
            TaskStatus::Skipped | TaskStatus::Failed | TaskStatus::Pending
        ) {
            let title = state.task(id).map(|t| t.title.as_str()).unwrap_or_default();
            let why = record
                .last_failure
                .as_deref()
                .and_then(|f| f.lines().next())
                .map(|f| format!(" ({f})"))
                .unwrap_or_default();
            ideas.push(format!("- Retomar **{id}** «{title}»{why}."));
        }
    }
    for check in state.checks.iter().filter(|c| !c.passed) {
        ideas.push(format!(
            "- Investigar por qué {} no se cumple: `{}` esperaba {} y obtuvo {}.",
            check_label(check),
            check.command,
            check.expected,
            check.obtained
        ));
    }
    ideas
}

/// The text of `report.md`.
#[must_use]
pub fn render_report(state: &ImplementState) -> String {
    let spec = state.plan.as_ref().map_or("?", |p| p.spec.as_str());
    let mut out = format!("# Reporte de implementación: {spec}\n\n");
    let _ = writeln!(out, "- Plan: `{}`", state.plan_path);
    if let Some(branch) = &state.branch {
        let _ = writeln!(out, "- Rama: `{branch}`");
    }
    let done = state
        .order
        .iter()
        .filter(|id| {
            state
                .record(id)
                .is_some_and(|r| r.status == TaskStatus::Committed)
        })
        .count();
    let _ = writeln!(out, "- Tareas hechas: {done} de {}", state.order.len());
    let passed = state.checks.iter().filter(|c| c.passed).count();
    let _ = writeln!(
        out,
        "- Pruebas de hipótesis y restricciones que se cumplen: {passed} de {}",
        state.checks.len()
    );

    out.push_str(
        "\n## Tareas\n\n| Tarea | Título | Estado | Commit |\n| --- | --- | --- | --- |\n",
    );
    for id in &state.order {
        let record: TaskRecord = state.record(id).cloned().unwrap_or_default();
        let title = state.task(id).map(|t| t.title.as_str()).unwrap_or_default();
        let _ = writeln!(
            out,
            "| {id} | {} | {} | {} |",
            cell(title),
            status_es(record.status),
            record
                .commit
                .as_deref()
                .map_or("—".to_owned(), |c| format!("`{c}`"))
        );
    }

    section(
        &mut out,
        "Logros",
        &lines_of(state, EntryKind::Achievement),
        "Ninguno.",
    );
    section(
        &mut out,
        "Fallos",
        &lines_of(state, EntryKind::Failure),
        "Ninguno.",
    );
    section(
        &mut out,
        "Desvíos del plan",
        &lines_of(state, EntryKind::Deviation),
        "Ninguno: se tocaron solo los archivos de cada tarea y se siguió el orden del plan.",
    );

    out.push_str("\n## Verificación\n\n");
    if state.checks.is_empty() {
        out.push_str("El plan no tiene pruebas de hipótesis ni de restricciones.\n");
    } else {
        out.push_str("| Prueba | Comando | Esperado | Obtenido | Cumple |\n| --- | --- | --- | --- | --- |\n");
        for c in &state.checks {
            let _ = writeln!(
                out,
                "| {} | `{}` | {} | {} | {} |",
                check_label(c),
                cell(&c.command),
                cell(&c.expected),
                cell(&c.obtained),
                if c.passed { "sí" } else { "no" }
            );
        }
    }

    section(
        &mut out,
        "Ideas para siguientes Specs",
        &ideas(state),
        "Ninguna.",
    );

    let notes: Vec<_> = state
        .journal
        .iter()
        .filter(|e| e.step == Step::Report && e.kind == EntryKind::User)
        .map(|e| {
            let text = e.text.trim_start_matches("Respuesta del usuario:").trim();
            format!("- {text}")
        })
        .collect();
    if !notes.is_empty() {
        section(&mut out, "Notas del usuario", &notes, "");
    }
    out
}

#[cfg(test)]
mod tests {
    use domain::plan::TaskId;
    use domain::research::TestKind;

    use super::*;
    use crate::flows::implement::nodes::tests::loaded_state;
    use crate::flows::implement::state::{CheckResult, CheckSubject};

    fn finished_state() -> ImplementState {
        let mut state = loaded_state(&[("T1", &[]), ("T2", &["T1"])]);
        let (t1, t2) = (TaskId::new("T1"), TaskId::new("T2"));
        let r1 = state.record_mut(&t1);
        r1.status = TaskStatus::Committed;
        r1.commit = Some("abc1234".into());
        let r2 = state.record_mut(&t2);
        r2.status = TaskStatus::Skipped;
        r2.last_failure = Some("`cargo test t2` → falla (exit 101)".into());
        let log = [
            (
                Step::Commit,
                Some(&t1),
                EntryKind::Achievement,
                "Tarea T1 en el commit abc1234",
            ),
            (
                Step::Implement,
                Some(&t1),
                EntryKind::Deviation,
                "write_file cambió Cargo.toml fuera de los archivos de la tarea",
            ),
            (
                Step::Test,
                Some(&t2),
                EntryKind::Failure,
                "los criterios siguen fallando",
            ),
            (
                Step::Implement,
                Some(&t1),
                EntryKind::Idea,
                "medir el arranque en frío",
            ),
        ];
        for (step, task, kind, text) in log {
            state.log(1, step, task.cloned(), kind, text);
        }
        state.checks = vec![CheckResult {
            subject: CheckSubject::Hypothesis,
            kind: TestKind::Command,
            command: "make bench".into(),
            expected: "p95 < 20ms".into(),
            obtained: "p95 31ms".into(),
            measured: Some(31.0),
            threshold: Some(20.0),
            unit: Some("ms".into()),
            passed: false,
        }];
        state.current = 2;
        state
    }

    #[test]
    fn lists_achievements_failures_deviations_and_ideas() {
        let report = render_report(&finished_state());
        for heading in [
            "## Logros",
            "## Fallos",
            "## Desvíos del plan",
            "## Verificación",
            "## Ideas para siguientes Specs",
        ] {
            assert!(report.contains(heading), "{heading}\n{report}");
        }
        assert!(report.contains("- Tareas hechas: 1 de 2"), "{report}");
        assert!(
            report.contains("| T1 | Hacer T1 | hecha | `abc1234` |"),
            "{report}"
        );
        assert!(
            report.contains("- Tarea T1 en el commit abc1234"),
            "{report}"
        );
        assert!(
            report.contains("- **T2**: los criterios siguen fallando"),
            "{report}"
        );
        assert!(
            report.contains("Cargo.toml fuera de los archivos"),
            "{report}"
        );
        assert!(report.contains("| Hipótesis | `make bench` | p95 < 20ms | p95 31ms | no |"));
        assert!(
            report.contains("- **T1**: medir el arranque en frío"),
            "{report}"
        );
        assert!(
            report.contains("- Retomar **T2** «Hacer T2» (`cargo test t2`"),
            "{report}"
        );
        assert!(
            report.contains("Investigar por qué Hipótesis no se cumple"),
            "{report}"
        );
    }

    #[test]
    fn empty_sections_say_so() {
        let report = render_report(&loaded_state(&[("T1", &[])]));
        assert!(
            report.contains("## Desvíos del plan\n\nNinguno"),
            "{report}"
        );
        assert!(report.contains("ni de restricciones"), "{report}");
    }
}
