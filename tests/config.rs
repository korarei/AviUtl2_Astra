use astra::config::{
    AviUtl2Version, BuildCall, BuildType, Config, Package, PackageSource, PackageSourceFile, ReleaseDependency,
    TaskCall,
};
use indexmap::IndexMap;
use std::fmt::Write as _;
use std::path::Path;
use validator::Validate;

fn load_config_with(source: &str, defines: IndexMap<String, String>, ver: Option<&str>) -> anyhow::Result<Config> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("astra.toml");
    std::fs::write(&file, source)?;
    Config::load(file, defines, ver)
}

fn load_config(source: &str) -> anyhow::Result<Config> {
    load_config_with(source, IndexMap::new(), None)
}

#[test]
fn loads_project_astra_defaults_and_aliases() -> anyhow::Result<()> {
    let source = r#"
version = 2

[variables]
VERSION = "1.2.3"
AVIUTL2 = "2.1.7a"
RUN_RELEASE = "main"
DEFAULT_SHELL = "pwsh"

[project]
name = "${{ ROOT }}-name"
version = "${{ VERSION }}"
author = ["Alice", "Bob"]
requires-aviutl2 = "${{ AVIUTL2 }}"

[astra]
requires-astra = ">=0.0.0"
build-dir = "${{ ROOT }}/build"
dist-dir = "${{ ROOT }}/dist"

[astra.run]
release = "${{ RUN_RELEASE }}"

[defaults.run]
shell = "${{ DEFAULT_SHELL }}"

[tasks.main]
run = "echo main"

[builds.main]
name = "${{ PROJECT_NAME }}-${{ PROJECT_VERSION }}"
suffix = ".anm2"
include-dirs = ["${{ PROJECT_AUTHOR }}", "${{ PROJECT_AUTHORS }}"]
artifacts = "${{ AVIUTL2_VERSION }}"

[releases.main]
"#;

    let config = load_config_with(
        source,
        IndexMap::from([("ROOT".to_owned(), "project".to_owned())]),
        Some(" 2.0.0 "),
    )?;
    assert_eq!(
        config.astra().build_dir(),
        Path::new("project").join("build").to_str().unwrap()
    );
    assert_eq!(
        config.astra().dist_dir(),
        Path::new("project").join("dist").to_str().unwrap()
    );
    assert_eq!(config.astra().run().release_id(), Some("main"));
    assert_eq!(config.task(&TaskCall::Simple("main".to_owned()))?.shell(), Some("pwsh"));

    let build = config.build("main", BuildType::Debug)?;
    assert_eq!(build.name(), "project-name-2.0.0");
    assert_eq!(
        build.include_dirs(),
        &["Alice, Bob".to_owned(), "Alice, Bob".to_owned()]
    );
    assert_eq!(build.artifacts(), &["2010701".to_owned()]);

    for key in ["version", "config_version", "config-version"] {
        load_config(&format!("{key} = 2\n\n[project]\nname = \"demo\"\n"))?;
    }

    let aliases = load_config(
        r#"
version = 2

[project]
name = "demo"

[default.run]
shell = "alias-shell"

[task.main]
run = "echo alias"

[build.main]
suffix = ".anm2"

[release.main]
"#,
    )?;
    assert_eq!(
        aliases.task(&TaskCall::Simple("main".to_owned()))?.shell(),
        Some("alias-shell")
    );
    assert_eq!(aliases.builds(BuildType::Debug).len(), 1);
    assert_eq!(aliases.releases().count(), 1);

    let defaults = load_config("version = 2\n\n[project]\nname = \"demo\"\n")?;
    assert_eq!(defaults.astra().build_dir(), "build");
    assert_eq!(defaults.astra().dist_dir(), "dist");
    assert_eq!(defaults.astra().run().release_id(), None);

    for key in ["build_dir", "build-dir", "build_directory", "build-directory"] {
        let config = load_config(&format!(
            "version = 2\n\n[project]\nname = \"demo\"\n\n[astra]\n{key} = \"custom-build\"\n"
        ))?;
        assert_eq!(config.astra().build_dir(), "custom-build", "{key}");
    }
    for key in [
        "dist_dir",
        "dist-dir",
        "dist_directory",
        "dist-directory",
        "distribution_dir",
        "distribution-dir",
        "distribution_directory",
        "distribution-directory",
    ] {
        let config = load_config(&format!(
            "version = 2\n\n[project]\nname = \"demo\"\n\n[astra]\n{key} = \"custom-dist\"\n"
        ))?;
        assert_eq!(config.astra().dist_dir(), "custom-dist", "{key}");
    }
    for key in [
        "venv_dir",
        "venv-dir",
        "venv_directory",
        "venv-directory",
        "virtualenv_dir",
        "virtualenv-dir",
        "virtualenv_directory",
        "virtualenv-directory",
    ] {
        assert!(
            load_config(&format!(
                "version = 2\n\n[project]\nname = \"demo\"\n\n[astra]\n{key} = \"custom-venv\"\n"
            ))
            .is_err(),
            "{key}"
        );
    }

    for (value, expected) in [("2010701", 2_010_701), ("2.1.7a", 2_010_701), ("v2.1.8", 2_010_800)] {
        assert_eq!(AviUtl2Version::String(value.to_owned()).resolve()?, expected);
    }
    assert_eq!(AviUtl2Version::Number(2_010_701).resolve()?, 2_010_701);
    for value in ["", "invalid"] {
        assert!(AviUtl2Version::String(value.to_owned()).resolve().is_err());
    }
    Ok(())
}

