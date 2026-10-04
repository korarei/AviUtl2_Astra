use crate::vars::{self, Scope, Vars};
use anyhow::{Context, bail};
use indexmap::IndexMap;
use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Serialize};
use serde_with::{
    DeserializeAs, MapPreventDuplicates, OneOrMany, PickFirst, SerializeAs, schemars_1::JsonSchemaAs, serde_as,
};
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::marker::PhantomData;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, LazyLock};
use validator::{Validate, ValidationError, ValidationErrors, ValidationErrorsKind};

static EMPTY_VARS: LazyLock<IndexMap<String, String>> = LazyLock::new(IndexMap::new);

pub(crate) const RESERVED_VARIABLES: &[&str] = &[
    "PROJECT_NAME",
    "PROJECT_VERSION",
    "PROJECT_AUTHOR",
    "PROJECT_AUTHORS",
    "AVIUTL2_VERSION",
    "BUILD_TYPE",
    "BUILD_DIR",
    "BUILD_DIRECTORY",
    "DEBUG",
    "NDEBUG",
    "SCRIPT_NAME",
];

#[serde_as]
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(alias = "config_version", alias = "config-version")]
    version: u32,
    #[serde_as(as = "MapPreventDuplicates<_, _>")]
    #[serde(default, alias = "environment")]
    env: IndexMap<String, String>,
    #[serde_as(as = "MapPreventDuplicates<_, _>")]
    #[serde(default, alias = "variables")]
    vars: IndexMap<String, String>,
    #[serde(default, alias = "default")]
    defaults: Defaults,
    #[serde(default)]
    astra: Astra,
    project: Project,
    #[serde_as(as = "MapPreventDuplicates<_, _>")]
    #[serde(default, alias = "task")]
    tasks: BTreeMap<String, Task>,
    #[serde_as(as = "MapPreventDuplicates<_, _>")]
    #[serde(default, alias = "build")]
    builds: IndexMap<String, Build>,
    #[serde_as(as = "MapPreventDuplicates<_, _>")]
    #[serde(default, alias = "release")]
    releases: IndexMap<String, Release>,
    #[serde(skip)]
    path: PathBuf,
    #[serde(skip)]
    scope: Scope,
    #[serde(skip)]
    defines: IndexMap<String, String>,
    #[serde(skip)]
    dotenv: IndexMap<String, String>,
    #[serde(skip)]
    project_version: Option<String>,
}

impl Config {
    pub(crate) fn find_path() -> anyhow::Result<PathBuf> {
        std::env::current_dir()?
            .ancestors()
            .map(|dir| dir.join("astra.toml"))
            .find(|path| path.is_file())
            .ok_or_else(|| anyhow::anyhow!("could not find `astra.toml` in current or any parent directory"))
    }

    pub fn load<P: AsRef<Path>>(file: P, defines: IndexMap<String, String>, ver: Option<&str>) -> anyhow::Result<Self> {
        if let Some(key) = defines.keys().find(|key| RESERVED_VARIABLES.contains(&key.as_str())) {
            bail!("cannot define reserved variable '{key}'");
        }

        let file = file.as_ref();
        let ver = ver.map(str::trim).map(str::to_owned);
        let mut config: Self = toml::from_str(
            &std::fs::read_to_string(file).with_context(|| format!("failed to read '{}'", file.display()))?,
        )
        .with_context(|| format!("failed to parse '{}'", file.display()))?;

        let dotenv = file.with_file_name(".env");
        match dotenvy::from_path_iter(&dotenv) {
            Ok(env) => {
                config.dotenv = env
                    .collect::<Result<_, _>>()
                    .with_context(|| format!("failed to parse '{}'", dotenv.display()))?;
            }
            Err(dotenvy::Error::Io(err)) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(err).with_context(|| format!("failed to read '{}'", dotenv.display())),
        }

        if config.version < 2 {
            bail!("configuration version must be at least 2, got {}", config.version);
        }

        if let Some(ver) = &ver {
            config.project.version = Some(ver.clone());
        }

        let scope = expand_map(&mut config.vars, &Scope::new(defines.clone()))?;
        config.env.values_mut().try_for_each(|val| -> anyhow::Result<()> {
            *val = vars::expand(val, &scope)?;
            Ok(())
        })?;

        config.defaults.expanded(&scope)?;
        if let Err(err) = config.defaults.validate() {
            bail!("configuration validation failed:\n{err}");
        }

        config.astra.expanded(&scope)?;
        if let Err(err) = config.astra.validate() {
            bail!("configuration validation failed:\n{err}");
        }

        config.astra.verify(file)?;

        if let Some(ver) = config.astra.requires_astra.as_deref() {
            static CURRENT_VERSION: LazyLock<semver::Version> =
                LazyLock::new(|| semver::Version::parse(env!("CARGO_PKG_VERSION")).unwrap());

            if !semver::VersionReq::parse(ver)?.matches(&CURRENT_VERSION) {
                bail!(
                    "current astra version ({}) does not satisfy requirement '{ver}'",
                    *CURRENT_VERSION
                );
            }
        }

        config.project.expanded(&scope)?;
        if let Err(err) = config.project.validate() {
            bail!("configuration validation failed:\n{err}");
        }

        config.normalize()?;

        let mut builtins = IndexMap::new();
        builtins.insert("PROJECT_NAME".to_string(), config.project.name.clone());

        if let Some(ver) = config.project.version() {
            builtins.insert("PROJECT_VERSION".to_string(), ver.to_string());
        }

        if !config.project.authors().is_empty() {
            let authors = config.project.authors().join(", ");
            builtins.insert("PROJECT_AUTHOR".to_string(), authors.clone());
            builtins.insert("PROJECT_AUTHORS".to_string(), authors);
        }

        if let Some(ver) = config.project.requires_aviutl2() {
            builtins.insert("AVIUTL2_VERSION".to_string(), ver.to_string());
        }

        file.clone_into(&mut config.path);
        config.scope = scope.set(Arc::new(builtins));
        config.defines = defines;
        config.project_version = ver;

        Ok(config)
    }

    #[cfg_attr(not(windows), allow(dead_code))]
    pub(crate) fn reload(&self) -> anyhow::Result<Self> {
        Self::load(&self.path, self.defines.clone(), self.project_version.as_deref())
    }

    #[must_use]
    pub fn astra(&self) -> &Astra {
        &self.astra
    }

    #[cfg_attr(not(windows), allow(dead_code))]
    pub(crate) fn project(&self) -> &Project {
        &self.project
    }

    pub fn build(&self, id: &str, build_type: BuildType) -> anyhow::Result<Build> {
        let mut build = self
            .builds
            .get(id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("build ID '{id}' not found"))?;

        let dir = Path::new(self.astra.build_dir())
            .join(id)
            .to_string_lossy()
            .into_owned();

        #[cfg(windows)]
        let dir = dir.replace('/', "\\");

        let vars = IndexMap::from([
            ("BUILD_TYPE".to_owned(), build_type.as_lowercase().to_owned()),
            (
                match build_type {
                    BuildType::Debug => "DEBUG".to_owned(),
                    BuildType::Release => "NDEBUG".to_owned(),
                },
                "1".to_owned(),
            ),
            ("BUILD_DIR".to_owned(), dir.clone()),
            ("BUILD_DIRECTORY".to_owned(), dir),
        ]);

        build.expanded(&self.scope.set(Arc::new(vars.clone())))?;

        if let Err(err) = build.validate() {
            bail!("configuration validation failed:\n{err}");
        }

        for call in build.depends.iter_mut().chain(&mut build.finally) {
            call.append(&vars);
        }

        Ok(build)
    }

