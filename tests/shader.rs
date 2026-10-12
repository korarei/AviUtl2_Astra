use astra::config::BuildTarget;
use indexmap::IndexMap;
use std::path::Path;

#[test]
fn reject_comments() -> anyhow::Result<()> {
    let target: BuildTarget = serde_json::from_value(serde_json::json!({ "path": "test.hlsl" }))?;
    let err = astra::build::shader::build(
        "float value;\n  /* unfinished",
        Path::new("test.hlsl"),
        &target,
        &[],
        &IndexMap::new(),
    )
    .unwrap_err();
    assert_eq!(err.to_string(), "test.hlsl:2:3: unfinished HLSL comment");
    Ok(())
}

#[test]
fn omit_statements() -> anyhow::Result<()> {
    let target: BuildTarget = serde_json::from_value(serde_json::json!({ "path": "test.hlsl" }))?;
    for statement in [
        "float removed;",
        "float removed() { return 0; }",
        "if (flag) removed(); else removed();",
        "for (int i = 0; i < 2; ++i) { removed(); }",
        "do removed(); while (flag);",
    ] {
        let src = format!("float before;\n//#omit\n// annotation\n{statement} float retained;\n");
        let output = astra::build::shader::build(&src, Path::new("test.hlsl"), &target, &[], &IndexMap::new())?;
        assert!(output.text.contains("float before;"), "{}", output.text);
        assert!(output.text.contains("float retained;"), "{}", output.text);
        assert!(!output.text.contains(statement), "{}", output.text);
        assert!(
            !output.text.contains("annotation") && !output.text.contains("#omit"),
            "{}",
            output.text
        );
    }

    Ok(())
}

#[test]
fn resolve_once() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("input.hlsl");
    std::fs::write(
        dir.path().join("values.hlsl"),
        "//#pragma once\n//#define VALUE 2\nfloat value;\n",
    )?;
    let target: BuildTarget = serde_json::from_value(serde_json::json!({ "path": file.to_string_lossy() }))?;
    let src = "//#include \"values.hlsl\"\n//#include \"values.hlsl\"\nfloat tail = ${VALUE};";
    assert_eq!(
        astra::build::shader::build(src, &file, &target, &[], &IndexMap::new())?.text,
        "float value;\nfloat tail = 2;\n"
    );
    Ok(())
}

#[test]
fn locate_include_errors() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("input.hlsl");
    std::fs::write(dir.path().join("values.hlsl"), "float a;\nfloat b = ${MISSING};")?;
    let target: BuildTarget = serde_json::from_value(serde_json::json!({ "path": file.to_string_lossy() }))?;
    let err =
        astra::build::shader::build("//#include \"values.hlsl\"\n", &file, &target, &[], &IndexMap::new()).unwrap_err();
    assert!(
        err.to_string()
            .contains("values.hlsl:2:11: variable 'MISSING' not found"),
        "{err}"
    );
    assert!(astra::build::shader::build("//#include \"missing.hlsl\"", &file, &target, &[], &IndexMap::new()).is_err());
    Ok(())
}

#[test]
fn reject_include_cycles() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("input.hlsl");
    std::fs::write(&file, "//#include \"values.hlsl\"\n")?;
    std::fs::write(dir.path().join("values.hlsl"), "//#include \"input.hlsl\"\n")?;
    let target: BuildTarget = serde_json::from_value(serde_json::json!({ "path": file.to_string_lossy() }))?;
    let err =
        astra::build::shader::build("//#include \"values.hlsl\"\n", &file, &target, &[], &IndexMap::new()).unwrap_err();
    assert!(err.to_string().contains("values.hlsl:1:3: circular include"), "{err}");
    Ok(())
}

#[test]
fn preserve_native_directives() -> anyhow::Result<()> {
    let target: BuildTarget = serde_json::from_value(serde_json::json!({ "path": "input.hlsl" }))?;
    let src = "#define VALUE 2\n\
        #if VALUE\n\
        #include \"external.hlsl\"\n\
        #pragma pack_matrix(row_major)\n\
        float4 value;\n\
        #endif\n";
    assert_eq!(
        astra::build::shader::build(src, Path::new("input.hlsl"), &target, &[], &IndexMap::new())?.text,
        src
    );
    Ok(())
}