#[test]
fn resolves_build_modes_suffixes_targets_and_calls() -> anyhow::Result<()> {
    let suffixes = [
        ("anm2", ".anm2", true),
        ("tra2", ".tra2", true),
        ("obj2", ".obj2", true),
        ("scn2", ".scn2", true),
        ("cam2", ".cam2", true),
        ("anm", ".anm", false),
        ("tra", ".tra", false),
        ("obj", ".obj", false),
        ("scn", ".scn", false),
        ("cam", ".cam", false),
    ];
    let mut source = String::from("version = 2\n\n[project]\nname = \"demo\"\n");
    for &(id, suffix, _) in &suffixes {
        let _ = writeln!(source, "\n[build.{id}]\nsuffix = \"{suffix}\"");
    }
    source.push_str(
        r#"
[build.no_suffix]
outputs = ["artifact.bin"]

[tasks.single]
run = "echo single"
dependencies = ["task:prepare"]
finally = ["task:finish"]

[variables]
PARENT = "parent"
BUILD_NAME = "main"
TARGET_NAME = "named"
TARGET_DIR = "include"
ENCODING = "shift_jis"
TASK_ID = "worker"
MODE = "debug"

[astra]
build-dir = "out"

[build.main]
name = "${{ BUILD_NAME }}"
suffix = " anm2 "
enabled = " all "
newline = " LF "
variables = { LOCAL = "${{ PARENT }}-build" }
dependencies = [
    "task:prepare",
    { task = "${{ TASK_ID }}", environment = { DEP_ENV = "${{ DEP_VAR }}" }, variables = { DEP_VAR = "${{ LOCAL }}" } }
]
finally = ["task:finish"]
include-dirs = ["${{ LOCAL }}", "${{ BUILD_DIR }}"]
outputs = ["${{ BUILD_DIRECTORY }}"]

[[build.main.target]]
name = "${{ TARGET_NAME }}"
path = "scripts/main.lua"
include-directories = ["${{ TARGET_DIR }}"]
variables = { TARGET_LOCAL = "${{ LOCAL }}" }
encoding = "${{ ENCODING }}"

[[build.main.target]]
path = "scripts/secondary.lua"

[build.single]
name = "single"
suffix = ".anm2"
dependencies = ["task:prepare"]
finally = ["task:finish"]
include-dirs = ["include"]
target = [{ name = "ignored", path = "scripts/only.lua" }]
artifact = "single.bin"

[build.boolean_true]
suffix = ".anm2"
enabled = true
artifact = "artifact-one"

[build.boolean_false]
suffix = ".anm2"
enabled = false
output = "artifact-two"

[build.debug]
suffix = ".anm2"
enabled = " DEBUG "

[build.release]
suffix = ".anm2"
enabled = "release"

[build.detailed_debug]
suffix = ".anm2"
enabled = { debug = false }

[build.detailed_release]
suffix = ".anm2"
enabled = { release = false }

[build.detailed_default]
suffix = ".anm2"
enabled = {}

[build.variable_mode]
suffix = ".anm2"
enabled = "${{ MODE }}"

[build.newline_crlf]
suffix = ".anm2"
newline = "crlf"

[build.newline_lf]
suffix = ".anm2"
newline = "lf"

[build.legacy_crlf]
suffix = ".anm"
newline = "crlf"

[build.literal_lf]
suffix = ".anm2"
newline = "\n"

[build.literal_crlf]
suffix = ".anm2"
newline = "\r\n"
"#,
    );

    let config = load_config(&source)?;
    assert_eq!(BuildType::Debug.as_lowercase(), "debug");
    assert_eq!(BuildType::Release.to_string(), "Release");

    for &(id, suffix, utf8) in &suffixes {
        let build = config.build(id, BuildType::Debug)?;
        assert_eq!(build.suffix(), Some(suffix));
        assert_eq!(
            build.encoding(),
            if utf8 {
                encoding_rs::UTF_8
            } else {
                encoding_rs::SHIFT_JIS
            }
        );
        assert_eq!(build.newline(), "\r\n");
    }

    for (id, debug, release) in [
        ("boolean_true", true, true),
        ("boolean_false", false, false),
        ("debug", true, false),
        ("release", false, true),
        ("detailed_debug", false, true),
        ("detailed_release", true, false),
        ("detailed_default", true, true),
        ("variable_mode", true, false),
    ] {
        let build = config.build(id, BuildType::Debug)?;
        assert_eq!(build.is_enabled(BuildType::Debug), debug, "{id}");
        assert_eq!(build.is_enabled(BuildType::Release), release, "{id}");
    }

    let build = config.build("main", BuildType::Debug)?;
    assert_eq!(build.id(), "main");
    assert_eq!(build.name(), "main");
    assert_eq!(build.suffix(), Some(".anm2"));
    assert_eq!(build.newline(), "\n");
    assert!(build.is_enabled(BuildType::Debug));
    assert!(build.is_enabled(BuildType::Release));
    assert_eq!(build.include_dirs()[0], "parent-build");

    let no_suffix_build = config.build("no_suffix", BuildType::Debug)?;
    assert_eq!(no_suffix_build.suffix(), None);
    let dir = Path::new("out").join("main").to_string_lossy().into_owned();
    assert_eq!(build.include_dirs()[1], dir);
    assert_eq!(build.artifacts(), &["out/main".to_owned()]);

    assert_eq!(
        config.build("boolean_true", BuildType::Debug)?.artifacts(),
        &["artifact-one".to_owned()]
    );
    assert_eq!(
        config.build("boolean_false", BuildType::Debug)?.artifacts(),
        &["artifact-two".to_owned()]
    );

    let TaskCall::Detailed { task, env, vars } = &build.depends()[1] else {
        panic!("detailed build dependency expected");
    };
    assert_eq!(task, "worker");
    assert_eq!(env.get("DEP_ENV").map(String::as_str), Some("parent-build"));
    assert_eq!(vars.get("DEP_VAR").map(String::as_str), Some("parent-build"));
    assert_eq!(vars.get("BUILD_TYPE").map(String::as_str), Some("debug"));
    assert_eq!(vars.get("DEBUG").map(String::as_str), Some("1"));
    assert_eq!(vars.get("BUILD_DIR").map(String::as_str), Some(dir.as_str()));
    assert!(matches!(build.depends()[0], TaskCall::Detailed { .. }));
    assert!(matches!(build.finally()[0], TaskCall::Detailed { .. }));
    let TaskCall::Detailed { vars, .. } = &build.finally()[0] else {
        panic!("detailed build finalizer expected");
    };
    assert_eq!(vars.get("BUILD_TYPE").map(String::as_str), Some("debug"));

    let release = config.build("main", BuildType::Release)?;
    let TaskCall::Detailed { vars, .. } = &release.depends()[1] else {
        panic!("detailed release dependency expected");
    };
    assert_eq!(vars.get("BUILD_TYPE").map(String::as_str), Some("release"));
    assert_eq!(vars.get("NDEBUG").map(String::as_str), Some("1"));
    assert!(!vars.contains_key("DEBUG"));

    let targets = build.targets().collect::<anyhow::Result<Vec<_>>>()?;
    assert_eq!(targets.len(), 2);
    assert_eq!(targets[0].name(), "named");
    assert_eq!(
        targets[0].path(),
        Path::new("scripts").join("main.lua").to_str().unwrap()
    );
    assert_eq!(targets[0].include_dirs(), &["include".to_owned()]);
    assert_eq!(
        targets[0].vars().get("TARGET_LOCAL").map(String::as_str),
        Some("parent-build")
    );
    assert_eq!(
        targets[0].vars().get("SCRIPT_NAME").map(String::as_str),
        Some("named@main")
    );
    assert_eq!(targets[0].encoding(), encoding_rs::SHIFT_JIS);
    assert_eq!(targets[1].name(), "secondary");
    assert_eq!(
        targets[1].vars().get("SCRIPT_NAME").map(String::as_str),
        Some("secondary@main")
    );
    assert_eq!(targets[1].encoding(), encoding_rs::UTF_8);

    assert_eq!(config.build("newline_crlf", BuildType::Debug)?.newline(), "\r\n");
    assert_eq!(config.build("newline_lf", BuildType::Debug)?.newline(), "\n");
    assert_eq!(config.build("legacy_crlf", BuildType::Debug)?.newline(), "\r\n");
    assert_eq!(config.build("literal_lf", BuildType::Debug)?.newline(), "\n");
    assert_eq!(config.build("literal_crlf", BuildType::Debug)?.newline(), "\r\n");

    let default = config.build("anm2", BuildType::Debug)?;
    assert_eq!(default.name(), "demo");
    assert_eq!(default.targets().count(), 0);

    let single = config.build("single", BuildType::Debug)?;
    assert!(matches!(&single.depends()[0], TaskCall::Detailed { task, .. } if task == "prepare"));
    assert_eq!(single.depends()[0].id(), "prepare");
    assert_eq!(single.finally()[0].id(), "finish");
    assert_eq!(single.include_dirs(), &["include".to_owned()]);
    assert_eq!(single.artifacts(), &["single.bin".to_owned()]);
    let target = single.targets().next().unwrap()?;
    assert_eq!(target.vars().get("SCRIPT_NAME").map(String::as_str), Some("single"));

    let task = config.task(&TaskCall::Simple("single".to_owned()))?;
    assert!(matches!(&task.depends()[0], TaskCall::Simple(id) if id == "prepare"));
    assert_eq!(task.depends()[0].id(), "prepare");
    assert_eq!(task.finally()[0].id(), "finish");
    Ok(())
}