    pub fn builds(&self, build_type: BuildType) -> impl ExactSizeIterator<Item = anyhow::Result<Build>> {
        self.builds.keys().map(move |id| self.build(id, build_type))
    }

    pub fn build_dirs(&self) -> impl ExactSizeIterator<Item = PathBuf> + '_ {
        let base = Path::new(self.astra.build_dir());
        self.builds.keys().map(move |id| base.join(id))
    }

    pub fn release(&self, id: &str) -> anyhow::Result<Release> {
        let mut release = self
            .releases
            .get(id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("release ID '{id}' not found"))?;

        release.expanded(&self.scope)?;

        if let Err(err) = release.validate() {
            bail!("configuration validation failed:\n{err}");
        }

        Ok(release)
    }

    pub fn releases(&self) -> impl ExactSizeIterator<Item = anyhow::Result<Release>> {
        self.releases.keys().map(move |id| self.release(id))
    }

    pub fn release_dirs(&self) -> impl ExactSizeIterator<Item = PathBuf> + '_ {
        let base = Path::new(self.astra.dist_dir());
        self.releases.keys().map(move |id| base.join(id))
    }

    pub fn task(&self, call: &TaskCall) -> anyhow::Result<Task> {
        let mut task = self
            .tasks
            .get(call.id())
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("task ID '{}' not found", call.id()))?;

        task.expanded(&self.scope.set(Arc::new(call.vars().clone())))?;

        if task.shell.is_none() {
            task.shell.clone_from(&self.defaults.run.shell);
        }

        task.env = self
            .env
            .iter()
            .chain(&task.env)
            .chain(call.env())
            .chain(&self.dotenv)
            .map(|(k, v)| {
                (
                    if cfg!(windows) {
                        k.to_ascii_uppercase()
                    } else {
                        k.clone()
                    },
                    v.clone(),
                )
            })
            .collect();

        if let Err(err) = task.validate() {
            bail!("configuration validation failed:\n{err}");
        }

        Ok(task)
    }

    fn normalize(&mut self) -> anyhow::Result<()> {
        static RESERVED_ID_PREFIXES: [&str; 2] = [".astra", ".git"];

        for (id, build) in &mut self.builds {
            if id.is_empty() {
                bail!("build ID cannot be empty");
            }

            if RESERVED_ID_PREFIXES.iter().any(|prefix| id.starts_with(prefix)) {
                bail!("build ID '{id}' is reserved");
            }

            if validate_filename(id).is_err() {
                bail!("build ID '{id}' must be a single relative path component");
            }

            build.id.clone_from(id);
            build.name.get_or_insert_with(|| self.project.name.clone());
        }

        for (id, release) in &mut self.releases {
            if id.is_empty() {
                bail!("release ID cannot be empty");
            }

            if RESERVED_ID_PREFIXES.iter().any(|prefix| id.starts_with(prefix)) {
                bail!("release ID '{id}' is reserved");
            }

            if validate_filename(id).is_err() {
                bail!("release ID '{id}' must be a single relative path component");
            }

            release.id.clone_from(id);

            if let Some(package) = &mut release.package {
                package.id.get_or_insert_with(|| self.project.name.clone());
                if let Some(package_id) = package.id.as_deref()
                    && RESERVED_ID_PREFIXES.iter().any(|prefix| package_id.starts_with(prefix))
                {
                    bail!("package ID '{package_id}' is reserved");
                }

                package.name.get_or_insert_with(|| self.project.name.clone());
                package
                    .filename
                    .get_or_insert_with(|| format!("{}.au2pkg.zip", self.project.name));

                if package.version.is_none() {
                    package.version.clone_from(&self.project.version);
                }

                if package.authors.is_none() && !self.project.authors.is_empty() {
                    package.authors = Some(self.project.authors.clone());
                }
            }
        }

        for (id, task) in &mut self.tasks {
            if id.is_empty() {
                bail!("task ID cannot be empty");
            }

            if RESERVED_ID_PREFIXES.iter().any(|prefix| id.starts_with(prefix)) {
                bail!("task ID '{id}' is reserved");
            }

            task.id.clone_from(id);
        }

        Ok(())
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, Validate, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Defaults {
    #[serde(default)]
    #[validate(nested)]
    run: Run,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, Validate, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Run {
    #[validate(length(min = 1))]
    shell: Option<String>,
}

impl Defaults {
    pub(crate) fn expanded(&mut self, scope: &Scope) -> anyhow::Result<()> {
        if let Some(shell) = &mut self.run.shell {
            *shell = vars::expand(shell, scope)?;
        }

        Ok(())
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, Validate, JsonSchema)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_field_names)]
pub struct Astra {
    #[validate(length(min = 1), custom(function = "validate_requires_astra"))]
    #[serde(alias = "requires-astra")]
    requires_astra: Option<String>,
    #[validate(nested)]
    #[serde(default)]
    run: AstraRun,
    #[validate(length(min = 1))]
    #[serde(alias = "build-dir", alias = "build_directory", alias = "build-directory")]
    build_dir: Option<String>,
    #[validate(length(min = 1))]
    #[serde(
        alias = "dist-dir",
        alias = "dist_directory",
        alias = "dist-directory",
        alias = "distribution_dir",
        alias = "distribution-dir",
        alias = "distribution_directory",
        alias = "distribution-directory"
    )]
    dist_dir: Option<String>,
}

impl Astra {
    pub(crate) fn expanded(&mut self, scope: &Scope) -> anyhow::Result<()> {
        if let Some(ver) = &mut self.requires_astra {
            *ver = vars::expand(ver, scope)?;
        }

        if let Some(id) = &mut self.run.release {
            *id = vars::expand(id, scope)?;
        }

        if let Some(dir) = &mut self.build_dir {
            *dir = convert_path(vars::expand(dir, scope)?);
        }

        if let Some(dir) = &mut self.dist_dir {
            *dir = convert_path(vars::expand(dir, scope)?);
        }

        Ok(())
    }

    #[must_use]
    pub fn build_dir(&self) -> &str {
        self.build_dir.as_deref().unwrap_or("build")
    }

    #[must_use]
    pub fn dist_dir(&self) -> &str {
        self.dist_dir.as_deref().unwrap_or("dist")
    }

    #[must_use]
    pub fn run(&self) -> &AstraRun {
        &self.run
    }

