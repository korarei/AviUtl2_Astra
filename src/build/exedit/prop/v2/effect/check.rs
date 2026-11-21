use crate::build::lang::lua;
use crate::build::source::Pos;
use anyhow::bail;
use std::borrow::Cow;
use std::collections::HashSet;
use std::fmt;

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::build::exedit) enum Id<'a> {
    Key(&'a str),
    Var(&'a str),
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Truth {
    Bool(bool),
    Int(u8),
}

impl fmt::Display for Truth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bool(val) => write!(f, "{val}"),
            Self::Int(val) => write!(f, "{val}"),
        }
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub(in crate::build::exedit) struct Prop<'a> {
    pos: Pos,
    id: Id<'a>,
    name: &'a str,
    default: Option<Truth>,
    rest: Vec<&'a str>,
}

#[allow(dead_code)]
impl<'a> Prop<'a> {
    pub(in crate::build::exedit) fn parse(
        pos: Pos,
        kind: &'a str,
        var: Option<&'a str>,
        body: &'a str,
    ) -> anyhow::Result<Option<Self>> {
        let id = match var {
            Some(var) if kind == "check" => Id::Var(var),
            None if kind.starts_with("check") && kind != "checksection" => Id::Key(kind),
            _ => return Ok(None),
        };

        if body.chars().any(char::is_control) {
            bail!("{pos}: property '{kind}' cannot contain control characters");
        }

        let mut parts = body.split(',');
        let name = parts.next().unwrap().trim();
        let count = parts.clone().count();

        if count < 1 {
            bail!("{pos}: property '{kind}' requires at least 1 argument, got {count}");
        }

        if count > 1 {
            tracing::warn!("{pos}: property '{kind}' accepts at most 1 argument, got {count}");
        }

        let arg = parts.next().unwrap().trim();
        let default = match id {
            Id::Var(_) => match arg {
                "_" => None,
                "true" => Some(Truth::Bool(true)),
                "false" => Some(Truth::Bool(false)),
                "1" => Some(Truth::Int(1)),
                "0" => Some(Truth::Int(0)),
                _ => {
                    bail!("{pos}: default value of property '{kind}' must be 'true', 'false', '0', or '1', got '{arg}'")
                }
            },
            Id::Key(_) => match arg {
                "1" => Some(Truth::Int(1)),
                "0" => Some(Truth::Int(0)),
                _ => bail!("{pos}: default value of property '{kind}' must be '0' or '1', got '{arg}'"),
            },
        };
        let rest = parts.collect();

        Ok(Some(Self {
            pos,
            id,
            name,
            default,
            rest,
        }))
    }

    pub(in crate::build::exedit) fn id(&self) -> Id<'a> {
        self.id
    }

    pub(in crate::build::exedit) fn name(&self) -> &'a str {
        self.name
    }

    pub(in crate::build::exedit) fn label(&self) -> &'a str {
        self.name.rsplit_once("::").map_or(self.name, |(_, label)| label)
    }

    pub(in crate::build::exedit) fn bind(&mut self, var: &'a str, val: Option<&str>) -> anyhow::Result<()> {
        let pos = &self.pos;

        let curr = match self.id {
            Id::Key(key) => bail!("{pos}: property '{key}' cannot bind to a variable"),
            Id::Var(curr) => curr,
        };

        if curr != "_" && curr != var {
            bail!("{pos}: variable name '{curr}' conflicts with '{var}'");
        }

        if let Some(val) = val {
            let Some(truth) = parse_truth(val) else {
                bail!("{pos}: initializer of variable '{var}' must be 'true', 'false', '0', or '1', got '{val}'");
            };

            match self.default {
                Some(default) if default != truth => {
                    bail!("{pos}: default value '{default}' conflicts with '{val}'");
                }
                None => self.default = Some(truth),
                Some(_) => {}
            }
        } else if self.default.is_none() {
            bail!("{pos}: variable '{var}' requires an initializer");
        }

        self.id = Id::Var(var);

        Ok(())
    }

    pub(in crate::build::exedit) fn validate(
        &self,
        keys: &mut HashSet<&'a str>,
        vars: &mut HashSet<&'a str>,
        names: &mut HashSet<Cow<'a, str>>,
    ) -> anyhow::Result<()> {
        let pos = &self.pos;

        let kind = match self.id {
            Id::Key(key) => {
                if !keys.insert(key) {
                    bail!("{pos}: duplicate property key '{key}'");
                }

                tracing::warn!("{pos}: property '{key}' uses legacy syntax");
                key
            }
            Id::Var(var) => {
                if !lua::is_ident(var).map_err(|err| anyhow::anyhow!("{pos}: {err}"))? {
                    bail!("{pos}: variable name '{var}' of property 'check' must be a Lua identifier");
                }

                if !vars.insert(var) {
                    bail!("{pos}: duplicate variable name '{var}'");
                }

                if self.name.starts_with("effect.") {
                    bail!(
                        "{pos}: property name must not start with 'effect.', got '{}'",
                        self.name
                    );
                }

                if lua::parse_number(self.name).is_some() {
                    bail!("{pos}: property name must not be a number, got '{}'", self.name);
                }

                if !names.insert(Cow::Borrowed(self.name)) {
                    bail!("{pos}: duplicate property name '{}'", self.name);
                }

                "check"
            }
        };

        if self.default.is_none() {
            bail!("{pos}: default value of property '{kind}' must be 'true', 'false', '0', or '1', got '_'");
        }

        Ok(())
    }
}

