use astra::build::hlsl::{self, Lexer, State, TokenKind};
use astra::config::BuildTarget;
use indexmap::IndexMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

// ============================================================================
// Test Helpers
// ============================================================================

#[derive(Clone)]
struct TestLogWriter(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for TestLogWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().write(bytes)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn capture_warnings() -> (Arc<Mutex<Vec<u8>>>, tracing::subscriber::DefaultGuard) {
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let writer = TestLogWriter(Arc::clone(&bytes));
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_max_level(tracing::Level::WARN)
        .with_writer(move || writer.clone())
        .finish();
    let guard = tracing::subscriber::set_default(subscriber);
    (bytes, guard)
}

// ============================================================================
// Lexer Tests
// ============================================================================

#[test]
fn distinguishes_hlsl_directives_from_comments_and_literals() {
    let src = "//#define VALUE 1\n/* #if FLAG */\n\"#pragma pack_matrix(row_major)\"\nfloat x; #endif\n #pragma pack_matrix(column_major)\n";
    let tokens = Lexer::new(src).collect::<Vec<_>>();
    assert_eq!(
        tokens
            .iter()
            .filter(|token| token.kind == TokenKind::Directive)
            .map(|token| token.text)
            .collect::<Vec<_>>(),
        ["#pragma pack_matrix(column_major)"]
    );
    assert!(tokens.iter().any(|token| {
        matches!(token.kind, TokenKind::Comment { is_block: false }) && token.text == "//#define VALUE 1"
    }));
    assert!(
        tokens
            .iter()
            .any(|token| { token.kind == TokenKind::String && token.text == "\"#pragma pack_matrix(row_major)\"" })
    );
    assert!(tokens.iter().any(|token| token.kind == TokenKind::Other('#')));
}

#[test]
fn preserves_comments_and_directive_continuations_across_chunks() {
    let mut state = State::default();
    let mut tokens = Vec::new();
    for src in [
        "#define MACRO \\",
        "cbuffer Hidden { int x; }; \\",
        "// continued",
        "/*",
        "#pragma pack_matrix(row_major)",
        "*/ cbuffer Visible { float4 x; };",
    ] {
        let mut lexer = Lexer::new(src);
        lexer.state = state;
        tokens.extend(lexer.by_ref());
        state = lexer.state;
    }
    assert_eq!(
        tokens.iter().filter(|token| token.kind == TokenKind::Directive).count(),
        1
    );
    assert_eq!(
        tokens
            .iter()
            .filter(|token| token.kind == TokenKind::Continuation)
            .count(),
        2
    );
    assert_eq!(
        tokens
            .iter()
            .filter(|token| token.kind == TokenKind::Ident && token.text == "cbuffer")
            .count(),
        1
    );
    assert!(
        tokens
            .iter()
            .any(|token| token.kind == TokenKind::Ident && token.text == "Visible")
    );
    assert!(!state.is_comment);
    assert!(!state.is_directive);
}

#[test]
fn ends_directive_continuation_on_an_empty_chunk() {
    let mut state = State::default();
    let mut tokens = Vec::new();
    for src in ["#define MACRO \\", "", "cbuffer Visible { float4 x; };"] {
        let mut lexer = Lexer::new(src);
        lexer.state = state;
        tokens.extend(lexer.by_ref());
        state = lexer.state;
    }
    assert!(!tokens.iter().any(|token| token.kind == TokenKind::Continuation));
    assert!(
        tokens
            .iter()
            .any(|token| token.kind == TokenKind::Ident && token.text == "cbuffer")
    );
}

#[test]
fn preserves_source_spans_for_unicode_and_escaped_strings() {
    let src = "cbuffer 色 { float4 値; }; \"日本語\\\"/* #if */\" // コメント\n0xff 1.5e-2 .5f";
    let tokens = Lexer::new(src).collect::<Vec<_>>();
    for token in &tokens {
        assert!(src.is_char_boundary(token.span.st));
        assert!(src.is_char_boundary(token.span.ed));
        assert_eq!(token.text, &src[token.span.st..token.span.ed]);
    }
    assert!(
        tokens
            .iter()
            .any(|token| token.kind == TokenKind::Ident && token.text == "値")
    );
    assert_eq!(
        tokens
            .iter()
            .filter(|token| token.kind == TokenKind::Number)
            .map(|token| token.text)
            .collect::<Vec<_>>(),
        ["0xff", "1.5e-2", ".5f"]
    );
    assert!(!tokens.iter().any(|token| token.kind == TokenKind::Directive));
}

// ============================================================================
// Declaration & Directive Parser Tests
// ============================================================================

#[test]
fn parses_types_and_declarations_with_source_spans() {
    let src = "row_major matrix<float, 2, 3> m[2], n";
    let tokens = Lexer::new(src)
        .filter(|token| {
            !matches!(
                token.kind,
                TokenKind::Whitespace | TokenKind::Newline | TokenKind::Comment { .. }
            )
        })
        .collect::<Vec<_>>();
    let declaration = hlsl::parse_declaration(&tokens).unwrap();
    assert_eq!(
        declaration.ty,
        Some(hlsl::Type {
            scalar: "float",
            rows: 2,
            cols: 3,
            is_matrix: true
        })
    );
    assert_eq!(declaration.is_row_major, Some(true));
    assert!(!declaration.has_error);
    assert_eq!(declaration.members.len(), 2);
    assert_eq!(declaration.members[0].name, "m");
    assert_eq!(declaration.members[0].dimensions, [Some(2)]);
    assert_eq!(
        &src[declaration.members[0].span.st..declaration.members[0].span.ed],
        "m"
    );
    assert_eq!(declaration.members[1].name, "n");
    assert!(declaration.members[1].dimensions.is_empty());
}

#[test]
fn preserves_user_types_and_unknown_array_lengths() {
    let tokens = Lexer::new("Custom value[COUNT]")
        .filter(|token| token.kind != TokenKind::Whitespace)
        .collect::<Vec<_>>();
    let declaration = hlsl::parse_declaration(&tokens).unwrap();
    assert_eq!(declaration.ty.unwrap().scalar, "Custom");
    assert!(!declaration.has_error);
    assert_eq!(declaration.members[0].name, "value");
    assert_eq!(declaration.members[0].dimensions, [None]);
}

#[test]
fn preserves_valid_members_before_an_incomplete_declaration() {
    let tokens = Lexer::new("float good, broken[2")
        .filter(|token| token.kind != TokenKind::Whitespace)
        .collect::<Vec<_>>();
    let declaration = hlsl::parse_declaration(&tokens).unwrap();
    assert!(declaration.has_error);
    assert_eq!(declaration.members.len(), 1);
    assert_eq!(declaration.members[0].name, "good");
}

#[test]
fn parses_native_directives_without_astra_directives() {
    let src = "# pragma pack_matrix(row_major)";
    let directive = hlsl::parse_directive(Lexer::new(src).next().unwrap()).unwrap();
    assert_eq!(directive.name, "pragma");
    assert_eq!(directive.rest, "pack_matrix(row_major)");
    assert_eq!(&src[directive.span.st..directive.span.ed], src);
    assert!(hlsl::parse_directive(Lexer::new("//#define VALUE 1").next().unwrap()).is_none());
}

// ============================================================================
// Shader Build & Preprocessing Tests
// ============================================================================

#[test]
fn expands_variables_and_preserves_block_comment_directives_in_shaders() -> anyhow::Result<()> {
    let target: BuildTarget = serde_json::from_value(serde_json::json!({ "path": "test.hlsl" }))?;
    let vars = IndexMap::from([("VALUE".to_owned(), "4".to_owned())]);
    let src = "/*\n\
               //#define VALUE 99\n\
               //#undef VALUE\n\
               //#if false\n\
               //#include \"missing.hlsl\"\n\
               //#endif\n\
               //#pragma once\n\
               ${VALUE}\n\
               */\n\
               // /* ${VALUE}\n\
               //#define RESULT ${VALUE}\n\
               float value = ${RESULT};\n";
    let output = astra::build::shader::build(src, Path::new("test.hlsl"), &target, &[], &vars)?;
    let expected = src
        .replace("//#define RESULT ${VALUE}\n", "")
        .replace("${VALUE}", "4")
        .replace("${RESULT}", "4");
    assert_eq!(output, expected);
    Ok(())
}

#[test]
fn skips_inactive_conditionals_in_shaders() -> anyhow::Result<()> {
    let target: BuildTarget = serde_json::from_value(serde_json::json!({ "path": "test.hlsl" }))?;
    let vars = IndexMap::from([("VALUE".to_owned(), "4".to_owned())]);
    let src = "//#if false\n/*\n//#endif\n*/\nint discarded;\n//#endif\nfloat value = ${VALUE};\n";
    let output = astra::build::shader::build(src, Path::new("test.hlsl"), &target, &[], &vars)?;
    assert_eq!(output, "float value = 4;\n");
    Ok(())
}

#[test]
fn resolves_includes_in_shaders() -> anyhow::Result<()> {
    let target: BuildTarget = serde_json::from_value(serde_json::json!({ "path": "test.hlsl" }))?;
    let dir = tempfile::tempdir()?;
    std::fs::write(dir.path().join("tail.hlsl"), "float b;")?;
    let build = |src| {
        astra::build::shader::build(
            src,
            Path::new("test.hlsl"),
            &target,
            &[dir.path().to_owned()],
            &IndexMap::new(),
        )
    };
    assert_eq!(build("float a;")?, "float a;\n");
    assert_eq!(build("float a;\n//#define X 1")?, "float a;\n");
    assert_eq!(build("//#include <tail.hlsl>\nfloat a;")?, "float b;\nfloat a;\n");
    assert_eq!(build("float a;\n//#include <tail.hlsl>")?, "float a;\nfloat b;\n");
    Ok(())
}

// ============================================================================
// Cbuffer Validation & Warning Tests
// ============================================================================

#[test]
fn warns_on_non_float_types_and_member_padding_in_cbuffers() -> anyhow::Result<()> {
    let (bytes, _guard) = capture_warnings();
    let target: BuildTarget = serde_json::from_value(serde_json::json!({ "path": "test.hlsl" }))?;
    let build = |src| astra::build::shader::build(src, Path::new("test.hlsl"), &target, &[], &IndexMap::new());

    let src = "//#define TYPE float4\ncbuffer Params {\n float a;\n ${TYPE} b;\n int c;\n float2x2 m;\n};\n";
    let output = build(src)?;
    assert!(output.contains(" float4 b;"), "{output}");
    let warnings = String::from_utf8(bytes.lock().unwrap().clone())?;
    assert!(
        warnings.contains("test.hlsl:4:10: cbuffer 'Params' member 'b' introduces 12 bytes"),
        "{warnings}"
    );
    assert!(
        warnings.contains("test.hlsl:5:2: cbuffer 'Params' uses non-float"),
        "{warnings}"
    );
    assert!(
        warnings.contains("test.hlsl:6:11: cbuffer 'Params' member 'm' introduces 20 bytes"),
        "{warnings}"
    );
    assert_eq!(warnings.lines().count(), 3, "{warnings}");
    Ok(())
}

#[test]
fn ignores_cbuffers_in_comments_and_disabled_conditionals() -> anyhow::Result<()> {
    let (bytes, _guard) = capture_warnings();
    let target: BuildTarget = serde_json::from_value(serde_json::json!({ "path": "test.hlsl" }))?;
    let build = |src| astra::build::shader::build(src, Path::new("test.hlsl"), &target, &[], &IndexMap::new());

    let src = "/* cbuffer Ignored { int x; }; */\n\
               // cbuffer IgnoredToo { bool x; };\n\
               //#if false\ncbuffer Dropped { int x; };\n//#endif\n\
               cbuffer Packed { float3 a; float b; float c; };\n\
               cbuffer Vectors { float2 a, b; };\n\
               cbuffer Generic { vector<float, 3> a; float b; matrix<float, 4, 4> c; };\n";
    build(src)?;
    assert!(bytes.lock().unwrap().is_empty());
    Ok(())
}

#[test]
fn warns_on_array_and_matrix_padding_in_cbuffers() -> anyhow::Result<()> {
    let (bytes, _guard) = capture_warnings();
    let target: BuildTarget = serde_json::from_value(serde_json::json!({ "path": "test.hlsl" }))?;
    let build = |src| astra::build::shader::build(src, Path::new("test.hlsl"), &target, &[], &IndexMap::new());

    build("cbuffer Arrays { float a[2]; float3 b; };\ncbuffer Matrices { float2x2 a; float2 b; };")?;
    let warnings = String::from_utf8(bytes.lock().unwrap().clone())?;
    assert!(
        warnings.contains("cbuffer 'Arrays' member 'a' introduces 12 bytes of padding (3 floats)"),
        "{warnings}"
    );
    assert!(
        warnings.contains("cbuffer 'Matrices' member 'a' introduces 8 bytes of padding (2 floats)"),
        "{warnings}"
    );
    assert_eq!(warnings.lines().count(), 2, "{warnings}");
    Ok(())
}

#[test]
fn warns_on_matrix_packing_and_pragma_pack_matrix() -> anyhow::Result<()> {
    let (bytes, _guard) = capture_warnings();
    let target: BuildTarget = serde_json::from_value(serde_json::json!({ "path": "test.hlsl" }))?;
    let build = |src| astra::build::shader::build(src, Path::new("test.hlsl"), &target, &[], &IndexMap::new());

    build("cbuffer Rows { row_major float2x3 a; float b; };\ncbuffer Columns { column_major float2x3 a; float2 b; };")?;
    let warnings = String::from_utf8(bytes.lock().unwrap().clone())?;
    assert!(
        warnings.contains("cbuffer 'Rows' member 'a' introduces 4 bytes of padding (1 float)"),
        "{warnings}"
    );
    assert!(
        warnings.contains("cbuffer 'Columns' member 'a' introduces 16 bytes of padding (4 floats)"),
        "{warnings}"
    );
    assert_eq!(warnings.lines().count(), 2, "{warnings}");

    bytes.lock().unwrap().clear();
    let src = "#define UNUSED 1\n\
               #define MACRO \\\n\
               //\n\
               #pragma unrelated\n\
               # pragma pack_matrix ( row_major ) //\n\
               cbuffer PragmaRows { float2x3 a; float b; };\n\
               cbuffer OverrideColumns { column_major float2x3 a; float2 b; };\n\
               #pragma pack_matrix(column_major)\n\
               cbuffer PragmaColumns { float2x3 a; float2 b; };\n\
               cbuffer OverrideRows { row_major float2x3 a; float b; };\n\
               cbuffer NativeTypes { int a; float4 b; };\n";
    assert_eq!(build(src)?, src);
    let warnings = String::from_utf8(bytes.lock().unwrap().clone())?;
    for (name, padding) in [
        ("PragmaRows", 4),
        ("OverrideColumns", 16),
        ("PragmaColumns", 16),
        ("OverrideRows", 4),
        ("NativeTypes", 12),
    ] {
        assert!(
            warnings.contains(&format!("cbuffer '{name}' member"))
                && warnings.lines().any(|line| line.contains(&format!("cbuffer '{name}'"))
                    && line.contains(&format!("introduces {padding} bytes"))),
            "{warnings}"
        );
    }
    assert!(warnings.contains("cbuffer 'NativeTypes' uses non-float"), "{warnings}");
    assert_eq!(warnings.lines().count(), 6, "{warnings}");

    bytes.lock().unwrap().clear();
    build(
        "/*\n#pragma pack_matrix(row_major)\n*/\ncbuffer CommentedPragma { float2x3 a; };\ncbuffer InternalPragma {\n#pragma pack_matrix(/* */ row_major)\nfloat2x3 a;\n#pragma pack_matrix(column_major)\nfloat2x3 b;\n};",
    )?;
    let warnings = String::from_utf8(bytes.lock().unwrap().clone())?;
    assert!(
        warnings.contains("cbuffer 'CommentedPragma' member 'a' introduces 16 bytes"),
        "{warnings}"
    );
    assert!(
        warnings.contains("cbuffer 'InternalPragma' member 'a' introduces 4 bytes"),
        "{warnings}"
    );
    assert!(
        warnings.contains("cbuffer 'InternalPragma' member 'b' introduces 20 bytes"),
        "{warnings}"
    );
    assert_eq!(warnings.lines().count(), 3, "{warnings}");
    Ok(())
}

#[test]
fn warns_on_unknown_array_length_or_ambiguous_packing_in_cbuffers() -> anyhow::Result<()> {
    let (bytes, _guard) = capture_warnings();
    let target: BuildTarget = serde_json::from_value(serde_json::json!({ "path": "test.hlsl" }))?;
    let build = |src| astra::build::shader::build(src, Path::new("test.hlsl"), &target, &[], &IndexMap::new());

    build("cbuffer Unknown { float a[COUNT]; float4 b; };")?;
    let warnings = String::from_utf8(bytes.lock().unwrap().clone())?;
    assert!(warnings.contains("cannot determine layout"), "{warnings}");
    assert!(!warnings.contains("bytes of padding"), "{warnings}");
    assert_eq!(warnings.lines().count(), 1, "{warnings}");

    bytes.lock().unwrap().clear();
    build(
        "#if FLAG\n#pragma pack_matrix(row_major)\n#endif\ncbuffer UnknownPacking { float2x3 a; };\ncbuffer ExplicitPacking { row_major float2x3 a; };\n#pragma pack_matrix(column_major)\ncbuffer ResetPacking { float2x3 a; };",
    )?;
    let warnings = String::from_utf8(bytes.lock().unwrap().clone())?;
    assert!(
        warnings.contains("cannot determine layout of declaration in cbuffer 'UnknownPacking'"),
        "{warnings}"
    );
    assert!(!warnings.contains("cbuffer 'UnknownPacking' member"), "{warnings}");
    assert!(
        warnings.contains("cbuffer 'ExplicitPacking' member 'a' introduces 4 bytes"),
        "{warnings}"
    );
    assert!(
        warnings.contains("cbuffer 'ResetPacking' member 'a' introduces 16 bytes"),
        "{warnings}"
    );
    assert_eq!(warnings.lines().count(), 3, "{warnings}");
    Ok(())
}

#[test]
fn handles_conditional_compilation_inside_cbuffers() -> anyhow::Result<()> {
    let (bytes, _guard) = capture_warnings();
    let target: BuildTarget = serde_json::from_value(serde_json::json!({ "path": "test.hlsl" }))?;
    let build = |src| astra::build::shader::build(src, Path::new("test.hlsl"), &target, &[], &IndexMap::new());

    build("#if 0\ncbuffer Native { int a; float4 b; };\n#endif\ncbuffer AfterConditional { float3 a; float3 b; };")?;
    let warnings = String::from_utf8(bytes.lock().unwrap().clone())?;
    assert!(warnings.contains("before HLSL preprocessing"), "{warnings}");
    assert!(
        warnings.contains("cbuffer 'AfterConditional' member 'b' introduces 4 bytes"),
        "{warnings}"
    );
    assert_eq!(warnings.lines().count(), 2, "{warnings}");

    bytes.lock().unwrap().clear();
    build(
        "cbuffer ConditionalMember { float3 a;\n#if FLAG\nint branch;\n#else\nfloat4 branch;\n#endif\nint b; float4 c; };\ncbuffer Next { float a; float4 b; };",
    )?;
    let warnings = String::from_utf8(bytes.lock().unwrap().clone())?;
    assert!(
        warnings.contains("subsequent offsets in cbuffer 'ConditionalMember'"),
        "{warnings}"
    );
    assert!(
        warnings.contains("cbuffer 'ConditionalMember' uses non-float"),
        "{warnings}"
    );
    assert!(!warnings.contains("member 'branch'"), "{warnings}");
    assert!(!warnings.contains("member 'c' introduces"), "{warnings}");
    assert!(
        warnings.contains("cbuffer 'Next' member 'b' introduces 12 bytes"),
        "{warnings}"
    );
    assert_eq!(warnings.lines().count(), 3, "{warnings}");
    Ok(())
}

#[test]
fn reports_original_positions_for_included_cbuffers_and_pragmas() -> anyhow::Result<()> {
    let (bytes, _guard) = capture_warnings();
    let target: BuildTarget = serde_json::from_value(serde_json::json!({ "path": "test.hlsl" }))?;
    let dir = tempfile::tempdir()?;
    let include = dir.path().join("members.hlsl");
    std::fs::write(&include, " float4 b;\n bool c;\n")?;
    let output = astra::build::shader::build(
        "cbuffer Included { float a;\n//#include <members.hlsl>\n};",
        Path::new("test.hlsl"),
        &target,
        &[dir.path().to_owned()],
        &IndexMap::new(),
    )?;
    assert!(output.contains(" bool c;"), "{output}");
    let warnings = String::from_utf8(bytes.lock().unwrap().clone())?;
    let canonical = std::fs::canonicalize(include)?;
    assert!(
        warnings.contains(&format!("{}:1:9:", canonical.display())),
        "{warnings}"
    );
    assert!(
        warnings.contains(&format!("{}:2:2:", canonical.display())),
        "{warnings}"
    );
    assert_eq!(warnings.lines().count(), 2, "{warnings}");

    bytes.lock().unwrap().clear();
    std::fs::write(dir.path().join("packing.hlsl"), "#pragma pack_matrix(row_major)\n")?;
    let src = "#pragma pack_matrix(column_major)\n//#include <packing.hlsl>\ncbuffer IncludedPacking { float2x3 a; };";
    let output = astra::build::shader::build(
        src,
        Path::new("test.hlsl"),
        &target,
        &[dir.path().to_owned()],
        &IndexMap::new(),
    )?;
    assert!(output.contains("#pragma pack_matrix(row_major)"), "{output}");
    let warnings = String::from_utf8(bytes.lock().unwrap().clone())?;
    assert!(
        warnings.contains("test.hlsl:3:36: cbuffer 'IncludedPacking' member 'a' introduces 4 bytes"),
        "{warnings}"
    );
    assert_eq!(warnings.lines().count(), 1, "{warnings}");
    Ok(())
}