#[test]
fn resolves_tasks_and_environment_precedence() -> anyhow::Result<()> {
    let config = load_config(
        r#"
version = 2

[variables]
SHARED = "shared"

[project]
name = "demo"

[environment]
BASE = "base"
OVERRIDE = "global"
GLOBAL = "${{ SHARED }}"

[defaults.run]
shell = "default-shell"

[tasks.hello]
hidden = true
variables = { LOCAL = "${{ GREETING }}-${{ SHARED }}" }
run = ["echo ${{ LOCAL }}", "${{ SHELL }} ${{ LOCAL }}"]
dependencies = [
    "task:prepare",
    { task = "${{ WORKER }}", environment = { DEP_ENV = "${{ DEP_VAR }}" }, variables = { DEP_VAR = "${{ LOCAL }}" } }
]
finally = ["task:finish"]
environment = { TASK_ENV = "${{ LOCAL }}", OVERRIDE = "task" }

[tasks.explicit]
shell = "explicit-shell"
run = "echo explicit"

[tasks."build:main"]
run = "echo build"
"#,
    )?;

    let call = TaskCall::Detailed {
        task: "hello".to_owned(),
        vars: IndexMap::from([
            ("GREETING".to_owned(), "hello".to_owned()),
            ("SHELL".to_owned(), "sh".to_owned()),
            ("WORKER".to_owned(), "worker".to_owned()),
        ]),
        env: IndexMap::from([
            ("CALL_ENV".to_owned(), "call".to_owned()),
            ("OVERRIDE".to_owned(), "call".to_owned()),
        ]),
    };
    let task = config.task(&call)?;
    assert_eq!(task.id(), "hello");
    assert!(task.hide());
    assert_eq!(task.shell(), Some("default-shell"));
    assert_eq!(
        task.run(),
        &["echo hello-shared".to_owned(), "sh hello-shared".to_owned()]
    );
    assert_eq!(task.depends()[0].id(), "prepare");
    assert_eq!(task.finally()[0].id(), "finish");

    let TaskCall::Detailed { task: id, env, vars } = &task.depends()[1] else {
        panic!("detailed task dependency expected");
    };
    assert_eq!(id, "worker");
    assert_eq!(env.get("DEP_ENV").map(String::as_str), Some("hello-shared"));
    assert_eq!(vars.get("DEP_VAR").map(String::as_str), Some("hello-shared"));

    let env = serde_json::to_value(&task)?["env"].clone();
    assert_eq!(env["BASE"], "base");
    assert_eq!(env["GLOBAL"], "shared");
    assert_eq!(env["TASK_ENV"], "hello-shared");
    assert_eq!(env["CALL_ENV"], "call");
    assert_eq!(env["OVERRIDE"], "call");
    assert_eq!(
        config.task(&TaskCall::Simple("explicit".to_owned()))?.shell(),
        Some("explicit-shell")
    );
    assert_eq!(
        config.task(&TaskCall::Simple("build:main".to_owned()))?.id(),
        "build:main"
    );
    Ok(())
}