    pub(crate) fn verify(&self, file: &Path) -> anyhow::Result<()> {
        let root = crate::fs::to_key(
            file.parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .unwrap_or(Path::new(".")),
        )?;

        let check_dir = |dir: &str, kind: &str| -> anyhow::Result<()> {
            let target = crate::fs::to_key(&root.join(dir))?;
            if root.starts_with(&target) {
                bail!("{kind} directory must not resolve to the project root or one of its parents: '{dir}'");
            }

            if target.starts_with(crate::fs::to_key(&root.join(".astra"))?) {
                bail!("{kind} directory must not be '.astra' or one of its children: '{dir}'");
            }

            Ok(())
        };

        check_dir(self.build_dir(), "build")?;
        check_dir(self.dist_dir(), "dist")?;

        Ok(())
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, Validate, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AstraRun {
    #[validate(length(min = 1))]
    release: Option<String>,
}

impl AstraRun {
    #[must_use]
    pub fn release_id(&self) -> Option<&str> {
        self.release.as_deref()
    }
}

#[serde_as]
#[derive(Debug, Clone, Serialize, Deserialize, Validate, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct Project {
    #[validate(length(min = 1))]
    name: String,
    #[validate(length(min = 1))]
    version: Option<String>,
    #[serde_as(as = "ManyOrOne<_>")]
    #[serde(default, alias = "author")]
    #[validate(custom(function = "validate_strings"))]
    authors: Vec<String>,
    #[serde(alias = "requires-aviutl2")]
    requires_aviutl2: Option<AviUtl2Version>,
    #[serde(skip)]
    resolved_requires_aviutl2: Option<u32>,
}

impl Project {
    pub(crate) fn expanded(&mut self, scope: &Scope) -> anyhow::Result<()> {
        self.name = vars::expand(&self.name, scope)?;

        if let Some(ver) = &mut self.version {
            *ver = vars::expand(ver, scope)?;
        }

        self.authors.iter_mut().try_for_each(|author| -> anyhow::Result<()> {
            *author = vars::expand(author, scope)?;
            Ok(())
        })?;

        if let Some(AviUtl2Version::String(ver)) = &mut self.requires_aviutl2 {
            *ver = vars::expand(ver, scope)?;
        }

        self.resolved_requires_aviutl2 = self
            .requires_aviutl2
            .as_ref()
            .map(AviUtl2Version::resolve)
            .transpose()?;

        Ok(())
    }

    #[must_use]
    pub(crate) fn version(&self) -> Option<&str> {
        self.version.as_deref()
    }

    #[must_use]
    pub(crate) fn authors(&self) -> &[String] {
        &self.authors
    }

    #[must_use]
    pub(crate) fn requires_aviutl2(&self) -> Option<u32> {
        self.resolved_requires_aviutl2
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum AviUtl2Version {
    Number(u32),
    String(String),
}

impl AviUtl2Version {
    pub fn resolve(&self) -> anyhow::Result<u32> {
        match self {
            Self::Number(n) => Ok(*n),
            Self::String(s) => crate::aviutl2_version::resolve(s),
        }
    }
}

#[serde_as]
#[derive(Debug, Clone, Serialize, Deserialize, Validate, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Task {
    #[serde(default, alias = "hidden")]
    hide: bool,
    #[validate(length(min = 1))]
    shell: Option<String>,
    #[serde_as(as = "ManyOrOne<_>")]
    #[validate(custom(function = "validate_run"))]
    run: Vec<String>,
    #[validate(nested)]
    #[serde_as(as = "ManyOrOne<_>")]
    #[serde(default, alias = "dependencies")]
    depends: Vec<TaskCall>,
    #[validate(nested)]
    #[serde_as(as = "ManyOrOne<_>")]
    #[serde(default)]
    finally: Vec<TaskCall>,
    #[serde(default, alias = "environment")]
    #[serde_as(as = "MapPreventDuplicates<_, _>")]
    env: IndexMap<String, String>,
    #[serde(default, alias = "variables")]
    #[serde_as(as = "MapPreventDuplicates<_, _>")]
    vars: IndexMap<String, String>,
    #[serde(skip)]
    id: String,
}

impl Task {
    pub(crate) fn expanded(&mut self, parent: &Scope) -> anyhow::Result<()> {
        let scope = expand_map(&mut self.vars, parent)?;

        self.env.values_mut().try_for_each(|val| -> anyhow::Result<()> {
            *val = vars::expand(val, &scope)?;
            Ok(())
        })?;

        if let Some(shell) = &mut self.shell {
            *shell = vars::expand(shell, &scope)?;
        }

        self.run.iter_mut().try_for_each(|cmd| -> anyhow::Result<()> {
            *cmd = vars::expand(cmd, &scope)?;
            Ok(())
        })?;

        self.depends.iter_mut().try_for_each(|call| call.expanded(&scope))?;
        self.finally.iter_mut().try_for_each(|call| call.expanded(&scope))?;

        Ok(())
    }

    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    #[must_use]
    pub fn hide(&self) -> bool {
        self.hide
    }

    #[must_use]
    pub fn shell(&self) -> Option<&str> {
        self.shell.as_deref()
    }

    #[must_use]
    pub fn run(&self) -> &[String] {
        &self.run
    }

    #[must_use]
    pub fn depends(&self) -> &[TaskCall] {
        &self.depends
    }

    #[must_use]
    pub fn finally(&self) -> &[TaskCall] {
        &self.finally
    }

    #[must_use]
    pub(crate) fn env(&self) -> &IndexMap<String, String> {
        &self.env
    }
}

#[serde_as]
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[serde(untagged)]
pub enum TaskCall {
    Simple(String),
    Detailed {
        task: String,
        #[serde(default, alias = "environment")]
        #[serde_as(as = "MapPreventDuplicates<_, _>")]
        env: IndexMap<String, String>,
        #[serde(default, alias = "variables")]
        #[serde_as(as = "MapPreventDuplicates<_, _>")]
        vars: IndexMap<String, String>,
    },
}

impl TaskCall {
    pub(crate) fn expanded(&mut self, scope: &Scope) -> anyhow::Result<()> {
        match self {
            Self::Simple(raw) => {
                let s = vars::expand(raw, scope)?;
                *raw = s.strip_prefix("task:").unwrap_or(&s).to_string();
            }
            Self::Detailed { task, env, vars } => {
                *task = vars::expand(task, scope)?;
                let var_scope = expand_map(vars, scope)?;
                env.values_mut().try_for_each(|val| -> anyhow::Result<()> {
                    *val = vars::expand(val, &var_scope)?;
                    Ok(())
                })?;
            }
        }

        Ok(())
    }

    pub(crate) fn append(&mut self, vars: &IndexMap<String, String>) {
        match self {
            Self::Simple(task) => {
                let task = std::mem::take(task);
                *self = Self::Detailed {
                    task,
                    env: IndexMap::new(),
                    vars: vars.clone(),
                };
            }
            Self::Detailed { vars: task_vars, .. } => task_vars.extend(vars.clone()),
        }
    }

    #[must_use]
    pub fn id(&self) -> &str {
        match self {
            Self::Simple(task) | Self::Detailed { task, .. } => task,
        }
    }

    #[must_use]
    pub(crate) fn env(&self) -> &IndexMap<String, String> {
        match self {
            Self::Detailed { env, .. } => env,
            Self::Simple(_) => &EMPTY_VARS,
        }
    }

