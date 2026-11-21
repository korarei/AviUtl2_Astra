use crate::build::lang::lua;
use crate::build::source::Pos;
use anyhow::bail;
use std::borrow::Cow;
use std::collections::HashSet;
use std::fmt;

#[allow(dead_code)]
const STEPS: &[f64] = &[
    1.0,
    0.1,
    0.01,
    0.001,
    0.000_1,
    0.000_01,
    0.000_001,
    0.000_000_1,
    0.000_000_01,
    0.000_000_001,
];

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::build::exedit) enum Id<'a> {
    Key(&'a str),
    Var(&'a str),
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub(in crate::build::exedit) struct Prop<'a> {
    pos: Pos,
    id: Id<'a>,
    name: &'a str,
    min: f64,
    max: f64,
    default: Option<f64>,
    step: Option<f64>,
    zero_text: Option<&'a str>,
    sensitivity: Option<f64>,
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
            Some(var) if kind == "track" => Id::Var(var),
            None if kind.starts_with("track") && kind != "trackgroup" => Id::Key(kind),
            _ => return Ok(None),
        };

        if body.chars().any(char::is_control) {
            bail!("{pos}: property '{kind}' cannot contain control characters");
        }

        let mut parts = body.split(',');
        let name = parts.next().unwrap().trim();
        let count = parts.clone().count();

        if count < 3 {
            bail!("{pos}: property '{kind}' requires at least 3 arguments, got {count}");
        }

        if count > 6 {
            tracing::warn!("{pos}: property '{kind}' accepts at most 6 arguments, got {count}");
        }

        let parse = |field: &str, arg: &str| -> anyhow::Result<f64> {
            let arg = arg.trim();
            let Some(val) = lua::parse_number(arg).filter(|val| val.is_finite()) else {
                bail!("{pos}: {field} of property '{kind}' must be a number, got '{arg}'");
            };

            Ok(val)
        };

        let min = parse("minimum value", parts.next().unwrap())?;
        let max = parse("maximum value", parts.next().unwrap())?;
        let arg = parts.next().unwrap();
        let default = if matches!(id, Id::Var(_)) && arg.trim() == "_" {
            None
        } else {
            Some(parse("default value", arg)?)
        };
        let step = parts
            .next()
            .map(|arg| {
                let val = parse("step size", arg)?;
                if !STEPS.iter().any(|&v| (val - v).abs() < f64::EPSILON) {
                    bail!("{pos}: step size of property '{kind}' must be 1, 0.1, ..., 0.000000001, got '{val}'");
                }

                Ok(val)
            })
            .transpose()?;
        let zero_text = parts.next().map(str::trim);
        let sensitivity = parts.next().map(|arg| parse("sensitivity", arg)).transpose()?;
        let rest = parts.collect();

        Ok(Some(Self {
            pos,
            id,
            name,
            min,
            max,
            default,
            step,
            zero_text,
            sensitivity,
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
            let Some(num) = lua::parse_number(val).filter(|val| val.is_finite()) else {
                bail!("{pos}: initializer of variable '{var}' must be a number, got '{val}'");
            };

            match self.default {
                Some(default) if (default - num).abs() > f64::EPSILON => {
                    bail!("{pos}: default value '{default}' conflicts with '{val}'");
                }
                None => self.default = Some(num),
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
                    bail!("{pos}: variable name '{var}' of property 'track' must be a Lua identifier");
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

                "track"
            }
        };

        let Some(default) = self.default else {
            bail!("{pos}: default value of property '{kind}' must be a number, got '_'");
        };

        let min = self.min;
        let max = self.max;

        if min > max {
            bail!("{pos}: minimum value ({min}) of property '{kind}' must not exceed maximum value ({max})");
        }

        if !(min <= default && default <= max) {
            bail!("{pos}: default value of property '{kind}' must be between {min} and {max}, got {default}");
        }

        Ok(())
    }
}

impl fmt::Display for Prop<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.id {
            Id::Key(key) => write!(f, "--{key}")?,
            Id::Var(var) => write!(f, "--track@{var}")?,
        }

        write!(
            f,
            ":{},{},{},{}",
            self.name,
            self.min,
            self.max,
            self.default.unwrap_or(self.min)
        )?;

        if let Some(step) = self.step {
            write!(f, ",{step}")?;
        }

        if let Some(zero_text) = self.zero_text {
            write!(f, ",{zero_text}")?;
        }

        if let Some(sensitivity) = self.sensitivity {
            write!(f, ",{sensitivity}")?;
        }

        for arg in &self.rest {
            write!(f, ",{arg}")?;
        }

        Ok(())
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
    fn parses_track_prop() {
        let prop = Prop::parse(pos(), "track", Some("value"), "  Label  , 0 , 10 , 2 , 1 , ")
            .unwrap()
            .unwrap();
        assert_eq!(prop.id(), Id::Var("value"));
        assert_eq!(prop.name(), "Label");
        assert_eq!(prop.label(), "Label");
        assert_eq!(prop.to_string(), "--track@value:Label,0,10,2,1,");

        let scoped = Prop::parse(pos(), "track", Some("value"), "Group::Sub::Label,0,10,0")
            .unwrap()
            .unwrap();
        assert_eq!(scoped.name(), "Group::Sub::Label");
        assert_eq!(scoped.label(), "Label");
        assert_eq!(scoped.to_string(), "--track@value:Group::Sub::Label,0,10,0");

        let hex = Prop::parse(pos(), "track", Some("value"), "Label,+0x2,0xff,0x1.fp3,0x1,off,+0b10")
            .unwrap()
            .unwrap();
        assert_eq!(hex.to_string(), "--track@value:Label,2,255,15.5,1,off,2");

        let legacy = Prop::parse(pos(), "track0", None, "Label,0,100,0,1").unwrap().unwrap();
        assert_eq!(legacy.id(), Id::Key("track0"));
        assert_eq!(legacy.to_string(), "--track0:Label,0,100,0,1");

        assert!(
            Prop::parse(pos(), "trackgroup", Some("a,b"), "Group")
                .unwrap()
                .is_none()
        );
        assert!(
            Prop::parse(pos(), "track0", Some("value"), "Label,0,100,0")
                .unwrap()
                .is_none()
        );
        assert!(
            Prop::parse(pos(), "check", Some("flag"), "Enable,true")
                .unwrap()
                .is_none()
        );

        let extra = Prop::parse(pos(), "track", Some("value"), "Value,0,1,0,0.1,unused,2,extra , more")
            .unwrap()
            .unwrap();
        assert_eq!(extra.to_string(), "--track@value:Value,0,1,0,0.1,unused,2,extra , more");

        for (kind, var, body) in [
            ("track", Some("value"), "Value"),
            ("track", Some("value"), "Value,"),
            ("track", Some("value"), "Value,0,1"),
            ("track", Some("value"), "Value,NaN,1,0"),
            ("track", Some("value"), "Value,0,inf,0"),
            ("track", Some("value"), "Value,0,1,NaN"),
            ("track", Some("value"), "Value,0,1,0,0"),
            ("track", Some("value"), "Value,0,1,0,0.2"),
            ("track", Some("value"), "Value,0,1,0,NaN"),
            ("track", Some("value"), "Value,0,1,0,1,unused,inf"),
            ("track0", None, "Value,0,1,_"),
        ] {
            let err = Prop::parse(pos(), kind, var, body).unwrap_err();
            assert!(err.to_string().starts_with("input.anm2:2:3:"), "{kind}: {body}: {err}");
        }
    }

    #[test]
    fn binds_var_and_default() {
        let mut prop = Prop::parse(pos(), "track", Some("_"), "Label, 0, 10, _, 1")
            .unwrap()
            .unwrap();
        prop.bind("value", Some("5")).unwrap();
        assert_eq!(prop.to_string(), "--track@value:Label,0,10,5,1");

        let mut matching = Prop::parse(pos(), "track", Some("value"), "Label,0,10,5.0,1")
            .unwrap()
            .unwrap();
        assert!(matching.bind("value", Some("5")).is_ok());
        assert!(matching.bind("value", Some("0x5")).is_ok());

        let mut hex = Prop::parse(pos(), "track", Some("_"), "Label,0,0x10,_,1")
            .unwrap()
            .unwrap();
        hex.bind("value", Some("0x1.fp2")).unwrap();
        assert_eq!(hex.to_string(), "--track@value:Label,0,16,7.75,1");

        let mut no_init = Prop::parse(pos(), "track", Some("_"), "Label,0,10,2,1")
            .unwrap()
            .unwrap();
        no_init.bind("value", None).unwrap();
        assert_eq!(no_init.to_string(), "--track@value:Label,0,10,2,1");

        for (kind, target, body, var, val) in [
            ("track0", None, "Label,0,10,2", "value", Some("2")),
            ("track", Some("other"), "Label,0,10,_", "value", Some("5")),
            ("track", Some("value"), "Label,0,10,_", "value", None),
            ("track", Some("value"), "Label,0,10,_", "value", Some("abc")),
            ("track", Some("value"), "Label,0,10,_", "value", Some("NaN")),
            ("track", Some("value"), "Label,0,10,4", "value", Some("5")),
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

        for (kind, var, body) in [
            ("track0", None, "Legacy,0,10,0"),
            ("track", Some("track0"), "Modern,0,10,0"),
        ] {
            let prop = Prop::parse(pos(), kind, var, body).unwrap().unwrap();
            prop.validate(&mut keys, &mut vars, &mut names).unwrap();
        }

        assert_eq!(keys, HashSet::from(["track0"]));
        assert_eq!(vars, HashSet::from(["track0"]));

        let duplicate = Prop::parse(pos(), "track0", None, "Other,0,10,1").unwrap().unwrap();
        assert!(duplicate.validate(&mut keys, &mut vars, &mut names).is_err());
    }

    #[test]
    fn validates_track_prop() {
        for body in [
            "Value,0,100,0",
            "1val,0,1,0",
            "Value,-1,1,1,1",
            "Value,0,1,0,0.000000001",
            "Value,0,1,1,0.1,unused,2",
        ] {
            let prop = Prop::parse(pos(), "track", Some("value"), body).unwrap().unwrap();
            let mut keys = HashSet::new();
            let mut vars = HashSet::new();
            let mut names = HashSet::new();
            prop.validate(&mut keys, &mut vars, &mut names).unwrap();
        }

        let mut keys = HashSet::new();

        let mut vars = HashSet::new();
        let mut names = HashSet::new();
        Prop::parse(pos(), "track0", None, "Value,0,100,0")
            .unwrap()
            .unwrap()
            .validate(&mut keys, &mut vars, &mut names)
            .unwrap();

        for (kind, var, body) in [
            ("track", Some("1invalid"), "Value,0,1,0"),
            ("track", Some("value"), "effect.val,0,1,0"),
            ("track", Some("value"), "123,0,1,0"),
            ("track", Some("value"), "Value,0,1,_"),
            ("track", Some("value"), "Value,0,1,2"),
            ("track", Some("value"), "Value,2,1,1"),
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

        let prop = Prop::parse(pos(), "track", Some("value"), "Value,0,1,0")
            .unwrap()
            .unwrap();
        let mut keys = HashSet::new();
        let mut vars = HashSet::from(["value"]);
        let mut names = HashSet::new();
        assert!(prop.validate(&mut keys, &mut vars, &mut names).is_err());

        let mut keys = HashSet::new();

        let mut vars = HashSet::new();
        let mut names = HashSet::from([Cow::Borrowed("Value")]);
        assert!(prop.validate(&mut keys, &mut vars, &mut names).is_err());
    }
}
