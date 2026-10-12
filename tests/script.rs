use astra::build::{lua, script};
use astra::config::BuildTarget;
use indexmap::IndexMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
struct LogWriter(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for LogWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().write(bytes)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn build_script(
    src: &str,
    file: &Path,
    dirs: &[PathBuf],
    vars: &IndexMap<String, String>,
    is_bundled: bool,
) -> anyhow::Result<String> {
    let target: BuildTarget = serde_json::from_value(serde_json::json!({
        "path": file.to_string_lossy().into_owned(),
    }))?;
    script::build(src, &target, dirs, vars, is_bundled, ".anm2").map(|output| output.script)
}

fn capture_warnings() -> (Arc<Mutex<Vec<u8>>>, tracing::subscriber::DefaultGuard) {
    capture_logs(tracing::Level::WARN)
}

fn capture_logs(level: tracing::Level) -> (Arc<Mutex<Vec<u8>>>, tracing::subscriber::DefaultGuard) {
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let writer = LogWriter(Arc::clone(&bytes));
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_max_level(level)
        .with_writer(move || writer.clone())
        .finish();
    let guard = tracing::subscriber::set_default(subscriber);
    (bytes, guard)
}

fn build_shader(src: &str, dirs: &[PathBuf]) -> anyhow::Result<String> {
    let prefix = "--[[pixelshader@main:\n";
    let suffix = "\nfloat4 main() : SV_Target { return 0; }\n]]\n";
    build_script(
        &format!("{prefix}{src}{suffix}"),
        Path::new("test.anm2"),
        dirs,
        &IndexMap::new(),
        false,
    )
    .map(|output| {
        output
            .strip_prefix(prefix)
            .unwrap()
            .strip_suffix(suffix)
            .unwrap()
            .to_owned()
    })
}

#[test]
fn replaces_inline_lua_values_and_preserves_separators() -> anyhow::Result<()> {
    let vars = IndexMap::from([
        ("AAA".to_owned(), "get_values()".to_owned()),
        ("BBB".to_owned(), "{first = 2, second = 3}".to_owned()),
    ]);
    for (src, expected) in [
        (
            "local a, b = --[[#replace AAA]] first + second, --[[#replace BBB]] (other); -- retained\n",
            "local a, b =  get_values(),  {first = 2, second = 3}; -- retained\n",
        ),
        (
            "local values = { value = --[[#replace BBB]] {}, --[[#replace AAA]] first + second, other }\n",
            "local values = { value =  {first = 2, second = 3},  get_values(), other }\n",
        ),
        (
            "settings.value = --[[#replace BBB]] {\n    first = 0,\n    second = 0,\n}  ; -- retained\n",
            "settings.value =  {first = 2, second = 3}  ; -- retained\n",
        ),
        (
            "local value = flag ? --[[#replace AAA]] first + second : other\n",
            "local value = flag ?  get_values() : other\n",
        ),
        (
            "local value = flag ? function() return --[[#replace AAA]] object:method() end : nil\n",
            "local value = flag ? function() return  get_values() end : nil\n",
        ),
    ] {
        assert_eq!(build_script(src, Path::new("test.anm2"), &[], &vars, false)?, expected);
    }
    Ok(())
}

#[test]
fn expose_outputs() -> anyhow::Result<()> {
    let target: BuildTarget = serde_json::from_value(serde_json::json!({ "path": "input.anm2" }))?;
    let src = "--value@value:名前, 1\nreturn value";
    let output = script::build(src, &target, &[], &IndexMap::new(), false, ".anm2")?;
    assert_eq!(output.script, format!("{src}\n"));
    assert!(output.l10n.contains("名前=\n"), "{}", output.l10n);
    Ok(())
}

#[test]
fn retain_caller_variables() -> anyhow::Result<()> {
    let vars = IndexMap::from([("VALUE".to_owned(), "1".to_owned())]);
    let src = "--#define VALUE 2\nlocal value = ${VALUE}\n--#undef VALUE\n";
    assert_eq!(
        build_script(src, Path::new("input.anm2"), &[], &vars, false)?,
        "local value = 2\n"
    );
    assert_eq!(vars["VALUE"], "1");
    Ok(())
}

#[test]
fn replaces_lua_statement_values_and_preserves_comments() -> anyhow::Result<()> {
    let vars = IndexMap::from([("AAA".to_owned(), "{first = 2, second = 3}".to_owned())]);
    for (src, expected) in [
        (
            "  --#replace AAA\n-- retained\n\n  local a = first + second  ; local b = 0\n",
            "-- retained\n\n  local a = {first = 2, second = 3}  ; local b = 0\n",
        ),
        (
            "--#replace AAA\nsettings.value = {\n first = 0, second = 0\n} -- retained\n",
            "settings.value = {first = 2, second = 3} -- retained\n",
        ),
        (
            "local a = --[[\n#replace\n AAA\n]] 0\n",
            "local a =  {first = 2, second = 3}\n",
        ),
        (
            "--#if false\n--#replace MISSING\nlocal a = 0\n--#endif\nlocal b = 0\n",
            "local b = 0\n",
        ),
    ] {
        assert_eq!(build_script(src, Path::new("test.anm2"), &[], &vars, false)?, expected);
    }
    for src in [
        "--#replace AAA",
        "--#replace AAA\nlocal a",
        "--#replace AAA\nlocal a, b = 0",
        "--#replace AAA\nlocal a = 0, 0",
        "--#replace AAA\nif flag then local a = 0 end",
        "--#replace AAA\nlocal function a() return 0 end",
        "--#replace AAA\n--#define OTHER 1\nlocal a = 0",
        "--#replace AAA\n--#include \"missing.lua\"\nlocal a = 0",
        "--#replace AAA\nlocal a = --[[#replace AAA]]0",
    ] {
        assert!(
            build_script(src, Path::new("test.anm2"), &[], &vars, false).is_err(),
            "{src}"
        );
    }
    Ok(())
}

#[test]
fn replaces_hlsl_statement_values_and_rejects_multiple_members() -> anyhow::Result<()> {
    let target: BuildTarget = serde_json::from_value(serde_json::json!({ "path": "test.hlsl" }))?;
    let vars = IndexMap::from([("AAA".to_owned(), "{2, 3}".to_owned())]);
    for (src, expected) in [
        (
            "  //#replace AAA\n// retained\n\n  float values[2] = {0, 0}  ; float b = 0;\n",
            "// retained\n\n  float values[2] = {2, 3}  ; float b = 0;\n",
        ),
        (
            "//#replace AAA\nsettings.values[index] = {0, 0};\n",
            "settings.values[index] = {2, 3};\n",
        ),
        (
            "//#replace AAA\nvector<float, 2> value = float2(0, 0);\n",
            "vector<float, 2> value = {2, 3};\n",
        ),
        (
            "float values[2] = /*\n#replace\n AAA\n*/ {0, 0};\n",
            "float values[2] =  {2, 3};\n",
        ),
    ] {
        assert_eq!(
            astra::build::shader::build(src, Path::new("test.hlsl"), &target, &[], &vars)?.text,
            expected
        );
    }
    for src in [
        "//#replace AAA",
        "//#replace AAA\nfloat a;",
        "//#replace AAA\nfloat a = 0, b = 0;",
        "//#replace AAA\nfloat a, b = 0;",
        "//#replace AAA\na = 0, b = 0;",
        "//#replace AAA\nif (flag) a = 0;",
        "//#replace AAA\nreturn a = 0;",
        "//#replace AAA\n//#define OTHER 1\nfloat a = 0;",
        "//#replace AAA\n//#include \"missing.hlsl\"\nfloat a = 0;",
        "//#replace AAA\nfloat a = /*#replace AAA*/0;",
    ] {
        assert!(
            astra::build::shader::build(src, Path::new("test.hlsl"), &target, &[], &vars).is_err(),
            "{src}"
        );
    }
    Ok(())
}

#[test]
fn resolves_replacement_values_at_the_directive_position() -> anyhow::Result<()> {
    let src = "--#define AAA 1\n\
        local a = --[[#replace AAA]]0\n\
        --#define AAA 2\n\
        local b = --[[#replace AAA]]0\n\
        --#if false\n\
        local ignored = --[[#replace MISSING]]0\n\
        --#endif\n";
    assert_eq!(
        build_script(src, Path::new("test.anm2"), &[], &IndexMap::new(), false)?,
        "local a = 1\nlocal b = 2\n"
    );
    let src = "local text = '--[[#replace MISSING]]0'\n";
    assert_eq!(
        build_script(src, Path::new("test.anm2"), &[], &IndexMap::new(), false)?,
        src
    );
    Ok(())
}

#[test]
fn keeps_token_boundaries_when_removing_replacement_comments() -> anyhow::Result<()> {
    let vars = IndexMap::from([("AAA".to_owned(), "get_values()".to_owned())]);
    assert_eq!(
        build_script("return--[[#replace AAA]]0", Path::new("test.anm2"), &[], &vars, false)?,
        "return get_values()\n"
    );
    let target: BuildTarget = serde_json::from_value(serde_json::json!({ "path": "test.hlsl" }))?;
    let src = "float main() { return/*#replace AAA*/0; }";
    let output = astra::build::shader::build(src, Path::new("test.hlsl"), &target, &[], &vars)?;
    assert_eq!(output.text, "float main() { return get_values(); }\n");

    let vars = IndexMap::from([("AAA".to_owned(), "1 -- retained".to_owned())]);
    assert!(
        build_script(
            "local a = --[[#replace AAA]]0;",
            Path::new("test.anm2"),
            &[],
            &vars,
            false
        )
        .is_err()
    );
    let vars = IndexMap::from([("AAA".to_owned(), "1 // retained".to_owned())]);
    assert!(
        astra::build::shader::build(
            "float a = /*#replace AAA*/0;",
            Path::new("test.hlsl"),
            &target,
            &[],
            &vars
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn preserves_module_include_positions_after_replacing_values() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("test.anm2");
    std::fs::write(dir.path().join("lib.lua"), "return --[[#replace VALUE]]0\n")?;
    let vars = IndexMap::from([
        ("VALUE".to_owned(), "{number = 42}".to_owned()),
        ("MODULE".to_owned(), "'lib'".to_owned()),
    ]);
    let src = "local before = --[[#replace VALUE]]0\n\
        --#include \"lib.lua\"\n\
        local lib = require(--[[#replace MODULE]]'dummy')\n\
        local retained = lib.number\n";
    let output = build_script(src, &file, &[], &vars, false)?;
    assert!(output.contains("local before = {number = 42}"), "{output}");
    assert!(output.contains("return {number = 42}"), "{output}");
    assert!(output.contains("local lib = (function()"), "{output}");
    assert!(output.contains("local retained = lib.number"), "{output}");
    assert!(!output.contains("#replace") && !output.contains("'dummy'"), "{output}");
    Ok(())
}

#[test]
fn replaces_inline_hlsl_initializers_and_individual_members() -> anyhow::Result<()> {
    let target: BuildTarget = serde_json::from_value(serde_json::json!({ "path": "test.hlsl" }))?;
    let vars = IndexMap::from([
        ("AAA".to_owned(), "first + second".to_owned()),
        ("BBB".to_owned(), "{2, 3}".to_owned()),
    ]);
    for (src, expected) in [
        (
            "float a = /*#replace AAA*/ 0, b = /*#replace AAA*/ (0); // retained\n",
            "float a =  first + second, b =  first + second; // retained\n",
        ),
        (
            "float values[2] = /*#replace BBB*/ {\n    0, 0\n}  ;\n",
            "float values[2] =  {2, 3}  ;\n",
        ),
        (
            "float values[2] = { /*#replace AAA*/ 0, 1 };\n",
            "float values[2] = {  first + second, 1 };\n",
        ),
    ] {
        let output = astra::build::shader::build(src, Path::new("test.hlsl"), &target, &[], &vars)?;
        assert_eq!(output.text, expected);
    }
    Ok(())
}

#[test]
fn rejects_replacements_that_are_not_one_expression() -> anyhow::Result<()> {
    let target: BuildTarget = serde_json::from_value(serde_json::json!({ "path": "test.hlsl" }))?;
    for value in ["", "1, 2", "1; other()", "1 +", "{1 2}", "1e+", "0xGG", "1.2.3"] {
        let vars = IndexMap::from([("AAA".to_owned(), value.to_owned())]);
        assert!(
            build_script(
                "local a = --[[#replace AAA]]0",
                Path::new("test.anm2"),
                &[],
                &vars,
                false
            )
            .is_err(),
            "{value}"
        );
        assert!(
            astra::build::shader::build(
                "float a = /*#replace AAA*/0;",
                Path::new("test.hlsl"),
                &target,
                &[],
                &vars
            )
            .is_err(),
            "{value}"
        );
    }
    let vars = IndexMap::from([("AAA".to_owned(), "1".to_owned())]);
    for src in [
        "--#replace AAA\nlocal a, b = 0, 0",
        "local a = --[[#replace AAA]]",
        "local a = --[[#replace AAA]]{--[[#replace AAA]]0}",
        "local a = --[[#replace MISSING]]0",
    ] {
        assert!(
            build_script(src, Path::new("test.anm2"), &[], &vars, false).is_err(),
            "{src}"
        );
    }
    Ok(())
}

#[test]
fn replaces_hlsl_values_with_casts_and_parenthesized_postfix_expressions() -> anyhow::Result<()> {
    let target: BuildTarget = serde_json::from_value(serde_json::json!({ "path": "test.hlsl" }))?;
    for value in ["(value)++", "(vector<float, 2>)value", "(float4)(value + other)"] {
        let vars = IndexMap::from([("VALUE".to_owned(), value.to_owned())]);
        let src = "float2 result = /*#replace VALUE*/0;\n";
        let output = astra::build::shader::build(src, Path::new("test.hlsl"), &target, &[], &vars)?;
        assert_eq!(output.text, format!("float2 result = {value};\n"));
    }
    Ok(())
}

#[test]
fn rejects_unclosed_shader_blocks_during_analysis() -> anyhow::Result<()> {
    for body in [
        "float4 main() : SV_Target { return 0;",
        "struct Element { float value;",
        "cbuffer Values { float value;",
    ] {
        let src = format!("--[[pixelshader@main:\n{body}\n]]\n");
        let error = build_script(&src, Path::new("test.anm2"), &[], &IndexMap::new(), false).unwrap_err();
        assert!(error.to_string().contains("unclosed declaration block"), "{error}");
    }
    Ok(())
}

#[test]
fn omits_lua_statements_and_preserves_following_code() -> anyhow::Result<()> {
    for statement in [
        "local aaa, bbb, ccc",
        "local removed = function() return 1 end",
        "if flag then removed() else removed() end",
        "for i = 1, 10 do removed(i) end",
        "while flag do removed() end",
        "repeat removed() until check()",
    ] {
        let src = format!("local before = 1\n--#omit\n---@type function\n{statement}; local retained = 2\n");
        let output = build_script(&src, Path::new("test.anm2"), &[], &IndexMap::new(), false)?;
        assert!(output.contains("local before = 1"), "{output}");
        assert!(output.contains("local retained = 2"), "{output}");
        assert!(!output.contains(statement), "{output}");
        assert!(!output.contains("#omit") && !output.contains("---@type"), "{output}");
    }

    let dir = tempfile::tempdir()?;
    let file = dir.path().join("test.anm2");
    std::fs::write(
        dir.path().join("lib.lua"),
        "local function clamp(value) return value end\n",
    )?;
    let src = "--#omit\n---@type function\nlocal clamp\n--#include \"lib.lua\"\nlocal value = clamp(1)\n";
    let output = build_script(src, &file, &[], &IndexMap::new(), false)?;
    assert!(output.contains("local function clamp(value)"), "{output}");
    assert!(output.contains("local value = clamp(1)"), "{output}");
    assert!(!output.contains("---@type"), "{output}");

    Ok(())
}

#[test]
fn rejects_omit_without_a_complete_target() -> anyhow::Result<()> {
    for src in [
        "--#omit",
        "--#omit\n-- annotation",
        "--#omit\nwhile flag do removed()",
        "--#omit extra\nlocal removed",
        "--#omit\n--#include \"missing.lua\"\nlocal removed",
    ] {
        assert!(
            build_script(src, Path::new("test.anm2"), &[], &IndexMap::new(), false).is_err(),
            "{src}"
        );
    }
    let src = "--#if false\n--#omit\n--#endif\nlocal retained = 1";
    let output = build_script(src, Path::new("test.anm2"), &[], &IndexMap::new(), false)?;
    assert!(output.contains("local retained = 1"), "{output}");

    let target: BuildTarget = serde_json::from_value(serde_json::json!({ "path": "test.ini" }))?;
    for src in [";#omit", ";#omit\n; annotation", ";#omit\n;#endif"] {
        assert!(
            astra::build::ini::build(src, &target, &[], &IndexMap::new()).is_err(),
            "{src}"
        );
    }
    Ok(())
}

#[test]
fn preserves_cols_after_trimming_leading_whitespace() {
    for (prefix, line, col) in [("  ", 1, 3), ("\n\t  ", 2, 4)] {
        let src = format!("{prefix}--[[pixelshader@float:float4 main() : SV_Target {{ return 0; }}]]");
        let err = build_script(&src, Path::new("test.anm2"), &[], &IndexMap::new(), false).unwrap_err();
        assert!(
            err.to_string()
                .contains(&format!("test.anm2:{line}:{col}: name of 'pixelshader' cannot")),
            "{err}"
        );
    }
}

#[test]
fn rejects_malformed_preprocessor_directives() {
    for (src, message) in [
        ("--#ifdef", "missing variable name"),
        ("--#ifndef", "missing variable name"),
        ("--#define 123 value", "invalid variable name"),
        ("--#undef VALUE extra", "unexpected argument"),
        ("--#include \"unfinished", "unclosed include path"),
        ("--#include <unfinished", "enclosed in quotes or angle brackets"),
        ("--#include \"\"", "empty include path"),
    ] {
        let err = build_script(src, Path::new("test.anm2"), &[], &IndexMap::new(), false).unwrap_err();
        assert!(err.to_string().contains("test.anm2:1:3:"), "{src}: {err}");
        assert!(err.to_string().contains(message), "{src}: {err}");
    }
}

#[test]
fn treats_only_the_first_trimmed_line_as_a_header() -> anyhow::Result<()> {
    let file = Path::new("test.anm2");
    let vars = IndexMap::new();
    let src = "\n \t@名前 ' [==[\nreturn 1";
    assert_eq!(build_script(src, file, &[], &vars, true)?, "@名前 ' [==[\nreturn 1\n");

    for (src, pos) in [
        ("@first\n@second\nreturn 1", "2:1"),
        ("return 1\n  @second", "2:3"),
        ("local value = @2", "1:15"),
        ("--#define VALUE 1\n@second", "2:1"),
    ] {
        let err = build_script(src, file, &[], &vars, true).unwrap_err();
        assert_eq!(err.to_string(), format!("test.anm2:{pos}: unexpected '@'"), "{src}");
    }

    let src = "local value = \"@value\"\n-- @comment\nreturn value";
    assert_eq!(build_script(src, file, &[], &vars, false)?, format!("{src}\n"));

    let src = "\n \t@header\n  --[[pixelshader @float:float4 main() : SV_Target { return 0; }]]";
    let err = build_script(src, file, &[], &vars, true).unwrap_err();
    assert!(
        err.to_string().contains("test.anm2:3:3: name of 'pixelshader' cannot"),
        "{err}"
    );
    Ok(())
}

#[test]
fn preprocesses_indented_shader_blocks_with_whitespace_before_at() -> anyhow::Result<()> {
    for kind in ["pixelshader", "computeshader"] {
        for space in ["", " ", "\t "] {
            let entry = entry_point(kind, "main");
            let src = format!(" \t--[[{kind}{space}@main:\n--#define VALUE 1\nfloat value = ${{VALUE}};\n{entry}\n]]");
            let output = build_script(&src, Path::new("test.anm2"), &[], &IndexMap::new(), false)?;
            assert_eq!(
                output,
                format!("--[[{kind}{space}@main:\nfloat value = 1;\n{entry}\n]]\n")
            );
        }
    }
    Ok(())
}

#[test]
fn rejects_shader_blocks_after_code() {
    for kind in ["pixelshader", "computeshader"] {
        for prefix in ["local shader = ", "local value = 1; "] {
            let src = format!("{prefix}--[[{kind} @main:\n--#include \"missing.hlsl\"\n]]");
            let err = build_script(&src, Path::new("test.anm2"), &[], &IndexMap::new(), false).unwrap_err();
            assert!(
                err.to_string().contains("shader block must start on its own line"),
                "{err}"
            );
        }
    }
}

#[test]
fn keeps_other_long_comments_and_strings_outside_shader_preprocessing() -> anyhow::Result<()> {
    for src in [
        "--[=[pixelshader @main:\n--#include \"missing.hlsl\"\n]=]",
        "local shader = [[pixelshader @main:\n--#include \"missing.hlsl\"\n]]",
    ] {
        let output = build_script(src, Path::new("test.anm2"), &[], &IndexMap::new(), false)?;
        assert_eq!(output, format!("{src}\n"));
    }
    Ok(())
}

#[test]
fn expand_multiline_literals() -> anyhow::Result<()> {
    let vars = IndexMap::from([("VALUE".to_owned(), "value".to_owned())]);

    for src in [
        "local text = [[${{ VALUE }}\nnext]]",
        "local text = [=[${{ VALUE }}\n\"${{ VALUE }}]=]",
        "--[[${{ VALUE }}\n\"${{ VALUE }}\n]]\nlocal value = 1",
        "local text = \"${{ VALUE }}\\\nnext\"",
        "local text = [[first\n\"${{ VALUE }}\nlast]]",
    ] {
        let output = build_script(src, Path::new("test.anm2"), &[], &vars, false)?;
        assert_eq!(output, format!("{}\n", src.replace("${{ VALUE }}", "value")), "{src}");
    }

    let src = "local text = [[first\n${{ MISSING }}\nlast]]";
    let err = build_script(src, Path::new("test.anm2"), &[], &vars, false).unwrap_err();
    assert_eq!(err.to_string(), "test.anm2:2:1: variable 'MISSING' not found");
    Ok(())
}

#[test]
fn expand_shader_names() -> anyhow::Result<()> {
    let vars = IndexMap::from([("NAME".to_owned(), "main".to_owned())]);

    for (kind, entry) in [
        ("pixelshader", "float4 main() : SV_Target { return 0; }"),
        ("computeshader", "[numthreads(1, 1, 1)] void main() {}"),
    ] {
        let src = format!("--[[{kind}@${{{{ NAME }}}}:\n--#define NAME changed\n{entry}\n]]");
        let output = build_script(&src, Path::new("test.anm2"), &[], &vars, false)?;
        assert!(output.contains(&format!("--[[{kind}@main:")), "{output}");
        assert!(output.contains(entry), "{output}");

        let src = format!("--[[{kind}@${{{{ MISSING }}}}:\n{entry}\n]]");
        let err = build_script(&src, Path::new("test.anm2"), &[], &vars, false).unwrap_err();
        assert_eq!(
            err.to_string(),
            format!("test.anm2:1:{}: variable 'MISSING' not found", kind.len() + 6)
        );
    }

    Ok(())
}

#[test]
fn builds_directives_conditions_properties_and_headers() -> anyhow::Result<()> {
    let vars = IndexMap::from([
        ("PLATFORM".to_owned(), "win".to_owned()),
        ("TARGET".to_owned(), "client".to_owned()),
        ("DEBUG".to_owned(), "false".to_owned()),
        ("VERSION".to_owned(), "3".to_owned()),
        ("HEX".to_owned(), "0x10".to_owned()),
        ("LINE_KEY".to_owned(), "LINE".to_owned()),
        ("MISSING_KEY".to_owned(), "MISSING".to_owned()),
        ("\"LINE\"".to_owned(), "1".to_owned()),
        ("\"PLATFORM\"".to_owned(), "1".to_owned()),
    ]);
    let src = r#"
--#define LINE 1
--[==[#define BLOCK [[nested]] ]==]
literal = [[
--#define HIDDEN hidden
]]
inline = true --#define INLINE 1
--[[#if 0
block_if_is_a_comment = true
]]
block_if_survived = true
--#if ${LINE} == 0
selected = "if"
--#elif ${LINE} == 1 and '${PLATFORM}' == 'win' and not ${DEBUG}
selected = "elif"
--#else
selected = "else"
--#endif
--#ifdef LINE
ifdef_selected = true
--#endif
--#ifdef ${{ LINE_KEY }}
ifdef_placeholder_selected = true
--#endif
--#ifdef "${{ LINE_KEY }}"
ifdef_quoted_placeholder_selected = true
--#endif
--#ifndef MISSING
ifndef_selected = true
--#endif
--#ifndef ${ MISSING_KEY }
ifndef_placeholder_selected = true
--#endif
--#ifndef "${{ MISSING_KEY }}"
ifndef_quoted_placeholder_selected = true
--#endif
--#if defined(PLATFORM) && '${PLATFORM}' == 'win' && ${VERSION} >= 3
platform_selected = true
--#endif
--#if ${HEX} == 16
hex_selected = true
--#endif
--#if ${LINE} ~= 0 or false
operator_selected = true
--#endif
--#if 1.5
float_selected = true
--#endif
--#if 'value'
string_selected = true
--#endif
--#if nil
nil_selected = true
--#else
nil_fallback = true
--#endif
--#if not defined(MISSING)
missing_is_undefined = true
--#endif
--#if "${{ PLATFORM }}" == 'win' && '${{ TARGET }}' == 'client'
quoted_selected = true
--#endif
--#if '${ PLATFORM }' == 'win'
placeholder_selected = true
--#endif
--#if defined "PLATFORM" and not defined(UNDEFINED)
defined_selected = true
--#endif -- trailing comment
--#if ${LINE} == 1
--#if '${PLATFORM}' == 'win'
nested_selected = true
--#elif (
nested_elif_selected = true
--#else
nested_else_selected = true
--#endif
--#endif
block = "${{ BLOCK }}"
--#undef BLOCK
--#define REPLACED first
--#define REPLACED second
replacement = "${REPLACED}"
--#undef REPLACED
--#define LATER_DEFINED defined_val
after_scope = "${LATER_DEFINED}"
--#undef LATER_DEFINED
--#define EMPTY
--#ifdef EMPTY
empty_define_selected = true
--#endif
--#undef EMPTY
--#undef LINE
--#ifdef LINE
after_line = "defined"
--#else
after_line = "undefined"
--#endif
--#ifdef INLINE
inline_define_leaked = true
--#endif
value = 10 --track@_:val,0,100,_,1
global = true --check@global:glob,true
primary = 0xffffff --color@primary:col,0xffffff
local no_init --track@_:no_init,0,50,0,1
spaced = 20 --track@ _ : spaced_val, 0, 100, _ , 1
spaced_chk = true --check@ _ : chk, _
spaced_sel = 2 --select@ _ : sel = _ , item1=1, item2=2
sample_file = "test.png" --file@ _ : file_prop
bracket_str = [[bracket]] --string@ _ : str_prop, _
single_str = 'single' --text@ _ : txt_prop, _
matched_str = "matched" --text@ _ : matched_prop, matched
win_path = "C:\\Windows\\System32" --file@ _ : path_prop
quote_str = "aaa\"bbb\"" --string@ _ : quote_prop, _
newline_str = "aaa\nbbb" --text@ _ : nl_prop, _
newline_matched = "first\nsecond" --text@ _ : nl_match_prop, first\nsecond
sample_color = 0xff0000 --color@ _ : col_prop, _
hex_color = 0xFF0000 --color@ _ : hex_prop, 0xff0000
nil_color = nil --color@_:nil_prop,nil
sample_nil_color = nil --color@ _ : sample_nil_prop, _
--color@default_nil_color:default_nil_prop,nil
--color:nil
val_num = 1.5 --value@ _ : val_num_prop, _
val_str = "hello" --value@ _ : val_str_prop, _
val_tbl = { 1, 2 } --value@ _ : val_tbl_prop, _
val_tbl_spaced = { 1, 2 } --value@ _ : val_tbl_spaced_prop, {1,2}
ignored_hyphens = 1 ---track@ignored_hyphens:0,100,1
ignored_kind_space = 2 -- track@ignored_kind_space:0,100,1
at_space = 3 --track @at_space:at_space_prop,0,100,3,1
"#;
    let result = build_script(src, Path::new("test.lua"), &[], &vars, false)?;

    for expected in [
        "selected = \"elif\"",
        "block_if_survived = true",
        "ifdef_selected = true",
        "ifdef_placeholder_selected = true",
        "ifdef_quoted_placeholder_selected = true",
        "ifndef_selected = true",
        "ifndef_placeholder_selected = true",
        "ifndef_quoted_placeholder_selected = true",
        "platform_selected = true",
        "hex_selected = true",
        "operator_selected = true",
        "float_selected = true",
        "string_selected = true",
        "nil_fallback = true",
        "missing_is_undefined = true",
        "quoted_selected = true",
        "placeholder_selected = true",
        "defined_selected = true",
        "nested_selected = true",
        "block = \"[[nested]]\"",
        "replacement = \"second\"",
        "after_scope = \"defined_val\"",
        "empty_define_selected = true",
        "after_line = \"undefined\"",
        "inline = true --#define INLINE 1",
        "--track@value:val,0,100,10,1",
        "--check@global:glob,true",
        "--color@primary:col,0xffffff",
        "--track@no_init:no_init,0,50,0,1",
        "--track@ spaced : spaced_val, 0, 100, 20 , 1",
        "--check@ spaced_chk : chk, true",
        "--select@ spaced_sel : sel = 2 , item1=1, item2=2",
        "--file@ sample_file : file_prop",
        "--string@ bracket_str : str_prop, bracket",
        "--text@ single_str : txt_prop, single",
        "--text@ matched_str : matched_prop, matched",
        "--file@ win_path : path_prop",
        "--string@ quote_str : quote_prop, aaa\"bbb\"",
        "--text@ newline_str : nl_prop, aaa\\nbbb",
        "--text@ newline_matched : nl_match_prop, first\\nsecond",
        "--color@ sample_color : col_prop, 0xff0000",
        "--color@ hex_color : hex_prop, 0xff0000",
        "--color@nil_color:nil_prop,nil",
        "--color@ sample_nil_color : sample_nil_prop, nil",
        "--color@default_nil_color:default_nil_prop,nil",
        "--color:nil",
        "--value@ val_num : val_num_prop, 1.5",
        "--value@ val_str : val_str_prop, \"hello\"",
        "--value@ val_tbl : val_tbl_prop, { 1, 2 }",
        "--value@ val_tbl_spaced : val_tbl_spaced_prop, {1,2}",
        "ignored_hyphens = 1 ---track@ignored_hyphens:0,100,1",
        "ignored_kind_space = 2 -- track@ignored_kind_space:0,100,1",
        "--track@at_space:at_space_prop,0,100,3,1",
        "--#define HIDDEN hidden",
    ] {
        assert!(result.contains(expected), "missing {expected:?}\n{result}");
    }

    for unexpected in [
        "selected = \"if\"",
        "selected = \"else\"",
        "nil_selected = true",
        "nested_elif_selected = true",
        "nested_else_selected = true",
        "after_line = \"defined\"",
        "inline_define_leaked = true",
        "--#define LINE",
    ] {
        assert!(!result.contains(unexpected), "unexpected {unexpected:?}\n{result}");
    }

    Ok(())
}

#[test]
fn normalize_headers() -> anyhow::Result<()> {
    for (src, is_bundled, expected) in [
        ("", false, ""),
        ("@custom\nlocal value = 1", true, "@custom\nlocal value = 1\n"),
        ("--@ custom\nlocal value = 1", true, "@custom\nlocal value = 1\n"),
        ("--@ custom\nlocal value = 1", false, "--@ custom\nlocal value = 1\n"),
        ("local value = 1", true, "@test\nlocal value = 1\n"),
        (
            "--color:0xffffff\nlocal value = 1",
            true,
            "@test\n--color:0xffffff\nlocal value = 1\n",
        ),
        (
            "--select@a:key=eq=1,item=1=1,==2\nlocal value = 1",
            false,
            "--select@a:key=eq=1,item=1=1,==2\nlocal value = 1\n",
        ),
    ] {
        assert_eq!(
            build_script(src, Path::new("test.lua"), &[], &IndexMap::new(), is_bundled)?,
            expected
        );
    }

    assert!(
        build_script(
            "@custom\nlocal value = 1",
            Path::new("test.lua"),
            &[],
            &IndexMap::new(),
            false
        )
        .is_err()
    );

    Ok(())
}

#[test]
fn reject_properties() -> anyhow::Result<()> {
    for (src, message) in [
        ("t.prop = 10 --track@_:0,100,1", "table field assignment"),
        (r#"t["prop"] = 10 --track@_:0,100,1"#, "table field assignment"),
        (
            "local value = 10 --track@other:0,100,1",
            "does not match assignment target",
        ),
        (
            "local value = 10 --track@ other : 0,100,1",
            "does not match assignment target",
        ),
        ("local value --track@other:0,100,1", "does not match assignment target"),
        (
            "local value = 10 --data@_:0",
            "'data' definition cannot be attached to variable declaration",
        ),
        (
            "local value = 10 --hide@_:0",
            "'hide' definition cannot be attached to variable declaration",
        ),
        (
            "local value = 10 --track@_:A,0,100,20,1",
            "default value '20' does not match assignment value '10'",
        ),
        (
            "local value = 10 --track@ _ : A, 0, 100, 20, 1",
            "default value '20' does not match assignment value '10'",
        ),
        (
            "local value = false --check@_:B,true",
            "default value 'true' does not match assignment value 'false'",
        ),
        (
            "local value = 0 --check@_:B,false",
            "default value 'false' does not match assignment value '0'",
        ),
        (
            "local value = false --checksection@_:B,true",
            "default value 'true' does not match assignment value 'false'",
        ),
        (
            "local value = 0xffffff --color@_:A,0x000000",
            "default value '0x000000' does not match assignment value '0xffffff'",
        ),
        (
            "local value = nil --color@_:A,0xffffff",
            "default value '0xffffff' does not match assignment value 'nil'",
        ),
        (
            "local value = 0xffffff --color@_:A,nil",
            "default value 'nil' does not match assignment value '0xffffff'",
        ),
        (
            "local value = nil --color@_:A,invalid",
            "default value 'invalid' does not match assignment value 'nil'",
        ),
        (
            "--color@_:A,0x1000000\n",
            "must be 'nil' or an integer between 0x000000 and 0xffffff",
        ),
        (
            "--color:0x1000000\n",
            "must be 'nil' or an integer between 0x000000 and 0xffffff",
        ),
        ("--color:invalid\n", "must be 'nil' or an integer"),
        (
            "local value = 1 --select@_:C=2,item1=1,item2=2",
            "default value '2' does not match assignment value '1'",
        ),
        ("local value = 'str' --track@_:A,0,100,0,1", "must be a number"),
        (
            "local value = 2 --check@_:B,true",
            "must be 'true', 'false', '0', or '1'",
        ),
        ("local value = 1 --checksection@_:B,true", "must be 'true' or 'false'"),
        ("local value = 'str' --select@_:C=1,item1=1", "must be an integer"),
        (
            "local value = 'str' --color@_:A,0xffffff",
            "must be 'nil' or an integer",
        ),
        ("local value = raw_ident --file@_:A", "must be a string"),
        (
            "local value = 1 --value@_:A,2",
            "default value '2' does not match assignment value '1'",
        ),
        (
            "local value = 'bar' --text@_:A,baz",
            "default value 'baz' does not match assignment value ''bar''",
        ),
        (
            "local value = \"aaa\\tbbb\" --text@_:A,aaa",
            "cannot contain control characters",
        ),
        ("--track0:A,0,\t100,0,1", "cannot contain control characters"),
        ("--value@value:A\n", "'value' requires at least 1 argument"),
        (
            "--track@a:dup,0,100,0,1\n--track@b:dup,0,100,0,1\n",
            "duplicate property name 'dup'",
        ),
        ("--#else\n", "#else without matching"),
        ("--#endif\n", "#endif without matching"),
        ("--#elif 1\n", "#elif without matching"),
        ("--#if 1\n--#else\n--#else\n--#endif\n", "duplicate #else"),
        ("--#if 1\n--#else\n--#elif 1\n--#endif\n", "#elif after #else"),
        ("--#if 1\nvalue\n", "unclosed conditional"),
        ("--#if \n", "empty conditional expression"),
        ("--[[#define VALUE\nvalue", "unfinished long comment"),
        (
            "--#define BUILD_DIR overridden\n",
            "cannot define reserved variable 'BUILD_DIR'",
        ),
        ("--select@_:C=1,item1\n", "must contain '='"),
    ] {
        let error = build_script(src, Path::new("test.lua"), &[], &IndexMap::new(), false).unwrap_err();
        assert!(error.to_string().contains(message), "{src:?}: {error}");
    }

    Ok(())
}

fn entry_point(kind: &str, name: &str) -> String {
    if kind == "computeshader" {
        format!("[numthreads(1, 1, 1)] void {name}() {{}}")
    } else {
        format!("float4 {name}() : SV_Target {{ return 0; }}")
    }
}

#[test]
fn validates_shader_definition_names_as_hlsl_identifiers() -> anyhow::Result<()> {
    for kind in ["pixelshader", "computeshader"] {
        for name in [
            "main",
            "PS_Main",
            "_main2",
            "_",
            "float4_main",
            "Float4",
            "float5",
            "IF",
        ] {
            let src = format!(
                "--[[{kind}@{name}:\n{}\n]]\nobj.{kind}(\"{name}\")\n",
                entry_point(kind, name)
            );
            let output = build_script(&src, Path::new("test.anm2"), &[], &IndexMap::new(), false)?;
            assert_eq!(output, src);
        }

        let src = format!(
            "--[[{kind}@ psmain :\n{}\n]]\nobj.{kind}(\"psmain\")\n",
            entry_point(kind, "psmain")
        );
        let output = build_script(&src, Path::new("test.anm2"), &[], &IndexMap::new(), false)?;
        assert_eq!(output, src);

        for name in [
            "",
            "   ",
            "123main",
            "ps-main",
            "ps main",
            "main.name",
            "処理",
            "main😀",
        ] {
            let src = format!("\n--[[{kind}@{name}:\n]]\n");
            let error = build_script(&src, Path::new("test.anm2"), &[], &IndexMap::new(), false).unwrap_err();
            assert!(error.to_string().starts_with("test.anm2:2:1:"), "{name:?}: {error}");
            assert!(
                error.to_string().contains("must match [A-Za-z_][A-Za-z0-9_]*"),
                "{name:?}: {error}"
            );
        }

        for name in [
            "if",
            "return",
            "float",
            "float4",
            "float2x2",
            "uint3x4",
            "bool2",
            "double4",
            "min16float4",
            "Texture2D",
            "sampler2D",
            "vector",
            "matrix",
            "class",
            "template",
            "sizeof",
            "globallycoherent",
            "RasterizerOrderedBuffer",
        ] {
            let src = format!("--[[{kind}@{name}:\n]]\n");
            let error = build_script(&src, Path::new("test.anm2"), &[], &IndexMap::new(), false).unwrap_err();
            assert!(
                error.to_string().contains("cannot be an HLSL keyword or reserved word"),
                "{name}: {error}"
            );
        }

        for (name, expected) in [("main\t", "control characters"), ("main@other", "cannot contain '@'")] {
            let src = format!("--[[{kind}@{name}:\n]]\n");
            let error = build_script(&src, Path::new("test.anm2"), &[], &IndexMap::new(), false).unwrap_err();
            assert!(error.to_string().contains(expected), "{name:?}: {error}");
        }

        let body = entry_point(kind, "main");
        let src = format!("--[[{kind}@main:\n{body}\n]]\n--[[{kind}@main:\n{body}\n]]\n");
        let error = build_script(&src, Path::new("test.anm2"), &[], &IndexMap::new(), false).unwrap_err();
        assert!(
            error
                .to_string()
                .contains(&format!("duplicate {kind} definition 'main'")),
            "{error}"
        );

        let src = format!("--#if false\n--[[{kind}@123invalid:\n]]\n--#endif\nreturn 1\n");
        let output = build_script(&src, Path::new("test.anm2"), &[], &IndexMap::new(), false)?;
        assert_eq!(output, "return 1\n");
    }

    Ok(())
}

#[test]
fn validates_shader_entry_point_definitions() -> anyhow::Result<()> {
    for (kind, name, body) in [
        (
            "pixelshader",
            "psmain",
            "float4 helper() { return 0; }\nfloat4 psmain(float2 uv : TEXCOORD0) : SV_Target { return helper(); }",
        ),
        (
            "computeshader",
            "csmain",
            "[numthreads(1, 1, 1)]\nvoid csmain(uint2 id : SV_DispatchThreadID) { }\nvoid helper() {}",
        ),
        (
            "pixelshader",
            "main",
            "struct Output { float4 color : SV_Target; };\nOutput main() { Output result; return result; }",
        ),
        (
            "pixelshader",
            "main",
            "vector<float, 4> main() : SV_Target { return 0; }",
        ),
        (
            "pixelshader",
            "main",
            "void main(out float4 color : SV_Target) { color = 0; }",
        ),
        (
            "pixelshader",
            "main",
            "RasterizerOrderedTexture2D<float4> output : register(u0);\n\
             void main(float4 pos : SV_Position) { output[uint2(pos.xy)] = 1; }",
        ),
        (
            "pixelshader",
            "main",
            "typedef float4 Color;\nColor main() : SV_Target { return 0; }",
        ),
        ("pixelshader", "main", "void main() {}"),
        (
            "pixelshader",
            "main",
            "float4 /* main() {} */ main /* gap */ ( ) : SV_Target { return 0; }",
        ),
        (
            "pixelshader",
            "main",
            "float4 main();\nfloat4 main() : SV_Target { return 0; }",
        ),
        (
            "pixelshader",
            "main",
            "#define UNUSED 1\nfloat4 main() : SV_Target { return 0; }",
        ),
        (
            "computeshader",
            "main",
            "[shader(\"compute\")] [numthreads(1, 1, 1)] inline void main() {}",
        ),
    ] {
        let src = format!("--[[{kind}@{name}:\n{body}\n]]\n");
        let output = build_script(&src, Path::new("test.anm2"), &[], &IndexMap::new(), false)?;
        assert_eq!(output, src);
    }

    for kind in ["pixelshader", "computeshader"] {
        for body in [
            "",
            "float4 other() : SV_Target { return 0; }",
            "// float4 main() : SV_Target {}",
            "/* float4 main() : SV_Target {} */",
            "#define BODY float4 main() {}",
            "float4 main() : SV_Target;",
            "float4 helper() { return main(); }",
            "float4 value = main();",
            "struct Methods { float4 main() { return 0; } };",
            "float4 main_other() : SV_Target { return 0; }",
            "float4 Main() : SV_Target { return 0; }",
            "--#if false\nfloat4 main() : SV_Target { return 0; }\n--#endif",
        ] {
            let src = format!("\n--[[{kind}@main:\n{body}\n]]\n");
            let error = build_script(&src, Path::new("test.anm2"), &[], &IndexMap::new(), false).unwrap_err();
            assert!(error.to_string().starts_with("test.anm2:2:1:"), "{body:?}: {error}");
            assert!(
                error
                    .to_string()
                    .contains(&format!("entry point 'main' is not defined in '{kind}' block")),
                "{body:?}: {error}"
            );
        }
    }

    for (kind, body) in [
        ("pixelshader", "[numthreads(1, 1, 1)] void main() {}"),
        ("computeshader", "void main() {}"),
        ("computeshader", "float4 main() { return 0; }"),
        ("computeshader", "[numthreads(1, 1, 1)] float4 main() { return 0; }"),
        ("computeshader", "[numthreads(1, 1, 1)] void other() {}\nvoid main() {}"),
    ] {
        let src = format!("\n--[[{kind}@main:\n{body}\n]]\n");
        let error = build_script(&src, Path::new("test.anm2"), &[], &IndexMap::new(), false).unwrap_err();
        assert!(
            error
                .to_string()
                .contains(&format!("entry point 'main' is not defined in '{kind}' block")),
            "{body:?}: {error}"
        );
    }

    let vars = IndexMap::from([("ENTRY".to_owned(), "main".to_owned())]);
    let src = "--[[pixelshader@main:\nfloat4 ${ENTRY}() : SV_Target { return 0; }\n]]\n";
    let output = build_script(src, Path::new("test.anm2"), &[], &vars, false)?;
    assert_eq!(
        output,
        "--[[pixelshader@main:\nfloat4 main() : SV_Target { return 0; }\n]]\n"
    );
    Ok(())
}

#[test]
fn resolves_directives_in_shader_blocks() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("main.lua");
    std::fs::write(
        dir.path().join("shader.hlsl"),
        "//#define FACTOR 2\nfloat factor = ${FACTOR};\n",
    )?;

    for kind in ["pixelshader", "computeshader"] {
        let open = "--[[";
        let close = "]]";
        let entry = entry_point(kind, "main");
        let src = format!(
            "{open}{kind}@main:\n\
             --#define ENABLED 1\n\
             --#if ${{ENABLED}} == 1\n\
             --#include \"shader.hlsl\"\n\
             --#else\n\
             --#include \"missing.hlsl\"\n\
             --#endif\n\
             --#undef ENABLED\n\
             {entry}\n\
             {close} local tail = 1\n\
             return tail\n"
        );
        let result = build_script(&src, &file, &[], &IndexMap::new(), false)?;
        assert_eq!(
            result,
            format!("{open}{kind}@main:\nfloat factor = 2;\n{entry}\n{close} local tail = 1\nreturn tail\n")
        );
    }

    let src =
        "--[[pixelshader@main:\n--#if 0\nskipped\n--#endif\nfloat4 main() : SV_Target { return 0; }\n]]\nreturn 1\n";
    assert_eq!(
        build_script(src, &file, &[], &IndexMap::new(), false)?,
        "--[[pixelshader@main:\nfloat4 main() : SV_Target { return 0; }\n]]\nreturn 1\n"
    );
    assert_eq!(
        build_script(
            "--[[computeshader@main:[numthreads(1, 1, 1)] void main() {}]]\nreturn 1\n",
            &file,
            &[],
            &IndexMap::new(),
            false
        )?,
        "--[[computeshader@main:[numthreads(1, 1, 1)] void main() {}]]\nreturn 1\n"
    );
    assert_eq!(
        build_script(
            "--#if 0\n--[[pixelshader@main:\n--#include \"missing.hlsl\"\n]]\n--#endif\nreturn 1\n",
            &file,
            &[],
            &IndexMap::new(),
            false,
        )?,
        "return 1\n"
    );

    Ok(())
}

#[test]
fn processes_only_level_zero_shader_comments_as_hlsl() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("main.lua");
    std::fs::write(
        dir.path().join("value.hlsl"),
        "/*#define VALUE\n2\n*/\nfloat value = ${VALUE};\n",
    )?;

    for kind in ["pixelshader", "computeshader"] {
        let entry = entry_point(kind, "main");

        for prefix in ["--", "//"] {
            let src = format!(
                "--[[{kind}@main:\n\
                 // Lua's quotes ' are HLSL comments\n\
                 {prefix}#include \"value.hlsl\"\n\
                 {entry}\n\
                 ]]\n\
                 local value = ${{VALUE}}\n"
            );
            let result = build_script(&src, &file, &[], &IndexMap::new(), false)?;
            assert!(result.contains("// Lua's quotes ' are HLSL comments"));
            assert!(result.contains("float value = 2;"));
            assert!(result.contains("local value = 2"));
            assert!(!result.contains("#include"));
        }

        let src = format!("--[[{kind}@main:\n/*#define VALUE\n3\n*/\n{entry}\n]]\nlocal value = ${{VALUE}}\n");
        let result = build_script(&src, &file, &[], &IndexMap::new(), false)?;
        assert!(result.contains("local value = 3"));
        assert!(!result.contains("/*#define"));

        let src = format!("--[[{kind}@main:\n--[=[#define VALUE\n4\n]=]\n{entry}\n]]\nlocal value = ${{VALUE}}\n");
        let result = build_script(&src, &file, &[], &IndexMap::new(), false)?;
        assert!(result.contains("local value = 4"));
        assert!(!result.contains("--[=[#define"));

        let src = format!(
            "--[[{kind}@main:\n// --#include \"missing.hlsl\"\n/*\n--#include \"missing.hlsl\"\n*/\n{entry}\n]]\n"
        );
        assert_eq!(build_script(&src, &file, &[], &IndexMap::new(), false)?, src);

        for src in [
            format!("--[=[{kind}@main:\n--#include \"missing.hlsl\"\n]=]\n"),
            format!("--[[{kind}@main\n--#include \"missing.hlsl\"\n]]\n"),
        ] {
            assert_eq!(build_script(&src, &file, &[], &IndexMap::new(), false)?, src);
        }
    }

    Ok(())
}

#[test]
fn shares_include_stack_between_lua_and_shader_blocks() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("main.hlsl");
    let src = "--#include \"child.lua\"\n";
    std::fs::write(&file, src)?;
    std::fs::write(
        dir.path().join("child.lua"),
        "--[[pixelshader@main:\n//#include \"main.hlsl\"\n]]\n",
    )?;
    let err = build_script(src, &file, &[], &IndexMap::new(), false).unwrap_err();
    assert!(err.to_string().contains("circular include detected"), "{err}");

    std::fs::write(dir.path().join("value.hlsl"), "float value;\n")?;
    std::fs::write(
        dir.path().join("child.lua"),
        "--[[pixelshader@main:\n\
         //#include \"value.hlsl\"\n\
         //#include \"value.hlsl\"\n\
         float4 main() : SV_Target { return 0; }\n\
         ]]\n",
    )?;
    let result = build_script(src, &file, &[], &IndexMap::new(), false)?;
    assert_eq!(result.matches("float value;").count(), 2);
    Ok(())
}

#[test]
fn preserves_ordinary_comments_and_strings_with_directives() -> anyhow::Result<()> {
    let src = "--[[ordinary comment\n--#include \"missing.hlsl\"\n]]\n\
               local text = [[pixelshader@main:\n--#include \"missing.hlsl\"\n]]\n";
    assert_eq!(
        build_script(src, Path::new("test.lua"), &[], &IndexMap::new(), false)?,
        src
    );
    Ok(())
}

#[test]
fn reports_original_positions_in_shader_blocks() -> anyhow::Result<()> {
    for kind in ["pixelshader", "computeshader"] {
        let src = format!("local value = 1\n--[[{kind}@main:\n  --#define VALUE ${{MISSING}}\n]]\n");
        let error = build_script(&src, Path::new("test.lua"), &[], &IndexMap::new(), false).unwrap_err();
        assert_eq!(error.to_string(), "test.lua:3:19: variable 'MISSING' not found");

        let src = format!("local value = 1\n--[[{kind}@main:\n--#if 1\nvalue\n]]\n");
        let error = build_script(&src, Path::new("test.lua"), &[], &IndexMap::new(), false).unwrap_err();
        assert_eq!(
            error.to_string(),
            "test.lua:3:3: unclosed conditional directive at end of file"
        );
    }

    Ok(())
}

#[test]
fn substitutes_conditional_placeholders_without_coercion() -> anyhow::Result<()> {
    let file = Path::new("test.lua");
    let vars = IndexMap::from([
        ("X".to_owned(), "aaaa".to_owned()),
        ("NUMBER".to_owned(), "123".to_owned()),
        ("KEY".to_owned(), "A".to_owned()),
        ("QUOTED".to_owned(), "\"A\"".to_owned()),
        ("PADDED".to_owned(), " A ".to_owned()),
        ("A".to_owned(), "unused".to_owned()),
    ]);

    for (expr, is_selected) in [
        (r#""${X}" == "aaaa""#, true),
        (r#""a${{ X }}b" == "aaaaab""#, true),
        (r#""${NUMBER}" == "123""#, true),
        ("${NUMBER} == 123", true),
        (r#""X" == "aaaa""#, false),
        ("defined(X)", true),
        (r#"defined("X")"#, false),
        ("defined(${KEY})", true),
        ("defined(${QUOTED})", false),
        ("defined(${PADDED})", false),
        ("defined(MISSING)", false),
    ] {
        let src = format!("--#if {expr}\nselected = true\n--#endif\n");
        let result = build_script(&src, file, &[], &vars, false)?;
        assert_eq!(result.contains("selected = true"), is_selected, "{expr}: {result}");
    }

    for expr in [r#"${X} == "aaaa""#, r#"X == "aaaa""#, "MISSING == 0", "${MISSING} == 0"] {
        let src = format!("--#if {expr}\nselected = true\n--#endif\n");
        assert!(build_script(&src, file, &[], &vars, false).is_err(), "{expr}");
    }

    let vars = IndexMap::from([
        ("\"A\"".to_owned(), "unused".to_owned()),
        ("X".to_owned(), "\"A\"".to_owned()),
    ]);
    let src = "--#if defined(${X}) and not defined(A)\n\
        selected = true\n\
        --#endif\n\
        --#ifdef \"A\"\n\
        quoted = true\n\
        --#endif\n\
        --#ifdef A\n\
        unquoted = true\n\
        --#endif\n";
    let result = build_script(src, file, &[], &vars, false)?;
    assert!(result.contains("selected = true"));
    assert!(result.contains("quoted = true"));
    assert!(!result.contains("unquoted = true"));
    Ok(())
}

#[test]
fn ignores_inactive_directives() -> anyhow::Result<()> {
    let src = r#"
--#ifdef DEBUG
--#define VAR 1
print("debug build")
--#else
--#define VAR 2
print("release build")
--#endif

--#if 0
local skipped = 1
bad.prop = 1 --track@_:0,100,1
--#define SKIPPED 1
--[[#define BLOCKED 1]]
--#undef VAR
--#include "missing.lua"
--#if (
nested = 1
--#endif
--#endif

--#ifdef SKIPPED
print("line define leaked")
--#endif
--#ifdef BLOCKED
print("block define leaked")
--#endif
--#ifdef VAR
print("var preserved")
--#endif

--#if ${VAR} > 1
print("hello")
--#endif
"#;
    let render = |is_debug| {
        if is_debug {
            build_script(
                src,
                Path::new("test.lua"),
                &[],
                &IndexMap::from([("DEBUG".to_owned(), "1".to_owned())]),
                false,
            )
        } else {
            build_script(src, Path::new("test.lua"), &[], &IndexMap::new(), false)
        }
    };

    let result = render(true)?;
    assert!(result.contains("print(\"debug build\")"));
    assert!(!result.contains("print(\"release build\")"));
    assert!(!result.contains("print(\"hello\")"));
    assert!(result.contains("print(\"var preserved\")"));
    assert!(!result.contains("skipped"));
    assert!(!result.contains("leaked"));

    let result = render(false)?;
    assert!(!result.contains("print(\"debug build\")"));
    assert!(result.contains("print(\"release build\")"));
    assert!(result.contains("print(\"hello\")"));
    assert!(result.contains("print(\"var preserved\")"));
    assert!(!result.contains("skipped"));
    assert!(!result.contains("leaked"));

    Ok(())
}

#[test]
fn resolves_includes_and_require_forms() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let include = dir.path().join("include");
    std::fs::create_dir_all(&include)?;
    std::fs::write(
        dir.path().join("relative.lua"),
        "--#define RELATIVE_VALUE relative\nrelative = true\n",
    )?;
    std::fs::write(include.join("quoted.txt"), "quoted = true\n")?;
    std::fs::write(include.join("system.txt"), "system = true\n")?;
    let escaped = if cfg!(windows) {
        include.join("escaped").join("quoted.txt")
    } else {
        include.join(r"escaped\quoted.txt")
    };

    if let Some(parent) = escaped.parent() {
        std::fs::create_dir_all(parent)?;
    }

    std::fs::write(&escaped, "escaped = true\n")?;

    let main = dir.path().join("main.lua");
    let result = build_script(
        r#"
--#include "relative.lua"
--#include "quoted.txt"
--#include "escaped\quoted.txt"
--#include <system.txt>
result = "${RELATIVE_VALUE}"
"#,
        &main,
        &[include],
        &IndexMap::new(),
        false,
    )?;

    for expected in [
        "relative = true",
        "quoted = true",
        "escaped = true",
        "system = true",
        "result = \"relative\"",
    ] {
        assert!(result.contains(expected), "missing {expected:?}\n{result}");
    }

    let module = dir.path().join("submod.lua");
    std::fs::write(&module, "local M = {}\nfunction M.greet() return 'hi' end\nreturn M\n")?;

    for req in [
        "local sub = require(\"submod\")",
        "local sub = require \"submod\"",
        "local sub = require 'submod'",
        "local sub = require [[submod]]",
        "local sub = require [=[submod]=]",
        "local sub = require [[\nsubmod]]",
        r#"local sub = require("sub\x6Dod")"#,
        r#"local sub = require("sub\u{6D}od")"#,
        r#"local sub = require("sub\109od")"#,
        r#"local sub = require("sub\z mod")"#,
        "local sub = require(\n    \"submod\"\n)",
        "local sub =\n    require(\"submod\")",
        "local sub = flag and require(\"submod\")",
        "require(\"submod\")",
    ] {
        let src = format!("local previous = require(\"other\")\n--#include \"submod.lua\"\n{req}\n");
        let result = build_script(&src, &main, &[], &IndexMap::new(), false)?;
        assert!(result.contains("local previous = require(\"other\")"), "{req}");
        assert!(result.contains("function M.greet()"), "{req}");
        assert!(result.contains("return M"), "{req}");
        assert!(result.contains("(function()"), "{req}");
        assert!(!result.contains(req), "{req}");
    }

    let pkg = dir.path().join("pkg");
    std::fs::create_dir_all(&pkg)?;
    std::fs::write(pkg.join("mod.lua"), "return true\n")?;
    std::fs::write(pkg.join("init.lua"), "return 'package'\n")?;

    for req in [
        r#"require("pkg.mod")"#,
        r#"require("pkg/mod")"#,
        r#"require("pkg\\mod")"#,
    ] {
        let result = build_script(
            &format!("--#include \"pkg/mod.lua\"\n{req}\n"),
            &main,
            &[],
            &IndexMap::new(),
            false,
        )?;
        assert!(result.contains("(function()"), "{req}");
        assert!(!result.contains(req), "{req}");
    }

    let result = build_script(
        "--#include \"pkg/init.lua\"\nrequire(\"pkg\")\n",
        &main,
        &[],
        &IndexMap::new(),
        false,
    )?;
    assert!(result.contains("return 'package'"));
    assert!(result.contains("(function()"));
    assert!(!result.contains("require(\"pkg\")"));

    let strange = dir.path().join("sub]=]mod.lua");
    std::fs::write(&strange, "return true\n")?;
    let result = build_script(
        "--#include \"sub]=]mod.lua\"\nrequire [==[sub]=]mod]==]\n",
        &main,
        &[],
        &IndexMap::new(),
        false,
    )?;
    assert!(result.contains("(function()"));
    assert!(!result.contains("require [==[sub]=]mod]==]"));

    for (src, message) in [
        (r#"--#include "missing.lua""#, "not found"),
        (
            "--#include \"actual.lua\"\nlocal value = require(\"different\")",
            "does not match included file",
        ),
    ] {
        if src.contains("actual.lua") {
            std::fs::write(dir.path().join("actual.lua"), "return true\n")?;
        }

        let error = build_script(src, &main, &[], &IndexMap::new(), false).unwrap_err();
        assert!(error.to_string().contains(message), "{src}: {error}");
    }

    let a = dir.path().join("a.lua");
    let b = dir.path().join("b.lua");
    std::fs::write(&a, "--#include \"b.lua\"\n")?;
    std::fs::write(&b, "--#include \"a.lua\"\n")?;
    let error = build_script(r#"--#include "b.lua""#, &a, &[], &IndexMap::new(), false).unwrap_err();
    assert!(error.to_string().contains("circular include detected"));
    Ok(())
}

#[test]
fn resolves_nested_requires_after_preprocessing() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let main = dir.path().join("main.lua");
    std::fs::write(dir.path().join("leaf.lua"), "return 7\n")?;
    std::fs::write(
        dir.path().join("parent.lua"),
        "local parent = true\n    --#include \"leaf.lua\"\n    local leaf = require('leaf')\nreturn leaf\n",
    )?;
    std::fs::write(dir.path().join("sibling.lua"), "return 9\n")?;
    let src = "local before = true\n  \
        --#include \"parent.lua\"\n\
        local parent = require('parent')\n\
        --#include \"sibling.lua\"\n\
        local sibling = require('sibling')\n\
        return parent, sibling\n";
    let result = build_script(src, &main, &[], &IndexMap::new(), false)?;
    assert!(
        result.contains(concat!(
            "local parent = (function()\n",
            "    local parent = true\n",
            "        local leaf = (function()\n",
            "            return 7\n",
            "        end)()\n",
            "    return leaf\n",
            "end)()"
        )),
        "{result}"
    );
    assert!(
        result.contains("local sibling = (function()\n    return 9\nend)()"),
        "{result}"
    );
    assert!(!result.contains("require("), "{result}");

    std::fs::write(
        dir.path().join("parent.lua"),
        "--#include \"leaf.lua\"\nlocal leaf = require('wrong')\n",
    )?;
    let err = build_script(src, &main, &[], &IndexMap::new(), false).unwrap_err();
    assert!(err.to_string().contains("parent.lua:2:14:"), "{err}");
    Ok(())
}

#[test]
fn resolves_independent_nested_requires() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let main = dir.path().join("main.lua");
    std::fs::write(dir.path().join("leaf.lua"), "return '日本語'\n")?;
    for name in ["first", "second", "third"] {
        std::fs::write(
            dir.path().join(format!("{name}.lua")),
            "--#include \"leaf.lua\"\nlocal value = require('leaf')\nreturn value\n",
        )?;
    }

    let src = "--#include \"first.lua\"\nlocal first = require('first')\n\
               --#include \"second.lua\"\nlocal second = require('second')\n\
               --#include \"third.lua\"\nlocal third = require('third')\nreturn first, second, third\n";
    let result = build_script(src, &main, &[], &IndexMap::new(), false)?;
    assert!(!result.contains("require("), "{result}");
    assert_eq!(result.matches("return '日本語'").count(), 3);
    for name in ["first", "second", "third"] {
        assert!(result.contains(&format!("local {name} = (function()")), "{result}");
    }

    std::fs::write(
        dir.path().join("second.lua"),
        "--#include \"leaf.lua\"\nlocal value = require('wrong')\nreturn value\n",
    )?;
    let err = build_script(src, &main, &[], &IndexMap::new(), false).unwrap_err();
    assert!(err.to_string().contains("second.lua:2:15:"), "{err}");
    Ok(())
}

#[test]
fn preserves_origins_through_empty_includes_and_require_rewrites() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let main = dir.path().join("main.anm2");
    std::fs::write(dir.path().join("empty.lua"), "")?;
    std::fs::write(dir.path().join("leaf.lua"), "return ${VALUE}\n")?;
    let vars = IndexMap::from([("VALUE".to_owned(), "7\n-- note".to_owned())]);
    for is_bundled in [false, true] {
        for src in ["", " \n\t\n", "--#include \"empty.lua\"\n"] {
            let result = build_script(src, &main, &[], &vars, is_bundled)?;
            assert!(result.is_empty(), "{result}");
        }

        for src in [
            "--#include \"empty.lua\"\nlocal empty = require('empty')\nreturn empty\n",
            "--#include \"empty.lua\"\nlocal empty = require(\n  'empty'\n)\nreturn empty\n",
            "--#include \"empty.lua\"\n--#include \"empty.lua\"\nreturn 1\n",
            "--#include \"leaf.lua\"\nlocal leaf = require('leaf')\nreturn leaf\n",
        ] {
            let result = build_script(src, &main, &[], &vars, is_bundled)?;
            assert!(!result.contains("require("), "{result}");
            assert!(result.contains("return"), "{result}");
        }

        let err = build_script(
            "--#include \"empty.lua\"\n   --[[pixelshader@float:float4 float() : SV_Target { return 0; }]]\n",
            &main,
            &[],
            &vars,
            is_bundled,
        )
        .unwrap_err();
        assert!(err.to_string().contains("main.anm2:2:4:"), "{err}");
    }

    Ok(())
}

#[test]
fn ignores_require_lookalikes() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let module = dir.path().join("submod.lua");
    std::fs::write(&module, "return true\n")?;
    let src = r#"
--#include "submod.lua"
local text = 'require("submod")'
local data = [[
require("submod")
]]
--[==[ require("submod") ]=] ]==]
object.require("submod")
object:require("submod")
-- require("submod")
require(value)
"#;
    let result = build_script(src, &dir.path().join("main.lua"), &[], &IndexMap::new(), false)?;

    assert!(result.contains("return true"));
    assert!(result.contains("local text = 'require(\"submod\")'"));
    assert!(result.contains("object.require(\"submod\")"));
    assert!(result.contains("object:require(\"submod\")"));
    assert!(result.contains("require(value)"));
    assert!(!result.contains("(function()"));
    Ok(())
}

#[test]
fn pairs_includes_with_the_next_statement_without_blank_lines() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let target = dir.path().join("main.lua");
    std::fs::write(dir.path().join("a.lua"), "return 'included'\n")?;

    for gap in [
        "",
        "-- note\n",
        "  -- first\n\t-- second\n",
        "--[[note]]\n",
        "--[[first\n\nlast]]\n",
    ] {
        let src = format!("local req = require\n--#include \"a.lua\"\n{gap}local a = req('a')\n");
        let result = build_script(&src, &target, &[], &IndexMap::new(), false)?;
        assert!(result.contains("local a = (function()"), "{gap:?}: {result}");
        assert!(!result.contains("req('a')"), "{gap:?}: {result}");
        assert!(result.contains(gap), "{gap:?}: {result}");
    }

    for gap in [
        "\n",
        " \t\n",
        "-- note\n\n",
        "\n-- note\n",
        "-- first\n \t\n-- second\n",
        "local req = require\n",
        "do_something()\n",
        "local unused = 1; ",
        "function helper() return req('a') end\n",
        "local helper = || -> req('a')\n",
    ] {
        let src = format!("local req = require\n--#include \"a.lua\"\n{gap}local a = req('a')\n");
        let result = build_script(&src, &target, &[], &IndexMap::new(), false)?;
        assert!(result.contains("local a = req('a')"), "{gap:?}: {result}");
        assert!(result.contains("return 'included'"), "{gap:?}: {result}");
        assert!(!result.contains("(function()"), "{gap:?}: {result}");
    }
    Ok(())
}

#[test]
fn resolves_require_references_before_includes() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    std::fs::write(dir.path().join("submod.lua"), "local require = other\nreturn true\n")?;

    for src in [
        "local require = other\n--#include \"submod.lua\"\nrequire('submod')\n",
        "require = other\n--#include \"submod.lua\"\nrequire('submod')\n",
        "local function require(name) return name end\n--#include \"submod.lua\"\nrequire('submod')\n",
        "function run(require)\n--#include \"submod.lua\"\nrequire('submod')\nend\n",
        "do local require = other\n--#include \"submod.lua\"\nrequire('submod')\nend\n",
    ] {
        let result = build_script(src, &dir.path().join("main.lua"), &[], &IndexMap::new(), false)?;
        assert!(result.contains("require('submod')"), "{src}: {result}");
        assert!(!result.contains("(function()"), "{src}: {result}");
    }

    let src = "local load = require\nwhile flag do\n--#include \"submod.lua\"\nload('submod')\nload = other\nend\n";
    let result = build_script(src, &dir.path().join("main.lua"), &[], &IndexMap::new(), false)?;
    assert!(result.contains("load('submod')"), "{src}: {result}");
    assert!(!result.contains("(function()"), "{src}: {result}");

    for src in [
        "--#include \"submod.lua\"\nrequire('submod')\n",
        "local load = require\n--#include \"submod.lua\"\nload('submod')\n",
        "local load = require\nrequire = other\n--#include \"submod.lua\"\nload('submod')\n",
        "do local require = other end\n--#include \"submod.lua\"\nrequire('submod')\n",
        "--#include \"submod.lua\"\nlocal require = require('submod')\n",
    ] {
        let result = build_script(src, &dir.path().join("main.lua"), &[], &IndexMap::new(), false)?;
        assert!(result.contains("(function()"), "{src}: {result}");
        assert!(!result.contains("require('submod')"), "{src}: {result}");
        assert!(!result.contains("load('submod')"), "{src}: {result}");
    }
    Ok(())
}

#[test]
fn preserves_lua_scopes_across_includes() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let target = dir.path().join("main.lua");
    std::fs::write(dir.path().join("aliases.lua"), "local req = require\n")?;
    std::fs::write(dir.path().join("leaf.lua"), "return 'leaf'\n")?;
    std::fs::write(dir.path().join("shadow.lua"), "local req = other\nreturn true\n")?;
    std::fs::write(
        dir.path().join("nested.lua"),
        "local req = require\n--#include \"leaf.lua\"\nlocal leaf = req('leaf')\nreturn leaf\n",
    )?;
    std::fs::write(
        dir.path().join("captured.lua"),
        "--#include \"leaf.lua\"\nlocal leaf = req('leaf')\nreturn leaf\n",
    )?;

    for src in [
        "--#include \"aliases.lua\"\n--#include \"leaf.lua\"\nlocal leaf = req('leaf')\n",
        "--#include \"nested.lua\"\nlocal nested = require('nested')\n",
        "local req = require\n--#include \"captured.lua\"\nlocal captured = require('captured')\n",
    ] {
        let result = build_script(src, &target, &[], &IndexMap::new(), false)?;
        assert!(result.contains("return 'leaf'"), "{src}: {result}");
        assert!(result.contains("(function()"), "{src}: {result}");
        assert!(!result.contains("req('leaf')"), "{src}: {result}");
        assert!(!result.contains("require('nested')"), "{src}: {result}");
        assert!(!result.contains("require('captured')"), "{src}: {result}");
    }

    let src = "local req = require\n\
               --#include \"shadow.lua\"\n\
               local shadow = req('shadow')\n\
               --#include \"leaf.lua\"\n\
               local leaf = req('leaf')\n";
    let result = build_script(src, &target, &[], &IndexMap::new(), false)?;
    assert!(result.contains("local shadow = (function()"), "{result}");
    assert!(result.contains("local leaf = (function()"), "{result}");
    assert!(!result.contains("req('shadow')"), "{result}");
    assert!(!result.contains("req('leaf')"), "{result}");

    let src = "--#include \"aliases.lua\"\nlocal req = other\n--#include \"leaf.lua\"\nreq('leaf')\n";
    let result = build_script(src, &target, &[], &IndexMap::new(), false)?;
    assert!(result.contains("req('leaf')"));
    assert!(!result.contains("(function()"));

    let src = "--#include \"shadow.lua\"\nlocal shadow = require('shadow')\n--#include \"leaf.lua\"\nreq('leaf')\n";
    let result = build_script(src, &target, &[], &IndexMap::new(), false)?;
    assert!(result.contains("req('leaf')"));
    Ok(())
}

#[test]
fn reject_include_depth() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;

    for suffix in ["lua", "hlsl", "ini"] {
        let prefix = match suffix {
            "lua" => "--",
            "hlsl" => "//",
            _ => ";",
        };

        for index in 0..66 {
            std::fs::write(
                dir.path().join(format!("{index}.{suffix}")),
                format!("{prefix}#include \"{}.{suffix}\"\n", index + 1),
            )?;
        }

        let file = dir.path().join(format!("main.{suffix}"));
        let src = format!("{prefix}#include \"0.{suffix}\"\n");
        let target: BuildTarget = serde_json::from_value(serde_json::json!({ "path": file.to_string_lossy() }))?;
        let err = match suffix {
            "lua" => build_script(&src, &file, &[], &IndexMap::new(), false).map(|_| ()),
            "hlsl" => astra::build::shader::build(&src, &file, &target, &[], &IndexMap::new()).map(|_| ()),
            _ => astra::build::ini::build(&src, &target, &[], &IndexMap::new()).map(|_| ()),
        }
        .unwrap_err();

        assert!(
            err.to_string().contains("include nesting exceeds 64 files"),
            "{suffix}: {err}"
        );
        assert!(err.to_string().contains(":1:"), "{suffix}: {err}");
    }

    Ok(())
}

#[test]
fn rejects_invalid_lua_tokens() -> anyhow::Result<()> {
    for (src, message) in [
        ("--[[aaaaa", "unfinished long comment"),
        ("--[=[aaaaa]]", "unfinished long comment"),
        ("[[aaaaa", "unfinished long string"),
        ("[=[aaaaa]]", "unfinished long string"),
        ("[=aaaaa", "invalid long string delimiter"),
        ("\"aaaaa", "unfinished string"),
        (r#"require("\q")"#, "invalid string escape"),
        (r#"require("\xQ0")"#, "invalid string escape"),
        (r#"require("\u{}")"#, "invalid string escape"),
    ] {
        let mut lexer = lua::Lexer::new(src);
        let err = lexer.by_ref().collect::<Result<Vec<_>, _>>().unwrap_err();
        assert_eq!(err.message, message, "{src}");
        assert_eq!(err.to_string(), message);
        assert!(lexer.next().is_none());

        let src = format!("local a = 1\n{src}");
        let err = build_script(&src, Path::new("test.lua"), &[], &IndexMap::new(), false).unwrap_err();
        assert!(err.to_string().contains("test.lua:2:"), "{src}: {err}");
        assert!(err.to_string().contains(message), "{src}: {err}");
    }

    let err = lua::Lexer::new("local a = 1\nrequire [[aaaaa")
        .find_map(Result::err)
        .unwrap();
    assert_eq!(err.offset, 20);
    Ok(())
}

#[test]
fn reports_syntax_depth_errors_with_original_positions() {
    let src = format!(
        "local before = 1\nlocal value = {}1{}",
        "(".repeat(500),
        ")".repeat(500)
    );
    let error = build_script(&src, Path::new("test.lua"), &[], &IndexMap::new(), false).unwrap_err();
    assert!(error.to_string().contains("test.lua:2:"), "{error}");
    assert!(
        error.to_string().contains("chunk has too many syntax levels"),
        "{error}"
    );
}

#[test]
fn normalizes_only_complete_assignments_before_real_property_comments() -> anyhow::Result<()> {
    let vars = IndexMap::new();
    for (src, expected) in [
        (
            "local x = \"--file@x:fake\" --string@x:Label,_",
            "--string@x:Label,--file@x:fake\n",
        ),
        ("; local x = 1 --value@x:Label,_", "--value@x:Label,1\n"),
        ("local x = 1; --value@_:Label,_", "--value@x:Label,1\n"),
        (";; local x = 1 ;; --value@x:Label,_", "--value@x:Label,1\n"),
        ("local x; --value@_:Label,1", "--value@x:Label,1\n"),
        ("local x = [[\ntext\n]] --text@x:Label,_", "--text@x:Label,text\\n\n"),
        (
            "--[[\nlocal x = 1 --track@x:Label,0,10,_\n]]\nreturn 1",
            "--[[\nlocal x = 1 --track@x:Label,0,10,_\n]]\nreturn 1\n",
        ),
    ] {
        assert_eq!(build_script(src, Path::new("test.anm2"), &[], &vars, false)?, expected);
    }

    for src in [
        "local x = 1; notify() --value@x:Label,_",
        "if true then\nx = 1 end --value@x:Label,_",
        "local x, y = 1, 2 --value@x:Label,_",
        "--[[\nlocal x = 1 ]] --value@x:Label,_",
        "local x = 1 --[[ annotation ]] --value@x:Label,_",
    ] {
        let err = build_script(src, Path::new("test.anm2"), &[], &vars, false).unwrap_err();
        assert!(
            err.to_string()
                .contains("must follow a single variable declaration or assignment"),
            "{src}: {err}"
        );
    }

    for src in [
        "obj.value = 1; --value@_:Label,_",
        "obj['value'] = 1; --value@_:Label,_",
        "obj[index()].value = 1; --value@_:Label,_",
    ] {
        let err = build_script(src, Path::new("test.anm2"), &[], &vars, false).unwrap_err();
        assert!(err.to_string().contains("table field assignment"), "{src}: {err}");
    }

    let err = build_script(
        "local x = 1 --trak@x:Label,0,10,1",
        Path::new("test.anm2"),
        &[],
        &vars,
        false,
    )
    .unwrap_err();
    assert!(err.to_string().contains("unsupported property 'trak'"));
    Ok(())
}

#[test]
#[allow(clippy::too_many_lines)]
fn validates_only_calls_resolved_to_shader_apis() -> anyhow::Result<()> {
    for kind in ["pixelshader", "computeshader"] {
        let block = format!("--[[{kind}@main:\n{}\n]]\n", entry_point(kind, "main"));
        for body in [
            format!("obj.{kind}(\"main\")"),
            format!("obj?.{kind}(\"main\")"),
            format!("obj[\"{kind}\"](\"main\")"),
            format!("local {kind} = obj.{kind}; {kind}(\"main\")"),
            format!("local api = obj.{kind}; local alias = api; alias(\"main\")"),
            format!("local object = obj; object.{kind}(\"main\")"),
            format!("local api = obj.{kind}; local function invoke() api(\"main\") end"),
        ] {
            let src = format!("{block}{body}");
            assert_eq!(
                build_script(&src, Path::new("test.anm2"), &[], &IndexMap::new(), false)?,
                format!("{src}\n")
            );
        }

        for body in [
            format!("obj.{kind}(\"missing\")"),
            format!("local api = obj.{kind}; api(\"missing\")"),
            format!("local api = obj.{kind}; local function invoke() api(\"missing\") end"),
            format!("do local api = obj.{kind}; api(\"missing\") end"),
            format!("local first, second = obj.{kind}, nil; first(\"missing\")"),
            format!("local api = other; api = obj.{kind}; api(\"missing\")"),
            format!("repeat local api = obj.{kind} until api(\"missing\")"),
        ] {
            let src = format!("{block}{body}");
            let err = build_script(&src, Path::new("test.anm2"), &[], &IndexMap::new(), false).unwrap_err();
            assert!(
                err.to_string().contains(&format!("undefined {kind} 'missing'")),
                "{body}: {err}"
            );
        }

        for body in [
            format!("local function {kind}(name) return name end; {kind}(\"missing\")"),
            format!("local {kind} = function(name) return name end; {kind}(\"missing\")"),
            format!("function helper({kind}) {kind}(\"missing\") end"),
            format!("function {kind}(name) return name end; {kind}(\"missing\")"),
            format!("function helper(obj) obj.{kind}(\"missing\") end"),
            format!("local api = obj.{kind}; do local api = other; api(\"missing\") end; api(\"main\")"),
            format!("do local api = obj.{kind} end; api(\"missing\")"),
            format!("local api = obj.{kind}; api = other; api(\"missing\")"),
            format!("local api = obj.{kind}; if flag then api = other end; api(\"missing\")"),
            format!("local api = obj.{kind}; local function invoke() api(\"missing\") end; api = other"),
            format!(
                "local api = obj.{kind}; local function invoke() local alias = api; alias(\"missing\") end; api = other"
            ),
            format!("for obj in objects do obj.{kind}(\"missing\") end"),
            format!("local api = obj.{kind}; api += other; api(\"missing\")"),
            format!("const api = obj.{kind}; local invoke = api -> api(\"missing\")"),
            format!("local api = obj.{kind}; local invoke = |api| -> api(\"missing\")"),
            format!(
                "local api = obj.{kind}; if flag then api = other \
                 elseif condition then api = other end; api(\"missing\")"
            ),
        ] {
            let src = format!("{block}{body}");
            assert_eq!(
                build_script(&src, Path::new("test.anm2"), &[], &IndexMap::new(), false)?,
                format!("{src}\n"),
                "{body}"
            );
        }

        for argument in ["name", "\"main\" .. suffix", "{}", ""] {
            let src = format!("{block}local api = obj.{kind}; api({argument})");
            let err = build_script(&src, Path::new("test.anm2"), &[], &IndexMap::new(), false).unwrap_err();
            assert!(err.to_string().contains("first argument"), "{argument}: {err}");
        }
    }
    Ok(())
}

#[test]
fn rejects_non_finite_tra_parameter_defaults() -> anyhow::Result<()> {
    for (suffix, prefix) in [(".tra", "--param:"), (".tra2", "--param:Label,")] {
        let target: BuildTarget = serde_json::from_value(serde_json::json!({"path": format!("test{suffix}")}))?;
        for value in ["NaN", "inf", "-inf", "1e999"] {
            let err = script::build(
                &format!("{prefix}{value}"),
                &target,
                &[],
                &IndexMap::new(),
                false,
                suffix,
            )
            .unwrap_err();
            assert!(err.to_string().contains("must be a number"), "{suffix}: {value}: {err}");
        }
        for value in ["0", "-1.5", "1e3"] {
            script::build(
                &format!("{prefix}{value}"),
                &target,
                &[],
                &IndexMap::new(),
                false,
                suffix,
            )?;
        }
    }
    Ok(())
}

#[test]
fn reports_lua_error_positions_through_includes() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("included.lua");
    let src = "local 名 = 1\n  local a = [=[unfinished";
    std::fs::write(&file, src)?;

    let err = lua::Lexer::new(src).collect::<Result<Vec<_>, _>>().unwrap_err();
    assert_eq!(err.offset, src.find("[=[").unwrap());

    let err = build_script(
        "--#include \"included.lua\"\n",
        &dir.path().join("main.lua"),
        &[],
        &IndexMap::new(),
        false,
    )
    .unwrap_err();
    assert!(
        err.to_string().contains("included.lua:2:13: unfinished long string"),
        "{err}"
    );
    Ok(())
}

#[test]
fn reports_decoded_require_strings() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    std::fs::write(dir.path().join("submod.lua"), "return true\n")?;

    let req = r#"require("\a\b\f\n\r\t\v\\\"\'\x41\u{1F600}\z   \065")"#;
    let decoded = "\x07\x08\x0C\n\r\t\x0B\\\"\x27A\u{1F600}A";
    let err = build_script(
        &format!("--#include \"submod.lua\"\n{req}\n"),
        &dir.path().join("main.lua"),
        &[],
        &IndexMap::new(),
        false,
    )
    .unwrap_err();
    assert!(
        err.to_string().contains(&format!("require module '{decoded}'")),
        "{err}"
    );

    for (req, decoded) in [
        ("require(\"sub\\\nmod\")", "sub\nmod"),
        ("require [[sub\nmod]]", "sub\nmod"),
    ] {
        let err = build_script(
            &format!("--#include \"submod.lua\"\n{req}\n"),
            &dir.path().join("main.lua"),
            &[],
            &IndexMap::new(),
            false,
        )
        .unwrap_err();
        assert!(
            err.to_string().contains(&format!("require module '{decoded}'")),
            "{req}: {err}"
        );
    }

    Ok(())
}

#[test]
fn errors_on_undefined_variable_in_script_ini_and_shader() -> anyhow::Result<()> {
    let target: BuildTarget = serde_json::from_value(serde_json::json!({
        "path": "test.anm2",
    }))?;

    for (src, pos) in [
        ("x = \"${UNDEFINED}\"", "1:6"),
        ("\n  x = ${UNDEFINED} -- note", "2:7"),
        ("text = \"あ${UNDEFINED}\"", "1:10"),
        ("-- ${UNDEFINED}", "1:4"),
        ("--#define VALUE ${UNDEFINED}", "1:17"),
        ("--[[#define VALUE\n    ${UNDEFINED}\n]]", "2:5"),
        ("--#include \"${UNDEFINED}.lua\"", "1:13"),
        ("--#include <${UNDEFINED}.lua>", "1:13"),
        (r#"--#include "dir\\${UNDEFINED}.lua""#, "1:18"),
    ] {
        let err = script::build(src, &target, &[], &IndexMap::new(), false, ".anm2").unwrap_err();
        assert_eq!(
            err.to_string(),
            format!("test.anm2:{pos}: variable 'UNDEFINED' not found"),
            "{src}"
        );
    }

    let ini_target: BuildTarget = serde_json::from_value(serde_json::json!({
        "path": "test.ini",
    }))?;

    for (src, pos) in [
        ("val = ${{ MISSING }}", "1:7"),
        ("\n項目=${MISSING}", "2:4"),
        (";#define VALUE ${MISSING}", "1:16"),
        (";#include \"${MISSING}.ini\"", "1:12"),
        (r#";#include "dir\\${MISSING}.ini""#, "1:17"),
    ] {
        let err = astra::build::ini::build(src, &ini_target, &[], &IndexMap::new()).unwrap_err();
        assert_eq!(
            err.to_string(),
            format!("test.ini:{pos}: variable 'MISSING' not found"),
            "{src}"
        );
    }

    let shader_target: BuildTarget = serde_json::from_value(serde_json::json!({
        "path": "test.hlsl",
    }))?;

    for (src, pos) in [
        ("float x = ${NOT_DEFINED};", "1:11"),
        ("\n  float x = ${NOT_DEFINED};", "2:13"),
        ("//#define VALUE ${NOT_DEFINED}", "1:17"),
        ("//#include \"${NOT_DEFINED}.hlsl\"", "1:13"),
        (r#"//#include "dir\\${NOT_DEFINED}.hlsl""#, "1:18"),
    ] {
        let err = astra::build::shader::build(src, Path::new("test.hlsl"), &shader_target, &[], &IndexMap::new())
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            format!("test.hlsl:{pos}: variable 'NOT_DEFINED' not found"),
            "{src}"
        );
    }

    Ok(())
}

#[test]
fn reports_byte_offset_errors_as_src_positions() -> anyhow::Result<()> {
    let vars = IndexMap::new();
    for (src, pos) in [
        ("\nlocal 名 = \"${MISSING}\"", "2:12"),
        ("local a = [[あ${MISSING}]]", "1:14"),
        ("--#define X あ${MISSING}", "1:14"),
        ("--#include \"あ${MISSING}.lua\"", "1:14"),
        ("--#if \"あ\" == ${MISSING}\n--#endif", "1:14"),
        ("--#ifdef ${MISSING}\n--#endif", "1:10"),
        (
            "\n--[[pixelshader@main:\n// あ${MISSING}\nfloat4 main() : SV_Target { return 0; }\n]]",
            "3:5",
        ),
    ] {
        let err = build_script(src, Path::new("test.anm2"), &[], &vars, false).unwrap_err();
        assert_eq!(
            err.to_string(),
            format!("test.anm2:{pos}: variable 'MISSING' not found"),
            "{src}"
        );
    }

    let vars = IndexMap::from([("X".to_owned(), "42".to_owned())]);
    let err = build_script("--#if ${X} == \"unfinished", Path::new("test.anm2"), &[], &vars, false).unwrap_err();
    assert_eq!(err.to_string(), "test.anm2:1:15: unclosed string");
    let vars = IndexMap::new();
    let target: BuildTarget = serde_json::from_value(serde_json::json!({ "path": "test.ini" }))?;
    let err = astra::build::ini::build("\n項目=${MISSING}", &target, &[], &vars).unwrap_err();
    assert_eq!(err.to_string(), "test.ini:2:4: variable 'MISSING' not found");
    let target: BuildTarget = serde_json::from_value(serde_json::json!({ "path": "test.hlsl" }))?;
    let err =
        astra::build::shader::build("\n// あ${MISSING}", Path::new("test.hlsl"), &target, &[], &vars).unwrap_err();
    assert_eq!(err.to_string(), "test.hlsl:2:5: variable 'MISSING' not found");
    Ok(())
}

#[test]
fn reports_include_cycle_positions() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let vars = IndexMap::new();
    for (name, directive, col) in [
        ("loop.lua", "--#include \"loop.lua\"", 3),
        ("loop.ini", ";#include \"loop.ini\"", 2),
        ("loop.hlsl", "//#include \"loop.hlsl\"", 3),
    ] {
        let file = dir.path().join(name);
        std::fs::write(&file, format!("\n{directive}\n"))?;
        let root = dir
            .path()
            .join(format!("main.{ext}", ext = name.rsplit('.').next().unwrap()));
        let target: BuildTarget = serde_json::from_value(serde_json::json!({ "path": root }))?;
        let err = match name.rsplit('.').next().unwrap() {
            "lua" => build_script(directive, &root, &[], &vars, false).unwrap_err(),
            "ini" => astra::build::ini::build(directive, &target, &[], &vars).unwrap_err(),
            _ => astra::build::shader::build(directive, &root, &target, &[], &vars).unwrap_err(),
        };
        assert_eq!(
            err.to_string(),
            format!(
                "{}:2:{col}: circular include detected: '{}'",
                file.display(),
                file.display()
            ),
        );
    }

    Ok(())
}

#[test]
fn preserves_escaped_placeholders_in_script_ini_and_shader() -> anyhow::Result<()> {
    let vars = IndexMap::from([("X".to_owned(), "42".to_owned())]);
    let src = r#"local a = "$${{MISSING}}"
local b = [=[$${MISSING}]=]
-- $${{MISSING}} ${X}
--#define LITERAL $${{MISSING}}
local c = "${LITERAL}"
--#if "$${X}" == "$${X}"
local d = ${X}
--#endif
"#;
    let expected = r#"local a = "${{MISSING}}"
local b = [=[${MISSING}]=]
-- ${{MISSING}} 42
local c = "${{MISSING}}"
local d = 42
"#;
    assert_eq!(build_script(src, Path::new("test.anm2"), &[], &vars, false)?, expected);
    assert!(lua::Lexer::new(r#""$${X}""#).next().unwrap().is_ok());

    let target: BuildTarget = serde_json::from_value(serde_json::json!({ "path": "test.ini" }))?;
    let src = r#"a=$${{MISSING}}
b=$${MISSING}
c=${X}
d=\${X}
e=$$
f=$${}
g=$$${X}
"#;
    assert_eq!(
        astra::build::ini::build(src, &target, &[], &vars)?,
        "a=${{MISSING}}\nb=${MISSING}\nc=42\nd=\\42\ne=$$\nf=${}\ng=$${X}\n"
    );

    let target: BuildTarget = serde_json::from_value(serde_json::json!({ "path": "test.hlsl" }))?;
    let src = "// $${{MISSING}} $${MISSING}\nfloat x = ${X};\n";
    let output = astra::build::shader::build(src, Path::new("test.hlsl"), &target, &[], &vars)?;
    assert_eq!(output.text, "// ${{MISSING}} ${MISSING}\nfloat x = 42;\n");
    Ok(())
}

#[test]
fn preserves_escaped_placeholders_in_include_paths() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    std::fs::write(dir.path().join("${{MISSING}}.lua"), "return 42\n")?;
    let output = build_script(
        r#"--#include "$${{MISSING}}.lua""#,
        &dir.path().join("main.lua"),
        &[],
        &IndexMap::new(),
        false,
    )?;
    assert_eq!(output, "return 42\n");
    Ok(())
}

#[test]
fn errors_on_invalid_or_unclosed_placeholders_in_script_ini_and_shader() -> anyhow::Result<()> {
    let target: BuildTarget = serde_json::from_value(serde_json::json!({
        "path": "test.anm2",
    }))?;

    for (src, expected) in [
        ("--${{}}", "empty variable name in placeholder"),
        ("--${}", "empty variable name in placeholder"),
        ("-- ${{   }}", "empty variable name in placeholder"),
        ("--${{ 123 }}", "invalid variable name '123' in placeholder"),
        ("--${{ NAME", "unclosed placeholder"),
        ("--${ NAME", "unclosed placeholder"),
        ("x = \"${{}}\"", "empty variable name in placeholder"),
        ("x = ${{}}", "empty variable name in placeholder"),
    ] {
        let err = script::build(src, &target, &[], &IndexMap::new(), false, ".anm2").unwrap_err();
        assert!(
            err.to_string().contains(expected),
            "expected '{expected}' for '{src}', got: {err}"
        );
    }

    let ini_target: BuildTarget = serde_json::from_value(serde_json::json!({
        "path": "test.ini",
    }))?;

    for (src, expected) in [
        ("val = ${{}}", "empty variable name in placeholder"),
        ("val = ${{ 123 }}", "invalid variable name '123' in placeholder"),
        ("val = ${{ NAME", "unclosed placeholder"),
    ] {
        let err = astra::build::ini::build(src, &ini_target, &[], &IndexMap::new()).unwrap_err();
        assert!(
            err.to_string().contains(expected),
            "expected '{expected}' for '{src}', got: {err}"
        );
    }

    let shader_target: BuildTarget = serde_json::from_value(serde_json::json!({
        "path": "test.hlsl",
    }))?;

    for (src, expected) in [
        ("float x = ${{}};", "empty variable name in placeholder"),
        ("float x = ${{ 123 }};", "invalid variable name '123' in placeholder"),
        ("float x = ${{ NAME;", "unclosed placeholder"),
    ] {
        let err = astra::build::shader::build(src, Path::new("test.hlsl"), &shader_target, &[], &IndexMap::new())
            .unwrap_err();
        assert!(
            err.to_string().contains(expected),
            "expected '{expected}' for '{src}', got: {err}"
        );
    }

    Ok(())
}

#[test]
fn warns_on_non_float_types_without_warning_on_padding() -> anyhow::Result<()> {
    let (bytes, _guard) = capture_warnings();
    build_shader(
        "--#define TYPE float4\ncbuffer Params {\n float a;\n ${TYPE} b;\n int c;\n float2x2 m;\n};\n",
        &[],
    )?;
    let logs = String::from_utf8(bytes.lock().unwrap().clone())?;
    assert!(
        logs.contains("test.anm2:6:2: cbuffer 'Params' uses non-float"),
        "{logs}"
    );
    assert!(!logs.contains("padding(number)"), "{logs}");
    assert_eq!(logs.lines().count(), 1, "{logs}");
    Ok(())
}

#[test]
fn reports_original_positions_for_included_cbuffers_and_pragmas() -> anyhow::Result<()> {
    let (bytes, _guard) = capture_logs(tracing::Level::INFO);
    let dir = tempfile::tempdir()?;
    let include = dir.path().join("members.hlsl");
    std::fs::write(&include, " float4 b;\n bool c;\n")?;
    let output = build_shader(
        "cbuffer Included { float a;\n--#include <members.hlsl>\n};",
        &[std::fs::canonicalize(dir.path())?],
    )?;
    assert!(output.contains(" bool c;"), "{output}");
    let warnings = String::from_utf8(bytes.lock().unwrap().clone())?;
    let canonical = std::fs::canonicalize(include)?;
    assert!(
        warnings.contains("test.anm2:2:1: cbuffer 'Included' layout"),
        "{warnings}"
    );
    assert!(
        warnings.contains(&format!("{}:2:2:", canonical.display())),
        "{warnings}"
    );
    assert_eq!(warnings.lines().count(), 2, "{warnings}");

    bytes.lock().unwrap().clear();
    std::fs::write(dir.path().join("packing.hlsl"), "#pragma pack_matrix(row_major)\n")?;
    let src = "#pragma pack_matrix(column_major)\n--#include <packing.hlsl>\ncbuffer IncludedPacking { float2x3 a; };";
    let output = build_shader(src, &[std::fs::canonicalize(dir.path())?])?;
    assert!(output.contains("#pragma pack_matrix(row_major)"), "{output}");
    let warnings = String::from_utf8(bytes.lock().unwrap().clone())?;
    assert!(
        warnings.contains("test.anm2:4:1: cbuffer 'IncludedPacking' layout"),
        "{warnings}"
    );
    assert_eq!(warnings.lines().count(), 1, "{warnings}");
    Ok(())
}

#[test]
fn reports_original_positions_through_nested_hlsl_includes() -> anyhow::Result<()> {
    let (bytes, _guard) = capture_logs(tracing::Level::INFO);
    let dir = tempfile::tempdir()?;
    std::fs::create_dir(dir.path().join("sub"))?;
    std::fs::write(
        dir.path().join("a.hlsl"),
        "//#define TYPE float4\n//#if false\nint ignored;\n//#endif\ncbuffer Outer { float a;\n\
         //#include \"sub/b.hlsl\"\n};\ncbuffer After { float a; float4 b; };\n",
    )?;
    std::fs::write(
        dir.path().join("sub/b.hlsl"),
        "//#pragma once\n ${TYPE} b;\n bool c;\n//#include \"c.hlsl\"\n",
    )?;
    std::fs::write(dir.path().join("sub/c.hlsl"), " float2x2 m;\n")?;

    let output = build_shader("    --#include <a.hlsl>", &[std::fs::canonicalize(dir.path())?])?;
    assert!(!output.contains("ignored"), "{output}");
    assert!(output.contains("float4 b;"), "{output}");
    let warnings = String::from_utf8(bytes.lock().unwrap().clone())?;

    for (file, pos, msg) in [
        ("a.hlsl", "5:1", "cbuffer 'Outer' layout"),
        ("sub/b.hlsl", "3:2", "cbuffer 'Outer' uses non-float"),
        ("a.hlsl", "8:1", "cbuffer 'After' layout"),
    ] {
        assert!(
            warnings.contains(&format!(
                "{}:{pos}: {msg}",
                std::fs::canonicalize(dir.path().join(file))?.display()
            )),
            "{warnings}"
        );
    }

    assert_eq!(warnings.lines().count(), 3, "{warnings}");
    Ok(())
}

#[test]
fn reports_placeholder_positions_for_multiline_hlsl_values() -> anyhow::Result<()> {
    let (bytes, _guard) = capture_warnings();
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("values.hlsl");
    std::fs::write(&file, "cbuffer Values { float a; ${MEMBERS} float4 tail; };\n")?;
    let output = build_script(
        "--[[pixelshader@main:\n--#include <values.hlsl>\nfloat4 main() : SV_Target { return 0; }\n]]\n",
        Path::new("test.anm2"),
        &[std::fs::canonicalize(dir.path())?],
        &IndexMap::from([("MEMBERS".to_owned(), "float4 b;\nbool c;".to_owned())]),
        false,
    )?;
    assert!(output.contains("float4 b;\nbool c; float4 tail;"), "{output}");
    let warnings = String::from_utf8(bytes.lock().unwrap().clone())?;
    let file = std::fs::canonicalize(file)?;
    assert!(
        warnings.contains(&format!("{}:1:27: cbuffer 'Values' uses non-float", file.display())),
        "{warnings}"
    );

    assert_eq!(warnings.lines().count(), 1, "{warnings}");
    Ok(())
}

#[test]
fn reports_placeholder_positions_for_multiline_lua_values() -> anyhow::Result<()> {
    let (bytes, _guard) = capture_warnings();
    let output = build_script(
        "--[[pixelshader@main:\ncbuffer Values { float a; ${MEMBERS} float4 tail; };\n\
         float4 main() : SV_Target { return 0; }\n]]\n",
        Path::new("test.anm2"),
        &[],
        &IndexMap::from([("MEMBERS".to_owned(), "float4 b;\nbool c;".to_owned())]),
        false,
    )?;
    assert!(output.contains("float4 b;\nbool c; float4 tail;"), "{output}");
    let warnings = String::from_utf8(bytes.lock().unwrap().clone())?;

    assert!(
        warnings.contains("test.anm2:2:27: cbuffer 'Values' uses non-float"),
        "{warnings}"
    );

    assert_eq!(warnings.lines().count(), 1, "{warnings}");
    Ok(())
}