    #[must_use]
    pub(crate) fn vars(&self) -> &IndexMap<String, String> {
        match self {
            Self::Detailed { vars, .. } => vars,
            Self::Simple(_) => &EMPTY_VARS,
        }
    }
}

impl Validate for TaskCall {
    fn validate(&self) -> Result<(), ValidationErrors> {
        let mut errors = ValidationErrors::new();
        let id = self.id();
        if id.is_empty() {
            errors.add("task", ValidationError::new("length"));
        }

        if let Self::Detailed { vars, .. } = self
            && let Some(key) = vars.keys().find(|key| RESERVED_VARIABLES.contains(&key.as_str()))
        {
            errors.add(
                "vars",
                ValidationError::new("reserved").with_message(format!("variable '{key}' is reserved").into()),
            );
        }

        if errors.is_empty() { Ok(()) } else { Err(errors) }
    }
}

#[serde_as]
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Build {
    enabled: Option<BuildEnabled>,
    name: Option<String>,
    suffix: Option<String>,
    newline: Option<String>,
    #[serde_as(as = "ManyOrOne<_>")]
    #[serde(default, alias = "dependencies")]
    depends: Vec<TaskCall>,
    #[serde_as(as = "ManyOrOne<_>")]
    #[serde(default)]
    finally: Vec<TaskCall>,
    #[serde(default, alias = "variables")]
    #[serde_as(as = "MapPreventDuplicates<_, _>")]
    vars: IndexMap<String, String>,
    #[serde_as(as = "ManyOrOne<_>")]
    #[serde(
        default,
        alias = "include-dirs",
        alias = "include_directories",
        alias = "include-directories"
    )]
    include_dirs: Vec<String>,
    #[serde_as(as = "Option<ManyOrOne<_>>")]
    #[serde(default, alias = "target")]
    targets: Option<Vec<BuildTarget>>,
    #[serde_as(as = "ManyOrOne<_>")]
    #[serde(default, alias = "artifact", alias = "outputs", alias = "output")]
    artifacts: Vec<String>,
    #[serde(skip)]
    id: String,
    #[serde(skip)]
    scope: Scope,
}

impl Build {
    pub(crate) fn expanded(&mut self, parent: &Scope) -> anyhow::Result<()> {
        let scope = expand_map(&mut self.vars, parent)?;

        if let Some(enabled) = &mut self.enabled {
            enabled.expanded(&scope)?;
        }

        if let Some(name) = &mut self.name {
            *name = vars::expand(name, &scope)?;
        }

        if let Some(newline) = &mut self.newline {
            *newline = vars::expand(newline, &scope)?;
        }

        if let Some(suffix) = &mut self.suffix {
            let expanded = vars::expand(suffix, &scope)?;
            let expanded = expanded.trim();

            *suffix = if expanded.starts_with('.') {
                expanded.to_string()
            } else {
                format!(".{expanded}")
            };
        }

        for call in self.depends.iter_mut().chain(&mut self.finally) {
            call.expanded(&scope)?;
        }

        for path in &mut self.include_dirs {
            *path = convert_path(vars::expand(path, &scope)?);
        }

        if !self.artifacts.is_empty() {
            let mut vars = IndexMap::new();
            for key in ["BUILD_DIR", "BUILD_DIRECTORY"] {
                if let Some(val) = scope.get(key) {
                    let val = if cfg!(windows) {
                        Cow::Owned(val.replace('\\', "/"))
                    } else {
                        Cow::Borrowed(val)
                    };

                    if val.contains('\\') {
                        bail!("variable '{key}' must not contain backslashes when expanding artifacts");
                    }

                    vars.insert(key.to_owned(), val.into_owned());
                }
            }

            let artifact_scope = scope.set(Arc::new(vars));
            for pattern in &mut self.artifacts {
                *pattern = vars::expand(pattern, &artifact_scope)?;
            }
        }

        self.scope = scope;

        Ok(())
    }

    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    #[must_use]
    pub fn name(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.id)
    }

    #[must_use]
    pub fn suffix(&self) -> Option<&str> {
        self.suffix.as_deref()
    }

    #[must_use]
    pub fn newline(&self) -> &'static str {
        self.newline.as_deref().and_then(parse_newline).unwrap_or("\r\n")
    }

    #[must_use]
    pub fn encoding(&self) -> &'static encoding_rs::Encoding {
        if self.suffix.as_deref().is_some_and(|s| s.ends_with('2')) {
            encoding_rs::UTF_8
        } else {
            encoding_rs::SHIFT_JIS
        }
    }

    #[must_use]
    pub fn is_enabled(&self, build_type: BuildType) -> bool {
        self.enabled
            .as_ref()
            .is_none_or(|enabled| enabled.is_enabled(build_type))
    }

    #[must_use]
    pub fn depends(&self) -> &[TaskCall] {
        &self.depends
    }

    #[must_use]
    pub fn finally(&self) -> &[TaskCall] {
        &self.finally
    }

    #[must_use]
    pub fn include_dirs(&self) -> &[String] {
        &self.include_dirs
    }

    pub fn targets(&self) -> impl ExactSizeIterator<Item = anyhow::Result<BuildTarget>> {
        let targets = self.targets.as_deref().unwrap_or_default();
        let is_bundled = targets.len() > 1;

        targets.iter().cloned().map(move |mut target| {
            target.expanded(&self.scope)?;
            if !self
                .suffix
                .as_deref()
                .is_some_and(|suffix| suffix.eq_ignore_ascii_case(".aul2"))
            {
                let key = if is_bundled {
                    format!("{}@{}", target.name(), self.name())
                } else {
                    self.name().to_owned()
                };
                target.vars.insert("SCRIPT_NAME".to_owned(), key.clone());
                target.key = key;
            }

            if let Err(err) = target.validate() {
                bail!("configuration validation failed:\n{err}");
            }

            Ok(target)
        })
    }

    #[must_use]
    pub fn artifacts(&self) -> &[String] {
        &self.artifacts
    }
}