#[test]
fn resolves_releases_packages_sources_and_defaults() -> anyhow::Result<()> {
    let config = load_config(
        r#"
version = 2

[variables]
ROOT = "root"
BUILD_ID = "core"
WORKER = "worker"
URL = "https://example.com/archive.zip"
HOST = "example.com"
PATH_VALUE = "assets/data.txt"
FILE_NAME = "generated.lua"
ENCODING = "shift_jis"
NEWLINE = "lf"
PICK = "*.txt"
PACKAGE = "demo"
CHANGELOG = "changes"

[project]
name = "demo"

[release.publish]
variables = { LOCAL = "${{ ROOT }}" }
dependencies = [
    "clean",
    "task:setup",
    "build:${{ BUILD_ID }}",
    { task = "${{ WORKER }}", environment = { FOO = "${{ LOCAL }}" }, variables = { BAR = "${{ LOCAL }}" } },
    { build = "${{ BUILD_ID }}" }
]
finally = ["task:finish"]

[release.publish.package]
filename = "${{ PACKAGE }}.AU2PKG.ZIP"
id = "${{ LOCAL }}-id"
name = "${{ LOCAL }}-name"
uninstall-subdirectory-files = true
information = "${{ LOCAL }} information"
license = "${{ LOCAL }} license"
summary = "${{ LOCAL }} summary"
description = "${{ LOCAL }} description"
website = "https://${{ HOST }}"
report-issue = "https://${{ HOST }}/issues"

[[release.publish.package.content]]
dir = "Plugin/${{ LOCAL }}"
source = ["build:${{ BUILD_ID }}", "build:main", { build = "build:main" }]

[[release.publish.package.content]]
dir = "Script/${{ LOCAL }}"
source = "url:${{ URL }}"

[[release.publish.package.content]]
dir = "Language/${{ LOCAL }}"
source = "HTTP://${{ HOST }}/archive.zip"

[[release.publish.package.content]]
dir = "Alias/${{ LOCAL }}"
source = "custom/${{ PATH_VALUE }}"

[[release.publish.package.content]]
dir = "Figure/${{ LOCAL }}"
source = { build = "${{ BUILD_ID }}" }

[[release.publish.package.content]]
dir = "Transition/${{ LOCAL }}"
source = { path = "${{ PATH_VALUE }}" }

[[release.publish.package.content]]
dir = "Preset/${{ LOCAL }}"
source = { url = "${{ URL }}", extract = true, picker = "${{ PICK }}" }

[[release.publish.package.content]]
directory = "./Default/${{ LOCAL }}"
src = { filename = "${{ FILE_NAME }}", content = "line1\r\n${{ LOCAL }}\r\n", encoding = "${{ ENCODING }}", newline = "${{ NEWLINE }}" }

[[release.publish.package.content]]
dir = "Plugin/${{ LOCAL }}/single"
source = ["build:main"]

[[release.publish.package.content]]
dir = "Plugin/${{ LOCAL }}/scalar"
source = "build:main"

[[release.publish.package.content]]
dir = "Plugin/${{ LOCAL }}/table"
source = { build = "build:main" }

[[release.publish.package.content]]
dir = "Plugin/${{ LOCAL }}/array-table"
source = [{ build = "build:main" }]

[release.publish.release-notes]
change-log = "${{ CHANGELOG }}"
filename = "notes-${{ LOCAL }}.md"
encoding = "${{ ENCODING }}"

[release.empty.package]

[release.single]
dependencies = ["build:main"]
finally = ["task:finish"]

[release.single.package]
content = [{ dir = "Plugin", source = ["build:main"] }]

[release.notes_only.notes]
changelog = "changes"
"#,
    )?;

    let release = config.release("publish")?;
    assert_eq!(release.id(), "publish");
    assert_eq!(release.depends().len(), 5);
    assert!(matches!(&release.depends()[0], ReleaseDependency::Task(task) if task.id() == "clean"));
    assert!(matches!(&release.depends()[1], ReleaseDependency::Task(task) if task.id() == "setup"));
    assert!(matches!(&release.depends()[2], ReleaseDependency::Build(build) if build.id() == "core"));
    let ReleaseDependency::Task(TaskCall::Detailed { task, env, vars }) = &release.depends()[3] else {
        panic!("detailed release task expected");
    };
    assert_eq!(task, "worker");
    assert_eq!(env.get("FOO").map(String::as_str), Some("root"));
    assert_eq!(vars.get("BAR").map(String::as_str), Some("root"));
    assert!(matches!(&release.depends()[4], ReleaseDependency::Build(build) if build.id() == "core"));
    assert_eq!(release.finally()[0].id(), "finish");

    let package = release.package().unwrap();
    assert_eq!(package.filename(), "demo.AU2PKG.ZIP");
    let meta = serde_json::to_value(package)?;
    for (key, value) in [
        ("id", "root-id"),
        ("name", "root-name"),
        ("information", "root information"),
        ("license", "root license"),
        ("summary", "root summary"),
        ("description", "root description"),
        ("website", "https://example.com"),
        ("report_issue", "https://example.com/issues"),
    ] {
        assert_eq!(meta[key], value, "{key}");
    }
    assert_eq!(meta["uninstall_subdirectory_files"], true);

    let contents = package.contents().collect::<anyhow::Result<Vec<_>>>()?;
    assert_eq!(contents.len(), 12);
    assert!(matches!(&contents[0].sources()[0], PackageSource::Build(src) if src.id() == "core"));
    assert!(matches!(&contents[0].sources()[1], PackageSource::Build(src) if src.id() == "main"));
    assert!(matches!(&contents[0].sources()[2], PackageSource::Build(src) if src.id() == "build:main"));
    assert!(
        matches!(&contents[1].sources()[0], PackageSource::Url(src) if src.url() == "https://example.com/archive.zip")
    );
    assert!(
        matches!(&contents[2].sources()[0], PackageSource::Url(src) if src.url() == "HTTP://example.com/archive.zip")
    );
    assert!(matches!(&contents[3].sources()[0], PackageSource::Path(src) if src.path() == "custom/assets/data.txt"));
    assert!(matches!(&contents[4].sources()[0], PackageSource::Build(src) if src.id() == "core"));
    assert!(matches!(&contents[5].sources()[0], PackageSource::Path(src) if src.path() == "assets/data.txt"));
    let PackageSource::Url(url) = &contents[6].sources()[0] else {
        panic!("URL source expected");
    };
    assert_eq!(url.extract(), Some(true));
    assert_eq!(url.pick(), Some("*.txt"));
    let PackageSource::Url(url) = &contents[1].sources()[0] else {
        panic!("URL source expected");
    };
    assert_eq!(url.extract(), None);
    assert_eq!(url.pick(), None);
    let PackageSource::File(file) = &contents[7].sources()[0] else {
        panic!("file source expected");
    };
    assert_eq!(file.filename(), "generated.lua");
    assert_eq!(file.content().as_ref(), "line1\nroot\n");
    assert_eq!(file.encoding(), encoding_rs::SHIFT_JIS);
    assert_eq!(file.newline(), Some("\n"));
    assert!(matches!(&contents[8].sources()[0], PackageSource::Build(src) if src.id() == "main"));
    assert!(matches!(&contents[9].sources()[0], PackageSource::Build(src) if src.id() == "main"));
    assert!(matches!(&contents[10].sources()[0], PackageSource::Build(src) if src.id() == "build:main"));
    assert!(matches!(&contents[11].sources()[0], PackageSource::Build(src) if src.id() == "build:main"));

    let notes = release.notes().unwrap();
    assert_eq!(notes.changelog(), "changes");
    assert_eq!(notes.filename(), "notes-root.md");
    assert_eq!(notes.encoding(), encoding_rs::SHIFT_JIS);

    let empty = config.release("empty")?;
    assert_eq!(empty.package().unwrap().filename(), "demo.au2pkg.zip");
    assert!(empty.package().unwrap().contents().next().is_none());
    let single = config.release("single")?;
    assert!(matches!(&single.depends()[0], ReleaseDependency::Build(build) if build.id() == "main"));
    assert_eq!(single.finally()[0].id(), "finish");
    let content = single.package().unwrap().contents().next().unwrap()?;
    assert!(matches!(&content.sources()[0], PackageSource::Build(build) if build.id() == "main"));
    let notes_only = config.release("notes_only")?;
    assert_eq!(notes_only.notes().unwrap().filename(), "RELEASE_NOTES.md");
    assert_eq!(notes_only.notes().unwrap().encoding(), encoding_rs::UTF_8);

    let package = Package::default();
    assert_eq!(package.filename(), "package.au2pkg.zip");
    let file: PackageSourceFile = toml::from_str("filename = \"plain.lua\"\ncontent = \"plain\"\n")?;
    assert_eq!(file.content().as_ref(), "plain");
    assert_eq!(file.encoding(), encoding_rs::UTF_8);
    assert_eq!(file.newline(), None);
    Ok(())
}

