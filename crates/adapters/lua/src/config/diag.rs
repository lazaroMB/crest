//! Turning Lua errors into [`PluginDiagnostic`]s with file and line.

use application::ports::plugin::PluginDiagnostic;

use crate::sandbox::Failure;

/// Splits `chunk:LINE: message` into the line and the message. Lua prefixes
/// errors with the chunk name, which may be a long path shortened with
/// `...`, so the first `:<digits>:` marks the location.
fn locate(text: &str) -> (Option<u32>, &str) {
    let text = text.split("\nstack traceback:").next().unwrap_or(text);
    let bytes = text.as_bytes();
    for (i, _) in text.match_indices(':') {
        let digits = bytes[i + 1..]
            .iter()
            .take_while(|b| b.is_ascii_digit())
            .count();
        let end = i + 1 + digits;
        if digits > 0 && bytes.get(end) == Some(&b':') {
            let line = text[i + 1..end].parse().ok();
            return (line, text[end + 1..].trim());
        }
    }
    (None, text.trim())
}

/// Line of the first Lua frame in a traceback.
fn traceback_line(traceback: &str) -> Option<u32> {
    traceback
        .lines()
        .skip(1)
        .find_map(|frame| locate(frame.trim()).0)
}

fn from_lua(err: &mlua::Error) -> (Option<u32>, String) {
    match err {
        mlua::Error::SyntaxError { message, .. } => {
            let (line, msg) = locate(message);
            (line, format!("syntax error: {msg}"))
        }
        mlua::Error::RuntimeError(message) => {
            let (line, msg) = locate(message);
            (line, msg.to_owned())
        }
        mlua::Error::MemoryError(_) => (None, "out of memory".to_owned()),
        mlua::Error::CallbackError { cause, traceback } => {
            let (line, msg) = from_lua(cause);
            (line.or_else(|| traceback_line(traceback)), msg)
        }
        other => {
            let text = other.to_string();
            let (line, msg) = locate(&text);
            (line, msg.to_owned())
        }
    }
}

/// Diagnostic for a file that failed to load.
pub(crate) fn from_failure(file: &str, failure: &Failure) -> PluginDiagnostic {
    let (line, message) = match failure {
        Failure::InstructionLimit => (None, "instruction limit exceeded".to_owned()),
        Failure::Lua(err) => from_lua(err),
    };
    PluginDiagnostic {
        source: file.to_owned(),
        message,
        line,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locates_the_line_after_the_chunk_name() {
        assert_eq!(
            locate("/home/u/.config/crest/init.lua:4: unexpected symbol near 'x'"),
            (Some(4), "unexpected symbol near 'x'")
        );
        assert_eq!(locate("...ng/path/init.lua:12: boom"), (Some(12), "boom"));
        assert_eq!(locate("no location here"), (None, "no location here"));
        assert_eq!(locate("C:\\x\\init.lua:7: e"), (Some(7), "e"));
    }

    #[test]
    fn drops_the_traceback_from_the_message() {
        assert_eq!(
            locate("init.lua:2: boom\nstack traceback:\n\tinit.lua:2: in main chunk"),
            (Some(2), "boom")
        );
    }

    #[test]
    fn syntax_and_runtime_errors_carry_their_line() {
        let lua = mlua::Lua::new();
        let syntax = lua
            .load("x = = 1")
            .set_name("@init.lua")
            .exec()
            .unwrap_err();
        let d = from_failure("init.lua", &Failure::Lua(syntax));
        assert_eq!(d.line, Some(1));
        assert!(d.message.starts_with("syntax error:"), "{}", d.message);

        let runtime = lua
            .load("local a = 1\n\nerror('boom')")
            .set_name("@init.lua")
            .exec()
            .unwrap_err();
        let d = from_failure("init.lua", &Failure::Lua(runtime));
        assert_eq!((d.line, d.message.as_str()), (Some(3), "boom"));
    }

    #[test]
    fn errors_from_rust_functions_use_the_traceback_line() {
        let lua = mlua::Lua::new();
        let f = lua
            .create_function(|_, ()| -> mlua::Result<()> { Err(mlua::Error::runtime("nope")) })
            .unwrap();
        lua.globals().set("f", f).unwrap();
        let err = lua
            .load("\n\nf()")
            .set_name("@init.lua")
            .exec()
            .unwrap_err();
        let d = from_failure("init.lua", &Failure::Lua(err));
        assert_eq!(d.line, Some(3), "{d:?}");
        assert!(d.message.contains("nope"));
    }

    #[test]
    fn the_instruction_limit_has_no_line() {
        let d = from_failure("p/init.lua", &Failure::InstructionLimit);
        assert_eq!(d.line, None);
        assert_eq!(d.message, "instruction limit exceeded");
    }
}