#[test]
fn preserve_comments() -> anyhow::Result<()> {
    let target: BuildTarget = serde_json::from_value(serde_json::json!({ "path": "test.hlsl" }))?;
    let src = "#define VALUE /*\n\
        //#include <missing.hlsl>\n\
        */ 2\n\
        /* comment\n\
        //#include <missing.hlsl>\n\
        */\n\
        // continued \\\n\
        //#include <missing.hlsl>\n\
        float value;";
    let output = astra::build::shader::build(src, Path::new("test.hlsl"), &target, &[], &IndexMap::new())?;
    assert_eq!(output.text, format!("{src}\n"));
    Ok(())
}

#[test]
fn reject_strings() -> anyhow::Result<()> {
    let src = "\"unfinished\n#pragma pack_matrix(row_major)\n";
    let target: BuildTarget = serde_json::from_value(serde_json::json!({ "path": "test.hlsl" }))?;
    let err = astra::build::shader::build(src, Path::new("test.hlsl"), &target, &[], &IndexMap::new()).unwrap_err();
    assert!(
        err.to_string().contains("test.hlsl:1:1: unfinished HLSL string"),
        "{err}"
    );
    Ok(())
}

#[test]
fn expand_variables() -> anyhow::Result<()> {
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
    assert_eq!(output.text, expected);
    Ok(())
}

#[test]
fn expand_definitions() -> anyhow::Result<()> {
    let target: BuildTarget = serde_json::from_value(serde_json::json!({ "path": "test.hlsl" }))?;
    let src = "/*#define VALUE\n\
        1 +\n\
        2\n\
        */\n\
        float value = ${VALUE};\n\
        /*#undef VALUE*/\n\
        //#ifdef VALUE\n\
        float leaked;\n\
        //#endif\n";
    let output = astra::build::shader::build(src, Path::new("test.hlsl"), &target, &[], &IndexMap::new())?;
    assert_eq!(output.text, "float value = 1 +\n2;\n");

    let src = "/*#define VALUE\n${MISSING}\n*/\n";
    let err = astra::build::shader::build(src, Path::new("test.hlsl"), &target, &[], &IndexMap::new()).unwrap_err();
    assert!(
        err.to_string().contains("test.hlsl:2:1: variable 'MISSING' not found"),
        "{err}"
    );

    let err = astra::build::shader::build(
        "/*#define VALUE\n1",
        Path::new("test.hlsl"),
        &target,
        &[],
        &IndexMap::new(),
    )
    .unwrap_err();
    assert!(
        err.to_string().contains("test.hlsl:1:1: unfinished HLSL comment"),
        "{err}"
    );
    Ok(())
}

#[test]
fn skip_conditions() -> anyhow::Result<()> {
    let target: BuildTarget = serde_json::from_value(serde_json::json!({ "path": "test.hlsl" }))?;
    let vars = IndexMap::from([("VALUE".to_owned(), "4".to_owned())]);
    let src = "//#if false\n/*\n//#endif\n*/\nint discarded;\n//#endif\nfloat value = ${VALUE};\n";
    let output = astra::build::shader::build(src, Path::new("test.hlsl"), &target, &[], &vars)?;
    assert_eq!(output.text, "float value = 4;\n");
    Ok(())
}

#[test]
fn resolve_includes() -> anyhow::Result<()> {
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
        .map(|output| output.text)
    };
    assert_eq!(build("float a;")?, "float a;\n");
    assert_eq!(build("float a;\n//#define X 1")?, "float a;\n");
    assert_eq!(build("//#include <tail.hlsl>\nfloat a;")?, "float b;\nfloat a;\n");
    assert_eq!(build("float a;\n//#include <tail.hlsl>")?, "float a;\nfloat b;\n");
    Ok(())
}

#[test]
fn include_extensions() -> anyhow::Result<()> {
    let target: BuildTarget = serde_json::from_value(serde_json::json!({ "path": "test.hlsl" }))?;
    let dir = tempfile::tempdir()?;
    std::fs::write(dir.path().join("tail.lua"), "//#define VALUE 3\nfloat b = ${VALUE};")?;
    std::fs::write(dir.path().join("tail.txt"), "float c;")?;
    let build = |src| {
        astra::build::shader::build(
            src,
            Path::new("test.hlsl"),
            &target,
            &[dir.path().to_owned()],
            &IndexMap::new(),
        )
    };
    let output = build("//#include <tail.lua>\n//#include <tail.txt>\n//#include <tail.lua>")?;
    assert_eq!(output.text, "float b = 3;\nfloat c;\nfloat b = 3;\n");
    Ok(())
}