#[test]
fn converts_paths_without_converting_globs_or_urls() -> anyhow::Result<()> {
    let config = load_config(
        r#"
version = 2

[project]
name = "demo"

[variables]
ROOT = 'root/part\leaf'
GLOB = 'assets/**/file\?.lua'

[astra]
build-dir = '${{ ROOT }}/build'
dist-dir = '${{ ROOT }}/dist'

[build.main]
suffix = ".anm2"
include-dirs = ['${{ ROOT }}/include']
artifacts = ['${{ BUILD_DIR }}/**/*.anm2', '${{ BUILD_DIRECTORY }}/single.anm2', '${{ GLOB }}']
dependencies = [{ task = "prepare" }]

[build.paths]
suffix = ".anm2"
include-dirs = ['${{ ROOT }}/include']
targets = [{ path = '${{ ROOT }}/main.lua', include-dirs = ['${{ ROOT }}/local'] }]

[release.main.notes]
changelog = '${{ ROOT }}/changes.md'

[release.main.package]
filename = "demo.zip"
content = [{ dir = '${{ ROOT }}/dest', source = [
    { path = '${{ GLOB }}' },
    { url = 'https://example.com/path/archive.zip', extract = true, pick = '${{ GLOB }}' }
] }]
"#,
    )?;
    let root = if cfg!(windows) {
        r"root\part\leaf"
    } else {
        r"root/part\leaf"
    };
    assert_eq!(
        config.astra().build_dir(),
        Path::new(root).join("build").to_str().unwrap()
    );
    assert_eq!(
        config.astra().dist_dir(),
        Path::new(root).join("dist").to_str().unwrap()
    );

    let build = config.build("main", BuildType::Debug);
    #[cfg(not(windows))]
    {
        assert!(build.unwrap_err().to_string().contains("backslashes"));
    }
    #[cfg(windows)]
    {
        let build = build?;
        assert_eq!(
            build.artifacts(),
            &[
                "root/part/leaf/build/main/**/*.anm2".to_owned(),
                "root/part/leaf/build/main/single.anm2".to_owned(),
                r"assets/**/file\?.lua".to_owned(),
            ]
        );
        let TaskCall::Detailed { vars, .. } = &build.depends()[0] else {
            panic!("detailed dependency expected");
        };
        assert_eq!(
            vars.get("BUILD_DIR").map(String::as_str),
            Some(r"root\part\leaf\build\main")
        );
    }

    let build = config.build("paths", BuildType::Debug)?;
    assert_eq!(
        build.include_dirs(),
        &[Path::new(root).join("include").to_string_lossy().into_owned()]
    );
    let target = build.targets().next().unwrap()?;
    assert_eq!(target.path(), Path::new(root).join("main.lua").to_str().unwrap());
    assert_eq!(
        target.include_dirs(),
        &[Path::new(root).join("local").to_string_lossy().into_owned()]
    );

    let release = config.release("main")?;
    assert_eq!(
        release.notes().unwrap().changelog(),
        Path::new(root).join("changes.md").to_str().unwrap()
    );
    let content = release.package().unwrap().contents().next().unwrap()?;
    assert_eq!(content.dir(), Path::new(root).join("dest").to_str().unwrap());
    assert!(matches!(&content.sources()[0], PackageSource::Path(src) if src.path() == r"assets/**/file\?.lua"));
    let PackageSource::Url(src) = &content.sources()[1] else {
        panic!("URL source expected");
    };
    assert_eq!(src.url(), "https://example.com/path/archive.zip");
    assert_eq!(src.pick(), Some(r"assets/**/file\?.lua"));
    Ok(())
}

