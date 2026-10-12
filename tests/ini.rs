use astra::config::BuildTarget;
use indexmap::IndexMap;

#[test]
fn replace_values() -> anyhow::Result<()> {
    let target: BuildTarget = serde_json::from_value(serde_json::json!({ "path": "test.ini" }))?;
    let vars = IndexMap::from([("AAA".to_owned(), "new value".to_owned())]);
    let src = "[section]\n;#replace AAA\n; retained\n\n  key  =  old value  \nother=1\n";
    assert_eq!(
        astra::build::ini::build(src, &target, &[], &vars)?,
        "[section]\n; retained\n\n  key  =  new value  \nother=1\n"
    );
    for src in [
        ";#replace AAA",
        ";#replace AAA\n[section]\nkey=0",
        ";#replace AAA\nkey=",
        ";#replace AAA\n;#include \"missing.ini\"\nkey=0",
    ] {
        assert!(astra::build::ini::build(src, &target, &[], &vars).is_err(), "{src}");
    }
    let vars = IndexMap::from([("AAA".to_owned(), "1\nother=2".to_owned())]);
    assert!(astra::build::ini::build(";#replace AAA\nkey=0", &target, &[], &vars).is_err());
    Ok(())
}

#[test]
fn retain_definitions() -> anyhow::Result<()> {
    let target: BuildTarget = serde_json::from_value(serde_json::json!({ "path": "input.ini" }))?;
    let vars = IndexMap::from([("VALUE".to_owned(), "original".to_owned())]);
    let src = ";#define VALUE changed\nfirst=${VALUE}\n;#undef VALUE\n;#ifndef VALUE\nsecond=1\n;#endif\n";
    assert_eq!(
        astra::build::ini::build(src, &target, &[], &vars)?,
        "first=changed\nsecond=1\n"
    );
    assert_eq!(vars["VALUE"], "original");
    assert!(astra::build::ini::build(";#define BUILD_DIR changed", &target, &[], &vars).is_err());
    Ok(())
}

#[test]
fn retain_include_state() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("input.ini");
    std::fs::write(
        dir.path().join("values.ini"),
        ";#define VALUE included\n[values]\nkey=1",
    )?;
    let target: BuildTarget = serde_json::from_value(serde_json::json!({ "path": file.to_string_lossy() }))?;
    let src = ";#include \"values.ini\"\n[main]\nkey=${VALUE}\n";
    assert_eq!(
        astra::build::ini::build(src, &target, &[], &IndexMap::new())?,
        "[values]\nkey=1\n[main]\nkey=included\n"
    );
    Ok(())
}

#[test]
fn reject_include_cycles() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("input.ini");
    let included = dir.path().join("included.ini");
    std::fs::write(&file, ";#include \"included.ini\"\n")?;
    std::fs::write(&included, ";#include \"input.ini\"\n")?;
    let target: BuildTarget = serde_json::from_value(serde_json::json!({ "path": file.to_string_lossy() }))?;
    let err = astra::build::ini::build(";#include \"included.ini\"\n", &target, &[], &IndexMap::new()).unwrap_err();
    assert!(err.to_string().contains("included.ini:1:2: circular include"), "{err}");
    Ok(())
}

#[test]
fn locate_include_errors() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("input.ini");
    let included = dir.path().join("included.ini");
    std::fs::write(&included, "[values]\nkey=${MISSING}\n")?;
    let target: BuildTarget = serde_json::from_value(serde_json::json!({ "path": file.to_string_lossy() }))?;
    let err = astra::build::ini::build(";#include \"included.ini\"\n", &target, &[], &IndexMap::new()).unwrap_err();
    assert!(
        err.to_string()
            .contains("included.ini:2:5: variable 'MISSING' not found"),
        "{err}"
    );
    assert!(astra::build::ini::build(";#include \"missing.ini\"", &target, &[], &IndexMap::new()).is_err());
    Ok(())
}

#[test]
fn preserve_line_endings() -> anyhow::Result<()> {
    let target: BuildTarget = serde_json::from_value(serde_json::json!({ "path": "input.ini" }))?;
    for src in ["", "[section]\nkey=1", "; comment\n\nkey=1  \n"] {
        assert_eq!(astra::build::ini::build(src, &target, &[], &IndexMap::new())?, src);
    }
    Ok(())
}

#[test]
fn omit_entries() -> anyhow::Result<()> {
    let target: BuildTarget = serde_json::from_value(serde_json::json!({ "path": "test.ini" }))?;
    let src = "[section]\n;#omit\n; annotation\n\nremoved=1\nretained=2\n";
    let output = astra::build::ini::build(src, &target, &[], &IndexMap::new())?;
    assert_eq!(output, "[section]\nretained=2\n");

    Ok(())
}

#[test]
fn omit_sections() -> anyhow::Result<()> {
    let target: BuildTarget = serde_json::from_value(serde_json::json!({ "path": "test.ini" }))?;
    for (src, expected) in [
        (
            "[before]\nkey=1\n;#omit\n; annotation\n\n[removed]\nkey=2\n; removed comment\n\n[next]\nkey=3\n",
            "[before]\nkey=1\n[next]\nkey=3\n",
        ),
        (";#omit\n[removed]\nkey=2\n", ""),
        (";#omit\n[removed]\n[next]\nkey=3", "[next]\nkey=3"),
        (
            ";#omit\n  [removed] ; annotation\nkey=[not a heading]\n;#include \"missing.ini\"\n  [next]\nkey=3\n",
            "  [next]\nkey=3\n",
        ),
    ] {
        let output = astra::build::ini::build(src, &target, &[], &IndexMap::new())?;
        assert_eq!(output, expected, "{src}");
    }
    Ok(())
}

#[test]
fn resolve_once() -> anyhow::Result<()> {
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

    let header_comment = dir.path().join("header_comment.ini");
    std::fs::write(
        &header_comment,
        ";#pragma once ; single inclusion\n[comment]\nval = 1\n",
    )?;
    let src = ";#include \"header_comment.ini\"\n;#include \"header_comment.ini\"\n";
    let result = astra::build::ini::build(src, &target, &[], &IndexMap::new())?;
    assert_eq!(result.matches("[comment]").count(), 1, "{result}");

    let header_unknown = dir.path().join("header_unknown.ini");
    std::fs::write(&header_unknown, ";#pragma message(\"hello\")\n[unknown]\nval = 1\n")?;
    let src = ";#include \"header_unknown.ini\"\n";
    let result = astra::build::ini::build(src, &target, &[], &IndexMap::new())?;
    assert!(result.contains(";#pragma message(\"hello\")"), "{result}");

    let header_if0 = dir.path().join("header_if0.ini");
    std::fs::write(&header_if0, ";#if 0\n;#pragma once\n;#endif\n[if0]\nval = 1\n")?;
    let src = ";#include \"header_if0.ini\"\n;#include \"header_if0.ini\"\n";
    let result = astra::build::ini::build(src, &target, &[], &IndexMap::new())?;
    assert_eq!(result.matches("[if0]").count(), 2, "{result}");

    Ok(())
}