impl Validate for Build {
    fn validate(&self) -> Result<(), ValidationErrors> {
        static VALID_SUFFIXES: [&str; 11] = [
            ".anm2", ".tra2", ".obj2", ".scn2", ".cam2", ".aul2", ".anm", ".tra", ".obj", ".scn", ".cam",
        ];
        static VALID_MODES: [&str; 3] = ["all", "debug", "release"];

        let mut errors = ValidationErrors::new();

        if let Some(name) = &self.name {
            if name.contains('\\') {
                errors.add(
                    "name",
                    ValidationError::new("invalid_filename")
                        .with_message("must be a single file name without path separators".into()),
                );
            } else if let Err(error) = validate_filename(name) {
                errors.add("name", error);
            }
        }

        match &self.suffix {
            Some(suffix) => {
                if !VALID_SUFFIXES.iter().any(|&ext| suffix.eq_ignore_ascii_case(ext)) {
                    errors.add(
                        "suffix",
                        ValidationError::new("invalid_suffix").with_message(
                            "suffix must be one of .anm2, .tra2, .obj2, .scn2, .cam2, .aul2, \
                            .anm, .tra, .obj, .scn, .cam"
                                .into(),
                        ),
                    );
                }
            }
            None => {
                if self.targets.as_ref().is_some_and(|targets| !targets.is_empty()) {
                    errors.add(
                        "suffix",
                        ValidationError::new("missing_suffix")
                            .with_message("suffix is required when targets are specified".into()),
                    );
                }
            }
        }

        if let Some(nl) = &self.newline {
            match parse_newline(nl) {
                Some(parsed) => {
                    if !self.suffix.as_deref().is_some_and(|s| s.ends_with('2')) && parsed != "\r\n" {
                        errors.add(
                            "newline",
                            ValidationError::new("invalid_newline")
                                .with_message("newline must be crlf for legacy ExEdit scripts".into()),
                        );
                    }
                }
                None => {
                    errors.add(
                        "newline",
                        ValidationError::new("invalid_newline")
                            .with_message("newline must be one of \\r\\n, crlf, \\n, lf".into()),
                    );
                }
            }
        }

        if let Some(BuildEnabled::Mode(mode)) = &self.enabled {
            let mode = mode.trim();
            if !VALID_MODES.iter().any(|&m| mode.eq_ignore_ascii_case(m)) {
                errors.add(
                    "enabled",
                    ValidationError::new("invalid_mode")
                        .with_message(format!("enabled must be one of all, debug, or release, got '{mode}'").into()),
                );
            }
        }

        for (field, calls) in [("depends", &self.depends), ("finally", &self.finally)] {
            for (i, call) in calls.iter().enumerate() {
                if let Err(e) = call.validate() {
                    errors.errors_mut().insert(
                        Cow::Owned(format!("{field}[{i}]")),
                        ValidationErrorsKind::Struct(Box::new(e)),
                    );
                }
            }
        }

        for (field, list) in [("include_dirs", &self.include_dirs), ("artifacts", &self.artifacts)] {
            if let Err(e) = validate_strings(list) {
                errors.add(field, e);
            }
        }

        for pattern in &self.artifacts {
            if let Err(e) = validate_glob(pattern) {
                errors.add("artifacts", e);
            }
        }

        if errors.is_empty() { Ok(()) } else { Err(errors) }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BuildType {
    Debug,
    Release,
}

impl BuildType {
    #[must_use]
    pub fn as_lowercase(&self) -> &'static str {
        match self {
            Self::Debug => "debug",
            Self::Release => "release",
        }
    }
}

impl std::fmt::Display for BuildType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Debug => f.write_str("Debug"),
            Self::Release => f.write_str("Release"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[serde(untagged)]
enum BuildEnabled {
    Boolean(bool),
    Mode(String),
    Detailed {
        #[serde(default)]
        debug: Option<bool>,
        #[serde(default)]
        release: Option<bool>,
    },
}

impl BuildEnabled {
    fn expanded(&mut self, scope: &Scope) -> anyhow::Result<()> {
        if let Self::Mode(val) = self {
            *val = vars::expand(val, scope)?;
        }

        Ok(())
    }

    #[must_use]
    fn is_enabled(&self, build_type: BuildType) -> bool {
        match self {
            Self::Boolean(b) => *b,
            Self::Mode(m) => {
                let mode = m.trim();
                mode.eq_ignore_ascii_case("all") || mode.eq_ignore_ascii_case(build_type.as_lowercase())
            }
            Self::Detailed { debug, release } => match build_type {
                BuildType::Debug => debug.unwrap_or(true),
                BuildType::Release => release.unwrap_or(true),
            },
        }
    }
}

#[serde_as]
#[derive(Debug, Clone, Serialize, Deserialize, Validate, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BuildTarget {
    #[validate(length(min = 1))]
    name: Option<String>,
    #[validate(length(min = 1))]
    path: String,
    #[serde_as(as = "ManyOrOne<_>")]
    #[serde(
        default,
        alias = "include-dirs",
        alias = "include_directories",
        alias = "include-directories"
    )]
    #[validate(custom(function = "validate_strings"))]
    include_dirs: Vec<String>,
    #[serde(default, alias = "variables")]
    #[serde_as(as = "MapPreventDuplicates<_, _>")]
    vars: IndexMap<String, String>,
    #[validate(custom(function = "validate_encoding"))]
    encoding: Option<String>,
    #[serde(skip)]
    key: String,
}

impl BuildTarget {
    pub(crate) fn expanded(&mut self, parent: &Scope) -> anyhow::Result<()> {
        let scope = expand_map(&mut self.vars, parent)?;

        self.path = convert_path(vars::expand(&self.path, &scope)?);

        if let Some(name) = &mut self.name {
            *name = vars::expand(name, &scope)?;
        }

        for dir in &mut self.include_dirs {
            *dir = convert_path(vars::expand(dir, &scope)?);
        }

        if let Some(encoding) = &mut self.encoding {
            *encoding = vars::expand(encoding, &scope)?;
        }

        self.vars = scope.flatten();

        Ok(())
    }

    #[must_use]
    pub fn name(&self) -> &str {
        self.name
            .as_deref()
            .or_else(|| Path::new(&self.path).file_stem().and_then(|s| s.to_str()))
            .unwrap_or("")
    }

    #[must_use]
    pub fn key(&self) -> &str {
        if self.key.is_empty() { self.name() } else { &self.key }
    }

    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    #[must_use]
    pub fn include_dirs(&self) -> &[String] {
        &self.include_dirs
    }

    #[must_use]
    pub fn vars(&self) -> &IndexMap<String, String> {
        &self.vars
    }

    #[must_use]
    pub fn encoding(&self) -> &'static encoding_rs::Encoding {
        self.encoding
            .as_deref()
            .and_then(|s| encoding_rs::Encoding::for_label(s.as_bytes()))
            .unwrap_or(encoding_rs::UTF_8)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BuildCall {
    build: String,
}

impl BuildCall {
    pub(crate) fn expanded(&mut self, scope: &Scope) -> anyhow::Result<()> {
        self.build = vars::expand(&self.build, scope)?;
        Ok(())
    }

    #[must_use]
    pub fn id(&self) -> &str {
        &self.build
    }
}

impl Validate for BuildCall {
    fn validate(&self) -> Result<(), ValidationErrors> {
        if self.id().is_empty() {
            let mut errors = ValidationErrors::new();
            errors.add("build", ValidationError::new("length"));
            return Err(errors);
        }

        Ok(())
    }
}

#[serde_as]
#[derive(Debug, Clone, Serialize, Deserialize, Validate, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Release {
    #[validate(nested)]
    #[serde_as(as = "ManyOrOne<_>")]
    #[serde(default, alias = "dependencies")]
    depends: Vec<ReleaseDependency>,
    #[validate(nested)]
    #[serde_as(as = "ManyOrOne<_>")]
    #[serde(default)]
    finally: Vec<TaskCall>,
    #[serde(default, alias = "variables")]
    #[serde_as(as = "MapPreventDuplicates<_, _>")]
    vars: IndexMap<String, String>,
    #[validate(nested)]
    package: Option<Package>,
    #[validate(nested)]
    #[serde(alias = "release_notes", alias = "release-notes")]
    notes: Option<ReleaseNotes>,
    #[serde(skip)]
    id: String,
}

impl Release {
    pub(crate) fn expanded(&mut self, parent: &Scope) -> anyhow::Result<()> {
        let scope = expand_map(&mut self.vars, parent)?;

        for dep in &mut self.depends {
            dep.expanded(&scope)?;
        }

        for call in &mut self.finally {
            call.expanded(&scope)?;
        }

        if let Some(package) = &mut self.package {
            package.expanded(&scope)?;
        }

        if let Some(notes) = &mut self.notes {
            notes.expanded(&scope)?;
        }

        Ok(())
    }

    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    #[must_use]
    pub fn depends(&self) -> &[ReleaseDependency] {
        &self.depends
    }

    #[must_use]
    pub fn finally(&self) -> &[TaskCall] {
        &self.finally
    }

    #[must_use]
    pub fn package(&self) -> Option<&Package> {
        self.package.as_ref()
    }

    #[must_use]
    pub fn notes(&self) -> Option<&ReleaseNotes> {
        self.notes.as_ref()
    }
}

