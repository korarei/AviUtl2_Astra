use astra::build::script;
use astra::config::BuildTarget;
use indexmap::IndexMap;
use std::path::{Path, PathBuf};

fn build_script(
    src: &str,
    file: &Path,
    dirs: &[PathBuf],
    vars: &IndexMap<String, String>,
    bundled: bool,
) -> anyhow::Result<String> {
    let target: BuildTarget = serde_json::from_value(serde_json::json!({
        "path": file.to_string_lossy().into_owned(),
    }))?;
    script::build(src, &target, dirs, vars, bundled, ".anm2").map(|output| output.script)
}

#[test]
fn builds_directives_conditions_properties_and_headers() -> anyhow::Result<()> {
    let vars = IndexMap::from([
        ("PLATFORM".to_owned(), "win".to_owned()),
        ("TARGET".to_owned(), "client".to_owned()),
        ("DEBUG".to_owned(), "false".to_owned()),
        ("VERSION".to_owned(), "3".to_owned()),
        ("HEX".to_owned(), "0x10".to_owned()),
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
--#if LINE == 0
selected = "if"
--#elif LINE == 1 and PLATFORM == 'win' and not DEBUG
selected = "elif"
--#else
selected = "else"
--#endif
--#ifdef LINE
ifdef_selected = true
--#endif
--#ifdef ${{ LINE }}
ifdef_placeholder_selected = true
--#endif
--#ifdef "${{ LINE }}"
ifdef_quoted_placeholder_selected = true
--#endif
--#ifndef MISSING
ifndef_selected = true
--#endif
--#ifndef ${ MISSING }
ifndef_placeholder_selected = true
--#endif
--#ifndef "${{ NOT_DEFINED }}"
ifndef_quoted_placeholder_selected = true
--#endif
--#if defined(PLATFORM) && PLATFORM == 'win' && VERSION >= 3
platform_selected = true
--#endif
--#if HEX == 16
hex_selected = true
--#endif
--#if LINE ~= 0 or false
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
--#if MISSING == 0
missing_defaults_to_zero = true
--#endif
--#if "${{ PLATFORM }}" == 'win' && '${{ TARGET }}' == 'client'
quoted_selected = true
--#endif
--#if ${ PLATFORM } == 'win'
placeholder_selected = true
--#endif
--#if defined "${{ PLATFORM }}" and not defined(${ UNDEFINED })
defined_selected = true
--#endif -- trailing comment
--#if LINE == 1
--#if PLATFORM == 'win'
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
color = 0xffffff --color@color:col,0xffffff
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
ignored_at_space = 3 --track @ignored_at_space:0,100,1
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
        "missing_defaults_to_zero = true",
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
        "--color@color:col,0xffffff",
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
        "ignored_at_space = 3 --track @ignored_at_space:0,100,1",
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

    for (src, bundled, expected) in [
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
        ("local value = 1\n@another", true, "@test\nlocal value = 1\n@another\n"),
    ] {
        assert_eq!(
            build_script(src, Path::new("test.lua"), &[], &IndexMap::new(), bundled)?,
            expected
        );
    }
    for (src, bundled) in [("@custom\nlocal value = 1", false)] {
        assert!(build_script(src, Path::new("test.lua"), &[], &IndexMap::new(), bundled).is_err());
    }

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
            "'data' annotation cannot be attached to variable declaration",
        ),
        (
            "local value = 10 --hide@_:0",
            "'hide' annotation cannot be attached to variable declaration",
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
        ("--[[#define VALUE\nvalue", "unclosed block"),
        (
            "--#define BUILD_DIR overridden\n",
            "cannot define reserved variable 'BUILD_DIR'",
        ),
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

        for name in [
            "",
            "123main",
            "ps-main",
            "ps main",
            " main",
            "main ",
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
            "float4 main() : SV_Target {",
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
        ("pixelshader", "void main() {}"),
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

    for (kind, open, close) in [("pixelshader", "--[[", "]]"), ("computeshader", "--[=[", "]=]")] {
        let entry = entry_point(kind, "main");
        let src = format!(
            "{open}{kind}@main:\n\
             --#define ENABLED 1\n\
             --#if ENABLED == 1\n\
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

--#if VAR > 1
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
--#include "escaped\\quoted.txt"
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
require("\q")
require("\xQ0")
require("\u{}")
"#;
    let result = build_script(src, &dir.path().join("main.lua"), &[], &IndexMap::new(), false)?;

    assert!(result.contains("return true"));
    assert!(result.contains("local text = 'require(\"submod\")'"));
    assert!(result.contains("object.require(\"submod\")"));
    assert!(result.contains("object:require(\"submod\")"));
    assert!(result.contains("require(value)"));
    assert!(result.contains("require(\"\\q\")"));
    assert!(result.contains("require(\"\\xQ0\")"));
    assert!(result.contains("require(\"\\u{}\")"));
    assert!(!result.contains("(function()"));
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
fn resolves_pragma_once_in_ini_includes() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let header = dir.path().join("header.ini");
    let a = dir.path().join("a.ini");
    let main = dir.path().join("main.ini");

    std::fs::write(&header, ";#pragma once\n[common]\nkey = common_val\n")?;
    std::fs::write(&a, ";#include \"header.ini\"\n[a]\nkey = a_val\n")?;

    let src = ";#include \"a.ini\"\n;#include \"header.ini\"\n[main]\nkey = main_val\n";
    let target: BuildTarget = serde_json::from_value(serde_json::json!({
        "path": main.to_string_lossy().into_owned(),
    }))?;
    let result = astra::build::ini::build(src, &target, &[], &IndexMap::new())?;
    assert_eq!(result.matches("[common]").count(), 1, "{result}");
    assert!(result.contains("[a]"), "{result}");
    assert!(result.contains("[main]"), "{result}");

    // Trailing comment on pragma once
    let header_comment = dir.path().join("header_comment.ini");
    std::fs::write(
        &header_comment,
        ";#pragma once ; single inclusion\n[comment]\nval = 1\n",
    )?;
    let src = ";#include \"header_comment.ini\"\n;#include \"header_comment.ini\"\n";
    let result = astra::build::ini::build(src, &target, &[], &IndexMap::new())?;
    assert_eq!(result.matches("[comment]").count(), 1, "{result}");

    // Unknown pragma is treated as a normal comment and kept
    let header_unknown = dir.path().join("header_unknown.ini");
    std::fs::write(&header_unknown, ";#pragma message(\"hello\")\n[unknown]\nval = 1\n")?;
    let src = ";#include \"header_unknown.ini\"\n";
    let result = astra::build::ini::build(src, &target, &[], &IndexMap::new())?;
    assert!(result.contains(";#pragma message(\"hello\")"), "{result}");

    // pragma once inside #if 0 is ignored
    let header_if0 = dir.path().join("header_if0.ini");
    std::fs::write(&header_if0, ";#if 0\n;#pragma once\n;#endif\n[if0]\nval = 1\n")?;
    let src = ";#include \"header_if0.ini\"\n;#include \"header_if0.ini\"\n";
    let result = astra::build::ini::build(src, &target, &[], &IndexMap::new())?;
    assert_eq!(result.matches("[if0]").count(), 2, "{result}");

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
        (r#"--#include "dir\\${UNDEFINED}.lua""#, "1:3"),
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
        (r#";#include "dir\\${MISSING}.ini""#, "1:2"),
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
        (r#"//#include "dir\\${NOT_DEFINED}.hlsl""#, "1:3"),
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