#[test]
fn validates_artifacts_and_package_globs() -> anyhow::Result<()> {
    for (pattern, valid) in [
        ("assets/**/*.lua", true),
        (r"assets/file\?.lua", true),
        ("../assets/*.lua", true),
        ("/assets/*.lua", true),
        ("C:/assets/*.lua", true),
        ("C:assets/*.lua", false),
        ("C:", false),
        ("custom://assets/data.txt", false),
        ("assets/[", false),
    ] {
        let config = load_config(&format!(
            "version = 2\n[project]\nname = 'demo'\n[build.main]\nartifacts = ['{pattern}']\n"
        ))?;
        assert_eq!(config.build("main", BuildType::Debug).is_ok(), valid, "{pattern}");
        assert_eq!(
            toml::from_str::<astra::config::PackageSourcePath>(&format!("path = '{pattern}'"))?
                .validate()
                .is_ok(),
            valid,
            "{pattern}"
        );
    }
    Ok(())
}

#[test]
fn validates_zip_pickers() -> anyhow::Result<()> {
    for (pattern, valid) in [
        ("assets/**/*.lua", true),
        ("./assets/*.lua", true),
        ("assets/../scripts/*.lua", true),
        (r"assets/file\?.lua", true),
        ("../assets/*.lua", false),
        ("assets/../../*.lua", false),
        ("/assets/*.lua", false),
        ("C:/assets/*.lua", false),
        ("C:assets/*.lua", false),
        ("assets/../C:/scripts/*.lua", false),
        ("assets/[", false),
    ] {
        assert_eq!(
            toml::from_str::<astra::config::PackageSourceUrl>(&format!(
                "url = 'https://example.com/archive.zip'\nextract = true\npick = '{pattern}'"
            ))?
            .validate()
            .is_ok(),
            valid,
            "{pattern}"
        );
    }
    Ok(())
}