impl fmt::Display for Prop<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.id {
            Id::Key(key) => {
                write!(f, "--{key}:{},{}", self.name, self.default.unwrap_or(Truth::Int(0)))?;
            }
            Id::Var(var) => {
                write!(
                    f,
                    "--check@{var}:{},{}",
                    self.name,
                    self.default.unwrap_or(Truth::Bool(false))
                )?;
            }
        }

        for arg in &self.rest {
            write!(f, ",{arg}")?;
        }

        Ok(())
    }
}

fn parse_truth(src: &str) -> Option<Truth> {
    match src {
        "true" => Some(Truth::Bool(true)),
        "false" => Some(Truth::Bool(false)),
        _ => match lua::parse_integer(src)? {
            1 => Some(Truth::Int(1)),
            0 => Some(Truth::Int(0)),
            _ => None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn pos() -> Pos {
        Pos {
            file: Path::new("input.anm2").into(),
            line: 2,
            col: 3,
        }
    }

    #[test]
    fn parses_check_prop() {
        let prop = Prop::parse(pos(), "check", Some("flag"), "  Enable  , true ")
            .unwrap()
            .unwrap();
        assert_eq!(prop.id(), Id::Var("flag"));
        assert_eq!(prop.name(), "Enable");
        assert_eq!(prop.label(), "Enable");
        assert_eq!(prop.to_string(), "--check@flag:Enable,true");

        let scoped = Prop::parse(pos(), "check", Some("flag"), "Group::Sub::Enable,0")
            .unwrap()
            .unwrap();
        assert_eq!(scoped.name(), "Group::Sub::Enable");
        assert_eq!(scoped.label(), "Enable");
        assert_eq!(scoped.to_string(), "--check@flag:Group::Sub::Enable,0");

        let int = Prop::parse(pos(), "check", Some("flag"), "Enable,1").unwrap().unwrap();
        assert_eq!(int.to_string(), "--check@flag:Enable,1");

        let legacy = Prop::parse(pos(), "check0", None, "Enable,1").unwrap().unwrap();
        assert_eq!(legacy.id(), Id::Key("check0"));
        assert_eq!(legacy.to_string(), "--check0:Enable,1");

        assert!(
            Prop::parse(pos(), "checksection", Some("flag"), "Group,true")
                .unwrap()
                .is_none()
        );
        assert!(
            Prop::parse(pos(), "check0", Some("flag"), "Enable,0")
                .unwrap()
                .is_none()
        );
        assert!(
            Prop::parse(pos(), "track", Some("value"), "Label,0,10,0")
                .unwrap()
                .is_none()
        );

        let extra = Prop::parse(pos(), "check", Some("flag"), "Enable,false,extra , more")
            .unwrap()
            .unwrap();
        assert_eq!(extra.to_string(), "--check@flag:Enable,false,extra , more");

        for (kind, var, body) in [
            ("check", Some("flag"), "Enable"),
            ("check", Some("flag"), "Enable,"),
            ("check", Some("flag"), "Enable,2"),
            ("check", Some("flag"), "Enable,0x1"),
            ("check0", None, "Enable,true"),
            ("check0", None, "Enable,_"),
        ] {
            let err = Prop::parse(pos(), kind, var, body).unwrap_err();
            assert!(err.to_string().starts_with("input.anm2:2:3:"), "{kind}: {body}: {err}");
        }
    }

    #[test]
    fn binds_var_and_default() {
        let mut prop = Prop::parse(pos(), "check", Some("_"), "Enable, _").unwrap().unwrap();
        prop.bind("flag", Some("true")).unwrap();
        assert_eq!(prop.to_string(), "--check@flag:Enable,true");

        let mut matching = Prop::parse(pos(), "check", Some("flag"), "Enable,1").unwrap().unwrap();
        assert!(matching.bind("flag", Some("1")).is_ok());
        assert!(matching.bind("flag", Some("0x1")).is_ok());

        let mut int_bind = Prop::parse(pos(), "check", Some("_"), "Enable,_").unwrap().unwrap();
        int_bind.bind("flag", Some("0b1")).unwrap();
        assert_eq!(int_bind.to_string(), "--check@flag:Enable,1");

        let mut no_init = Prop::parse(pos(), "check", Some("_"), "Enable,false").unwrap().unwrap();
        no_init.bind("flag", None).unwrap();
        assert_eq!(no_init.to_string(), "--check@flag:Enable,false");

        for (kind, target, body, var, val) in [
            ("check0", None, "Enable,0", "flag", Some("0")),
            ("check", Some("other"), "Enable,_", "flag", Some("true")),
            ("check", Some("flag"), "Enable,_", "flag", None),
            ("check", Some("flag"), "Enable,_", "flag", Some("2")),
            ("check", Some("flag"), "Enable,true", "flag", Some("false")),
            ("check", Some("flag"), "Enable,true", "flag", Some("1")),
            ("check", Some("flag"), "Enable,0", "flag", Some("false")),
        ] {
            let mut invalid = Prop::parse(pos(), kind, target, body).unwrap().unwrap();
            assert!(invalid.bind(var, val).is_err(), "{kind}: {body}: {var}={val:?}");
        }
    }

    #[test]
    fn validates_separate_id_sets() {
        let mut keys = HashSet::new();
        let mut vars = HashSet::new();
        let mut names = HashSet::new();

        for (kind, var, body) in [("check0", None, "Legacy,0"), ("check", Some("check0"), "Modern,true")] {
            let prop = Prop::parse(pos(), kind, var, body).unwrap().unwrap();
            prop.validate(&mut keys, &mut vars, &mut names).unwrap();
        }

        assert_eq!(keys, HashSet::from(["check0"]));
        assert_eq!(vars, HashSet::from(["check0"]));

        let duplicate = Prop::parse(pos(), "check0", None, "Other,1").unwrap().unwrap();
        assert!(duplicate.validate(&mut keys, &mut vars, &mut names).is_err());
    }

    #[test]
    fn validates_check_prop() {
        for (kind, var, body) in [
            ("check", Some("flag"), "Enable,true"),
            ("check", Some("flag"), "1enable,true"),
            ("check", Some("flag"), "Enable,0"),
            ("check0", None, "Enable,1"),
        ] {
            let prop = Prop::parse(pos(), kind, var, body).unwrap().unwrap();
            let mut keys = HashSet::new();
            let mut vars = HashSet::new();
            let mut names = HashSet::new();
            prop.validate(&mut keys, &mut vars, &mut names).unwrap();
        }

        for (kind, var, body) in [
            ("check", Some("1invalid"), "Enable,true"),
            ("check", Some("flag"), "effect.foo,true"),
            ("check", Some("flag"), "123,true"),
            ("check", Some("flag"), "Enable,_"),
        ] {
            let mut keys = HashSet::new();
            let mut vars = HashSet::new();
            let mut names = HashSet::new();
            let err = Prop::parse(pos(), kind, var, body)
                .unwrap()
                .unwrap()
                .validate(&mut keys, &mut vars, &mut names)
                .unwrap_err();
            assert!(err.to_string().starts_with("input.anm2:2:3:"), "{kind}: {body}: {err}");
        }

        let prop = Prop::parse(pos(), "check", Some("flag"), "Enable,true")
            .unwrap()
            .unwrap();
        let mut keys = HashSet::new();
        let mut vars = HashSet::from(["flag"]);
        let mut names = HashSet::new();
        assert!(prop.validate(&mut keys, &mut vars, &mut names).is_err());

        let mut keys = HashSet::new();

        let mut vars = HashSet::new();
        let mut names = HashSet::from([Cow::Borrowed("Enable")]);
        assert!(prop.validate(&mut keys, &mut vars, &mut names).is_err());
    }
}