#[serde_as]
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[serde(untagged)]
pub enum ReleaseDependency {
    Simple(String),
    Build(BuildCall),
    Task(TaskCall),
}

impl ReleaseDependency {
    pub(crate) fn expanded(&mut self, scope: &Scope) -> anyhow::Result<()> {
        match self {
            Self::Simple(s) => {
                let val = vars::expand(s, scope)?;
                *self = if let Some(build) = val.strip_prefix("build:") {
                    Self::Build(BuildCall {
                        build: build.to_string(),
                    })
                } else if let Some(task) = val.strip_prefix("task:") {
                    Self::Task(TaskCall::Simple(task.to_string()))
                } else {
                    Self::Task(TaskCall::Simple(val))
                };
            }
            Self::Build(b) => {
                b.expanded(scope)?;
            }
            Self::Task(c) => {
                c.expanded(scope)?;
            }
        }
        Ok(())
    }
}

impl Validate for ReleaseDependency {
    fn validate(&self) -> Result<(), ValidationErrors> {
        match self {
            Self::Simple(_) => {
                let mut errors = ValidationErrors::new();
                errors.add(
                    "dependency",
                    ValidationError::new("unexpanded")
                        .with_message("release dependency must be expanded before validation".into()),
                );
                Err(errors)
            }
            Self::Build(b) => b.validate(),
            Self::Task(c) => c.validate(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Validate, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReleaseNotes {
    #[validate(length(min = 1))]
    #[serde(alias = "change_log", alias = "change-log")]
    changelog: String,
    #[validate(custom(function = "validate_filename"))]
    filename: Option<String>,
    #[validate(custom(function = "validate_encoding"))]
    encoding: Option<String>,
}

impl ReleaseNotes {
    pub(crate) fn expanded(&mut self, scope: &Scope) -> anyhow::Result<()> {
        self.changelog = convert_path(vars::expand(&self.changelog, scope)?);

        if let Some(filename) = &mut self.filename {
            *filename = vars::expand(filename, scope)?;
        }

        if let Some(encoding) = &mut self.encoding {
            *encoding = vars::expand(encoding, scope)?;
        }

        Ok(())
    }

    #[must_use]
    pub fn changelog(&self) -> &str {
        &self.changelog
    }

    #[must_use]
    pub fn filename(&self) -> &str {
        self.filename.as_deref().unwrap_or("RELEASE_NOTES.md")
    }

    #[must_use]
    pub fn encoding(&self) -> &'static encoding_rs::Encoding {
        self.encoding
            .as_deref()
            .and_then(|s| encoding_rs::Encoding::for_label(s.as_bytes()))
            .unwrap_or(encoding_rs::UTF_8)
    }
}

#[serde_as]
#[derive(Debug, Clone, Default, Serialize, Deserialize, Validate, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Package {
    #[validate(custom(function = "validate_package_filename"))]
    filename: Option<String>,
    #[validate(length(min = 1))]
    id: Option<String>,
    #[validate(length(min = 1))]
    name: Option<String>,
    #[serde(default, alias = "uninstall-subdirectory-files")]
    uninstall_subdirectory_files: bool,
    #[validate(length(min = 1))]
    information: Option<String>,
    #[validate(length(min = 1))]
    license: Option<String>,
    #[validate(length(min = 1))]
    summary: Option<String>,
    #[validate(length(min = 1))]
    description: Option<String>,
    #[validate(length(min = 1))]
    website: Option<String>,
    #[validate(length(min = 1))]
    #[serde(alias = "report-issue", alias = "issues")]
    report_issue: Option<String>,
    #[serde_as(as = "ManyOrOne<_>")]
    #[serde(default, alias = "content")]
    contents: Vec<PackageContent>,
    #[serde(skip)]
    version: Option<String>,
    #[serde(skip)]
    authors: Option<Vec<String>>,
}

impl Package {
    pub(crate) fn expanded(&mut self, scope: &Scope) -> anyhow::Result<()> {
        if let Some(filename) = &mut self.filename {
            *filename = vars::expand(filename, scope)?;
        }

        if let Some(id) = &mut self.id {
            *id = vars::expand(id, scope)?;
        }

        if let Some(name) = &mut self.name {
            *name = vars::expand(name, scope)?;
        }

        if let Some(information) = &mut self.information {
            *information = vars::expand(information, scope)?;
        }

        if let Some(license) = &mut self.license {
            *license = vars::expand(license, scope)?;
        }

        if let Some(summary) = &mut self.summary {
            *summary = vars::expand(summary, scope)?;
        }

        if let Some(description) = &mut self.description {
            *description = vars::expand(description, scope)?;
        }

        if let Some(website) = &mut self.website {
            *website = vars::expand(website, scope)?;
        }

        if let Some(report_issue) = &mut self.report_issue {
            *report_issue = vars::expand(report_issue, scope)?;
        }

        for content in &mut self.contents {
            content.expanded(scope)?;
        }

        Ok(())
    }

    #[must_use]
    pub fn filename(&self) -> &str {
        self.filename.as_deref().unwrap_or("package.au2pkg.zip")
    }

    #[must_use]
    pub(crate) fn is_au2pkg(&self) -> bool {
        self.filename().to_ascii_lowercase().ends_with(".au2pkg.zip")
    }

    pub fn contents(&self) -> impl ExactSizeIterator<Item = anyhow::Result<&PackageContent>> {
        let is_au2pkg = self.is_au2pkg();

        self.contents.iter().map(move |content| {
            if let Err(err) = content.validate() {
                bail!("configuration validation failed:\n{err}");
            }

            if is_au2pkg {
                static ALLOWED_TOP_DIRS: [&str; 8] = [
                    "Plugin",
                    "Script",
                    "Language",
                    "Alias",
                    "Figure",
                    "Transition",
                    "Preset",
                    "Default",
                ];

                let dir = content.dir();
                let top_dir = Path::new(dir)
                    .components()
                    .find(|c| *c != Component::CurDir)
                    .and_then(|c| c.as_os_str().to_str())
                    .unwrap_or(dir);

                if !ALLOWED_TOP_DIRS.contains(&top_dir) {
                    bail!(
                        "invalid top-level directory '{top_dir}' in au2pkg: must be one of {}",
                        ALLOWED_TOP_DIRS.join(", ")
                    );
                }
            }

            Ok(content)
        })
    }

    #[must_use]
    pub(crate) fn to_manifest(&self) -> String {
        let mut lines = vec!["[package]".to_string()];

        if let Some(id) = &self.id {
            lines.push(format!("id={id}"));
        }

        if let Some(name) = &self.name {
            lines.push(format!("name={name}"));
        }

        lines.push(format!(
            "uninstallSubFolderFile={}",
            u8::from(self.uninstall_subdirectory_files)
        ));

        if let Some(information) = &self.information {
            lines.push(format!("information={information}"));
        }

        lines.join("\r\n")
    }

    #[must_use]
    pub(crate) fn to_description(&self) -> String {
        let mut sections = vec![format!("[ {} ]", self.name.as_deref().unwrap_or_default())];

        if let Some(summary) = self.summary.as_deref()
            && !summary.is_empty()
        {
            sections.push(summary.to_string());
        }

        let mut meta = Vec::new();

        if let Some(version) = &self.version {
            meta.push(format!("Version: {version}"));
        }

        if let Some(license) = &self.license {
            meta.push(format!("License: {license}"));
        }

        if let Some(authors) = &self.authors
            && !authors.is_empty()
        {
            meta.push(format!(
                "{}: {}",
                if authors.len() > 1 { "Authors" } else { "Author" },
                authors.join(", ")
            ));
        }

        if let Some(website) = &self.website {
            meta.push(format!("Website: {website}"));
        }

        if let Some(report_issue) = &self.report_issue {
            meta.push(format!("Report Issue: {report_issue}"));
        }

        if !meta.is_empty() {
            sections.push(meta.join("\r\n"));
        }

        if let Some(desc) = self.description.as_deref().filter(|s| !s.is_empty()) {
            sections.push(desc.to_string());
        }

        sections.join("\r\n\r\n")
    }
}

#[serde_as]
#[derive(Debug, Clone, Serialize, Deserialize, Validate, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PackageContent {
    #[validate(length(min = 1), custom(function = "validate_directory"))]
    #[serde(
        alias = "directory",
        alias = "target",
        alias = "dst",
        alias = "dest",
        alias = "destination"
    )]
    dir: String,
    #[validate(nested)]
    #[serde_as(as = "ManyOrOne<_>")]
    #[serde(alias = "src", alias = "source")]
    sources: Vec<PackageSource>,
}