#[test]
fn rejects_unsafe_output_directories_without_cleaning_saved_paths() -> anyhow::Result<()> {
    for field in ["build-dir", "dist-dir"] {
        for path in [
            ".",
            "..",
            "nested/..",
            "nested/../..",
            ".astra",
            "nested/../.astra/cache",
        ] {
            assert!(
                load_config(&format!(
                    "version = 2\n[project]\nname = 'demo'\n[astra]\n{field} = '{path}'\n"
                ))
                .is_err(),
                "{field}: {path}"
            );
        }
        let config = load_config(&format!(
            "version = 2\n[project]\nname = 'demo'\n[astra]\n{field} = 'nested/../output'\n"
        ))?;
        assert_eq!(
            if field == "build-dir" {
                config.astra().build_dir()
            } else {
                config.astra().dist_dir()
            },
            if cfg!(windows) {
                r"nested\..\output"
            } else {
                "nested/../output"
            }
        );
        assert_eq!(
            load_config(&format!(
                "version = 2\n[project]\nname = 'demo'\n[astra]\n{field} = '.ASTRA/cache'\n"
            ))
            .is_ok(),
            !cfg!(windows),
            "{field}"
        );
    }
    Ok(())
}

#[cfg(windows)]
#[test]
fn rejects_unicode_case_variants_of_project_ancestors() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    std::fs::create_dir(dir.path().join("Ä"))?;
    let file = dir.path().join("Ä").join("astra.toml");
    for field in ["build-dir", "dist-dir"] {
        std::fs::write(
            &file,
            format!("version = 2\n[project]\nname = 'demo'\n[astra]\n{field} = '../ä'\n"),
        )?;
        assert!(
            Config::load(&file, IndexMap::new(), None)
                .unwrap_err()
                .to_string()
                .contains("project root or one of its parents"),
            "{field}"
        );
    }
    Ok(())
}

#[test]
fn rejects_invalid_configuration_patterns() -> anyhow::Result<()> {
    for source in [
        "version = 1\n\n[project]\nname = \"demo\"\n",
        "[project]\nname = \"demo\"\n",
        "version = 2\n\n[project]\nname = \"\"\n",
        "version = 2\n\n[project]\nname = \"demo\"\nauthor = [\"\"]\n",
        "version = 2\n\n[project]\nname = \"demo\"\nrequires-aviutl2 = \"invalid\"\n",
        "version = 2\n\n[project]\nname = \"${{ UNKNOWN }}\"\n",
        "version = 2\n\n[project]\nname = \"${{ UNKNOWN\"\n",
        "version = 2\n\n[project]\nname = \"demo\"\n\n[variables]\nA = \"${{ B }}\"\nB = \"${{ A }}\"\n",
        "version = 2\n\n[project]\nname = \"demo\"\n\n[variables]\nBUILD_DIR = \"other\"\n",
        "version = 2\n\n[project]\nname = \"demo\"\n\n[variables]\nVALUE = \"one\"\nVALUE = \"two\"\n",
        "version = 2\n\n[project]\nname = \"demo\"\nunknown = true\n",
        "version = 2\n\n[project]\nname = \"demo\"\n\n[astra]\nrequires-astra = \"invalid\"\n",
        "version = 2\n\n[project]\nname = \"demo\"\n\n[astra]\nrequires-astra = \">=999.0.0\"\n",
        "version = 2\n\n[project]\nname = \"demo\"\n\n[defaults.run]\nshell = \"\"\n",
    ] {
        assert!(load_config(source).is_err(), "{source}");
    }

    for field in ["build-dir", "dist-dir"] {
        let source = format!("version = 2\n\n[project]\nname = \"demo\"\n\n[astra]\n{field} = \"\"\n");
        assert!(load_config(&source).is_err(), "{field}");
    }

    for id in [".astra", ".astra-cache", ".astratest", ".git", ".github", ".gitignore"] {
        for section in [
            format!("[tasks.\"{id}\"]\nrun = \"echo\""),
            format!("[build.\"{id}\"]\nsuffix = \".anm2\""),
            format!("[release.\"{id}\"]"),
        ] {
            let source = format!("version = 2\n\n[project]\nname = \"demo\"\n\n{section}\n");
            assert!(
                load_config(&source).unwrap_err().to_string().contains("reserved"),
                "{id}"
            );
        }

        let source = format!("version = 2\n\n[project]\nname = \"demo\"\n\n[release.main.package]\nid = \"{id}\"\n");
        assert!(
            load_config(&source).unwrap_err().to_string().contains("reserved"),
            "{id}"
        );
    }

    assert!(load_config("version = 2\n\n[project]\nname = \"demo\"\n").is_ok());
    assert!(
        load_config_with(
            "version = 2\n\n[project]\nname = \"demo\"\n",
            IndexMap::from([("BUILD_DIR".to_owned(), "other".to_owned())]),
            None,
        )
        .is_err()
    );

    let config = load_config(
        r#"
version = 2

[project]
name = "demo"

[build.bad_missing_suffix]
targets = [{ path = "src/main.lua" }]

[build.bad_suffix]
suffix = ".bad"

[build.bad_name]
suffix = ".anm2"
name = "../name"

[build.bad_newline]
suffix = ".anm2"
newline = "unknown"

[build.bad_legacy_newline]
suffix = ".anm"
newline = "lf"

[build.bad_mode]
suffix = ".anm2"
enabled = "other"

[build.bad_include]
suffix = ".anm2"
include-dirs = [""]

[build.bad_artifact]
suffix = ".anm2"
outputs = [""]

[build.bad_vars]
suffix = ".anm2"
variables = { BUILD_DIR = "other" }

[build.bad_target]
suffix = ".anm2"

[[build.bad_target.target]]
name = ""
path = ""
include-dirs = [""]
encoding = "unknown"

[tasks.empty_run]
run = []

[tasks.empty_command]
run = [""]

[tasks.empty_shell]
shell = ""
run = ["echo"]

[tasks.empty_dependency]
run = ["echo"]
depends = [{ task = "" }]

[tasks.reserved_vars]
run = ["echo"]
variables = { BUILD_DIR = "other" }

[release.reserved_vars]
variables = { BUILD_DIR = "other" }
"#,
    )?;
    for id in [
        "bad_missing_suffix",
        "bad_suffix",
        "bad_name",
        "bad_newline",
        "bad_legacy_newline",
        "bad_mode",
        "bad_include",
        "bad_artifact",
        "bad_vars",
    ] {
        assert!(config.build(id, BuildType::Debug).is_err(), "{id}");
    }
    assert!(config.build("missing", BuildType::Debug).is_err());
    assert!(config.release("missing").is_err());
    assert!(
        config
            .build("bad_target", BuildType::Debug)?
            .targets()
            .next()
            .unwrap()
            .is_err()
    );
    for id in ["empty_run", "empty_command", "empty_shell", "empty_dependency"] {
        assert!(config.task(&TaskCall::Simple(id.to_owned())).is_err(), "{id}");
    }
    assert!(config.task(&TaskCall::Simple("reserved_vars".to_owned())).is_err());
    assert!(config.release("reserved_vars").is_err());

    assert!(config.task(&TaskCall::Simple("missing".to_owned())).is_err());
    assert!(TaskCall::Simple(String::new()).validate().is_err());
    assert!(
        TaskCall::Detailed {
            task: "task".to_owned(),
            env: IndexMap::new(),
            vars: IndexMap::from([("BUILD_DIR".to_owned(), "other".to_owned())]),
        }
        .validate()
        .is_err()
    );
    assert!(toml::from_str::<BuildCall>("build = \"\"")?.validate().is_err());

    for (field, value) in [
        ("changelog", "\"\""),
        ("filename", "\"bad/path.md\""),
        ("encoding", "\"unknown\""),
    ] {
        let notes = if field == "changelog" {
            format!("{field} = {value}")
        } else {
            format!("changelog = \"changes\"\n{field} = {value}")
        };
        let source = format!("version = 2\n\n[project]\nname = \"demo\"\n\n[release.bad.notes]\n{notes}\n");
        assert!(load_config(&source)?.release("bad").is_err(), "{field}");
    }

    for filename in ["", ".zip", "demo.tar.gz", " demo.zip", "demo.zip ", "../demo.zip"] {
        let source =
            format!("version = 2\n\n[project]\nname = \"demo\"\n\n[release.bad.package]\nfilename = \"{filename}\"\n");
        assert!(load_config(&source)?.release("bad").is_err(), "{filename:?}");
    }

    let config = load_config(
        r#"
version = 2

[project]
name = "demo"

[release.bad.package]
filename = "demo.zip"

[[release.bad.package.content]]
dir = ""
source = "file.txt"

[[release.bad.package.content]]
dir = " "
source = "file.txt"

[[release.bad.package.content]]
dir = "../outside"
source = "file.txt"

[[release.bad.package.content]]
dir = "nested/../outside"
source = "file.txt"

[[release.bad.package.content]]
dir = "/outside"
source = "file.txt"

[[release.bad.package.content]]
dir = "arbitrary/path"
source = "file.txt"

[[release.bad.package.content]]
dir = "."
source = "file.txt"
"#,
    )?;
    let release = config.release("bad")?;
    let results = release.package().unwrap().contents().collect::<Vec<_>>();
    assert_eq!(results.len(), 7);
    assert!(results[..5].iter().all(|result| result.is_err()));
    assert!(results[5..].iter().all(|result| result.is_ok()));

    let config = load_config(
        "version = 2\n\n[project]\nname = \"demo\"\n\n\
         [release.bad.package]\nfilename = \"demo.au2pkg.zip\"\n\n\
         [[release.bad.package.content]]\ndir = \"Invalid/Plugin\"\nsource = \"file.txt\"\n",
    )?;
    assert!(
        config
            .release("bad")?
            .package()
            .unwrap()
            .contents()
            .next()
            .unwrap()
            .is_err()
    );

    let config = load_config(
        r#"
version = 2

[project]
name = "demo"

[release.bad.package]
filename = "demo.zip"

[[release.bad.package.content]]
dir = "Plugin"
source = { url = "ftp://example.com/archive.zip" }

[[release.bad.package.content]]
dir = "Plugin"
source = { url = "not a url" }

[[release.bad.package.content]]
dir = "Plugin"
source = { url = "https://example.com/archive.zip", pick = "*.txt" }

[[release.bad.package.content]]
dir = "Plugin"
source = { build = "" }

[[release.bad.package.content]]
dir = "Plugin"
source = { path = "" }

[[release.bad.package.content]]
dir = "Plugin"
source = { filename = "../bad.lua", content = "x", encoding = "unknown", newline = "unknown" }
"#,
    )?;
    let release = config.release("bad")?;
    let results = release.package().unwrap().contents().collect::<Vec<_>>();
    assert!(results.iter().all(|result| result.is_err()));
    assert!(PackageSource::Simple("file.txt".to_owned()).validate().is_err());
    assert!(ReleaseDependency::Simple("clean".to_owned()).validate().is_err());
    Ok(())
}