impl PackageContent {
    pub(crate) fn expanded(&mut self, scope: &Scope) -> anyhow::Result<()> {
        self.dir = convert_path(vars::expand(&self.dir, scope)?);

        for source in &mut self.sources {
            source.expanded(scope)?;
        }

        Ok(())
    }

    #[must_use]
    pub fn dir(&self) -> &str {
        &self.dir
    }

    #[must_use]
    pub fn sources(&self) -> &[PackageSource] {
        &self.sources
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[serde(untagged)]
pub enum PackageSource {
    Simple(String),
    Build(PackageSourceBuild),
    Path(PackageSourcePath),
    Url(PackageSourceUrl),
    File(PackageSourceFile),
}

impl PackageSource {
    pub(crate) fn expanded(&mut self, scope: &Scope) -> anyhow::Result<()> {
        match self {
            Self::Simple(val) => {
                let s = vars::expand(val, scope)?;

                *self = if let Some(build) = s.strip_prefix("build:") {
                    Self::Build(PackageSourceBuild {
                        build: build.to_string(),
                    })
                } else if let Some(url) = s.strip_prefix("url:") {
                    Self::Url(PackageSourceUrl {
                        url: url.to_string(),
                        extract: None,
                        pick: None,
                    })
                } else if s.get(..7).is_some_and(|p| p.eq_ignore_ascii_case("http://"))
                    || s.get(..8).is_some_and(|p| p.eq_ignore_ascii_case("https://"))
                {
                    Self::Url(PackageSourceUrl {
                        url: s,
                        extract: None,
                        pick: None,
                    })
                } else {
                    Self::Path(PackageSourcePath { path: s })
                };
            }
            Self::Build(source) => {
                source.build = vars::expand(&source.build, scope)?;
            }
            Self::Path(source) => {
                source.path = vars::expand(&source.path, scope)?;
            }
            Self::Url(source) => {
                source.url = vars::expand(&source.url, scope)?;
                if let Some(pick) = &mut source.pick {
                    *pick = vars::expand(pick, scope)?;
                }
            }
            Self::File(source) => {
                source.filename = vars::expand(&source.filename, scope)?;
                source.content = vars::expand(&source.content, scope)?;
                if let Some(encoding) = &mut source.encoding {
                    *encoding = vars::expand(encoding, scope)?;
                }
                if let Some(newline) = &mut source.newline {
                    *newline = vars::expand(newline, scope)?;
                }
            }
        }
        Ok(())
    }
}

impl Validate for PackageSource {
    fn validate(&self) -> Result<(), ValidationErrors> {
        match self {
            Self::Simple(_) => {
                let mut errors = ValidationErrors::new();
                errors.add(
                    "source",
                    ValidationError::new("unexpanded")
                        .with_message("package source must be expanded before validation".into()),
                );
                Err(errors)
            }
            Self::Build(b) => b.validate(),
            Self::Path(p) => p.validate(),
            Self::Url(u) => u.validate(),
            Self::File(f) => f.validate(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Validate, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PackageSourceBuild {
    #[validate(length(min = 1))]
    build: String,
}

impl PackageSourceBuild {
    #[must_use]
    pub fn id(&self) -> &str {
        &self.build
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Validate, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PackageSourcePath {
    #[validate(length(min = 1), custom(function = "validate_glob"))]
    path: String,
}

impl PackageSourcePath {
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PackageSourceUrl {
    url: String,
    #[serde(default)]
    extract: Option<bool>,
    #[serde(alias = "picker")]
    pick: Option<String>,
}

impl PackageSourceUrl {
    #[must_use]
    pub fn url(&self) -> &str {
        &self.url
    }

    #[must_use]
    pub fn extract(&self) -> Option<bool> {
        self.extract
    }

    #[must_use]
    pub fn pick(&self) -> Option<&str> {
        self.pick.as_deref()
    }
}

impl Validate for PackageSourceUrl {
    fn validate(&self) -> Result<(), ValidationErrors> {
        let mut errors = ValidationErrors::new();

        match reqwest::Url::parse(&self.url) {
            Ok(parsed)
                if !parsed.scheme().eq_ignore_ascii_case("http") && !parsed.scheme().eq_ignore_ascii_case("https") =>
            {
                errors.add(
                    "url",
                    ValidationError::new("invalid_url").with_message("URL scheme must be http or https".into()),
                );
            }
            Err(err) => {
                errors.add(
                    "url",
                    ValidationError::new("invalid_url")
                        .with_message(format!("invalid URL '{}': {err}", self.url).into()),
                );
            }
            _ => {}
        }

        if self.pick.is_some() && self.extract != Some(true) {
            errors.add(
                "pick",
                ValidationError::new("requires_extract").with_message("pick requires extract = true".into()),
            );
        }

        if let Some(pick) = &self.pick
            && let Err(e) = validate_picker(pick)
        {
            errors.add("pick", e);
        }

        if errors.is_empty() { Ok(()) } else { Err(errors) }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Validate, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PackageSourceFile {
    #[validate(custom(function = "validate_filename"))]
    filename: String,
    content: String,
    #[validate(custom(function = "validate_encoding"))]
    encoding: Option<String>,
    #[validate(custom(function = "validate_newline"))]
    newline: Option<String>,
}

impl PackageSourceFile {
    #[must_use]
    pub fn filename(&self) -> &str {
        &self.filename
    }

    #[must_use]
    pub fn content(&self) -> Cow<'_, str> {
        if self.content.contains('\r') {
            Cow::Owned(self.content.replace("\r\n", "\n").replace('\r', "\n"))
        } else {
            Cow::Borrowed(&self.content)
        }
    }

    #[must_use]
    pub fn encoding(&self) -> &'static encoding_rs::Encoding {
        self.encoding
            .as_deref()
            .and_then(|s| encoding_rs::Encoding::for_label(s.as_bytes()))
            .unwrap_or(encoding_rs::UTF_8)
    }

    #[must_use]
    pub fn newline(&self) -> Option<&'static str> {
        self.newline.as_deref().and_then(parse_newline)
    }
}

struct ManyOrOne<T>(PhantomData<T>);

impl<'de, T, TA> DeserializeAs<'de, Vec<T>> for ManyOrOne<TA>
where
    TA: DeserializeAs<'de, T>,
{
    fn deserialize_as<D>(deserializer: D) -> Result<Vec<T>, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        <PickFirst<(Vec<TA>, OneOrMany<TA>)> as DeserializeAs<'de, Vec<T>>>::deserialize_as(deserializer)
    }
}

impl<T, TA> SerializeAs<Vec<T>> for ManyOrOne<TA>
where
    TA: SerializeAs<T>,
{
    fn serialize_as<S>(src: &Vec<T>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        <OneOrMany<TA> as SerializeAs<Vec<T>>>::serialize_as(src, serializer)
    }
}

impl<T, TA> JsonSchemaAs<Vec<T>> for ManyOrOne<TA>
where
    TA: JsonSchemaAs<T>,
{
    fn inline_schema() -> bool {
        <OneOrMany<TA> as JsonSchemaAs<Vec<T>>>::inline_schema()
    }

    fn schema_name() -> Cow<'static, str> {
        <OneOrMany<TA> as JsonSchemaAs<Vec<T>>>::schema_name()
    }

    fn schema_id() -> Cow<'static, str> {
        <OneOrMany<TA> as JsonSchemaAs<Vec<T>>>::schema_id()
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        <OneOrMany<TA> as JsonSchemaAs<Vec<T>>>::json_schema(generator)
    }
}

fn expand_map(vars: &mut IndexMap<String, String>, parent: &Scope) -> anyhow::Result<Scope> {
    let mut scope = parent.set(Arc::new(IndexMap::with_capacity(vars.len())));

    for (key, val) in vars.iter_mut() {
        if RESERVED_VARIABLES.contains(&key.as_str()) {
            bail!("cannot define reserved variable '{key}'");
        }

        let expanded = vars::expand(val, &scope)?;
        val.clone_from(&expanded);
        scope.insert(key.clone(), expanded);
    }

    Ok(scope)
}

fn convert_path(path: String) -> String {
    #[cfg(windows)]
    let path = if path.contains('/') {
        path.replace('/', "\\")
    } else {
        path
    };

    path
}

fn validate_requires_astra(ver: &str) -> Result<(), ValidationError> {
    semver::VersionReq::parse(ver)
        .map(|_| ())
        .map_err(|_| ValidationError::new("invalid_version_req"))
}

fn validate_package_filename(filename: &str) -> Result<(), ValidationError> {
    validate_filename(filename)?;

    let Some((stem, _)) = filename
        .rsplit_once('.')
        .filter(|(_, ext)| ext.eq_ignore_ascii_case("zip"))
    else {
        return Err(
            ValidationError::new("invalid_extension").with_message("package filename must end with .zip".into())
        );
    };

    if stem != stem.trim() || stem.is_empty() {
        return Err(ValidationError::new("invalid_filename")
            .with_message("package filename must have a base name before .zip".into()));
    }

    Ok(())
}

fn validate_filename(filename: &str) -> Result<(), ValidationError> {
    let mut components = Path::new(filename).components();

    if filename != filename.trim()
        || filename.is_empty()
        || filename.contains(std::path::is_separator)
        || !matches!(components.next(), Some(Component::Normal(_)))
        || components.next().is_some()
    {
        return Err(ValidationError::new("invalid_filename")
            .with_message("must be a single file name without leading/trailing whitespace or path separators".into()));
    }

    #[cfg(windows)]
    if filename.contains([':', '<', '>', '"', '|', '?', '*']) {
        return Err(ValidationError::new("invalid_filename")
            .with_message("must not contain Windows reserved characters (< > : \" | ? *)".into()));
    }

    Ok(())
}

fn validate_directory(dir: &str) -> Result<(), ValidationError> {
    if dir != dir.trim()
        || dir.is_empty()
        || Path::new(dir)
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::RootDir | Component::Prefix(_)))
    {
        return Err(ValidationError::new("invalid_directory").with_message(
            "must be a relative path without leading/trailing whitespace, '..', root directory, or drive letters"
                .into(),
        ));
    }

    Ok(())
}

fn validate_glob(pattern: &str) -> Result<(), ValidationError> {
    let bytes = pattern.as_bytes();
    let expr = if bytes.first().is_some_and(u8::is_ascii_alphabetic) && bytes.get(1) == Some(&b':') {
        if bytes.get(2) != Some(&b'/') {
            return Err(
                ValidationError::new("invalid_glob").with_message("drive-relative paths are not allowed".into())
            );
        }

        &pattern[2..]
    } else {
        pattern
    };

    let (prefix, _) = wax::Glob::new(expr)
        .map_err(|err| ValidationError::new("invalid_glob").with_message(err.to_string().into()))?
        .partition();

    if let Some(Component::Prefix(component)) = prefix.components().next()
        && (!matches!(component.kind(), std::path::Prefix::Disk(_)) || !prefix.has_root())
    {
        return Err(ValidationError::new("invalid_glob")
            .with_message("only drive-absolute paths are allowed as Windows path prefixes".into()));
    }

    Ok(())
}

fn validate_picker(pattern: &str) -> Result<(), ValidationError> {
    let (prefix, _) = wax::Glob::new(pattern)
        .map_err(|err| ValidationError::new("invalid_picker").with_message(err.to_string().into()))?
        .partition();

    let cleaned = clean_path::clean(&prefix);

    if [&prefix, &cleaned].into_iter().any(|path| {
        let text = path.to_string_lossy();
        let bytes = text.as_bytes();
        (bytes.first().is_some_and(u8::is_ascii_alphabetic) && bytes.get(1) == Some(&b':'))
            || path
                .components()
                .any(|c| matches!(c, Component::RootDir | Component::Prefix(_)))
    }) || cleaned.components().any(|c| matches!(c, Component::ParentDir))
    {
        return Err(ValidationError::new("invalid_picker")
            .with_message("pick must resolve to a relative path within the zip archive".into()));
    }

    Ok(())
}

fn validate_run(run: &[String]) -> Result<(), ValidationError> {
    if run.is_empty() || run.iter().any(String::is_empty) {
        return Err(ValidationError::new("length"));
    }

    Ok(())
}

fn validate_strings(list: &[String]) -> Result<(), ValidationError> {
    if list.iter().any(String::is_empty) {
        return Err(ValidationError::new("length"));
    }
    Ok(())
}

fn validate_encoding(encoding: &str) -> Result<(), ValidationError> {
    if encoding_rs::Encoding::for_label(encoding.as_bytes()).is_none() {
        return Err(
            ValidationError::new("invalid_encoding").with_message(format!("unsupported encoding '{encoding}'").into())
        );
    }
    Ok(())
}

fn validate_newline(newline: &str) -> Result<(), ValidationError> {
    if parse_newline(newline).is_none() {
        return Err(
            ValidationError::new("invalid_newline").with_message("newline must be one of \\r\\n, crlf, \\n, lf".into())
        );
    }
    Ok(())
}

fn parse_newline(s: &str) -> Option<&'static str> {
    let s = s.trim_matches(|c: char| c.is_whitespace() && c != '\r' && c != '\n');
    if s == "\n" || s.eq_ignore_ascii_case("lf") {
        Some("\n")
    } else if s == "\r\n" || s.eq_ignore_ascii_case("crlf") {
        Some("\r\n")
    } else {
        None
    }
}
