use crate::build::lang::lua;
use crate::build::source::Pos;
use anyhow::bail;
use std::borrow::Cow;
use std::collections::HashSet;
use std::fmt;

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Color {
    Nil,
    Int(i64),
}

impl fmt::Display for Color {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Nil => f.write_str("nil"),
            Self::Int(val) => write!(f, "0x{val:06x}"),
        }
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub(in crate::build::exedit) struct Prop<'a> {
    pos: Pos,
    is_compat: bool,
    var: &'a str,
    name: &'a str,
    default: Option<Color>,
    rest: Option<&'a str>,
}

#[allow(dead_code)]
impl<'a> Prop<'a> {
    pub(in crate::build::exedit) fn parse(
        pos: Pos,
        kind: &str,
        var: Option<&'a str>,
        body: &'a str,
    ) -> anyhow::Result<Option<Self>> {
        if kind != "color" {
            return Ok(None);
        }

        if body.chars().any(char::is_control) {
            bail!("{pos}: property 'color' cannot contain control characters");
        }

        let is_compat = var.is_none();
        let (var, name, args) = if let Some(var) = var {
            let Some((name, args)) = body.split_once(',') else {
                bail!("{pos}: property 'color' requires at least 1 argument, got 0");
            };

            (var, name.trim(), args)
        } else {
            ("color", "色", body)
        };

        let (arg, rest) = args
            .split_once(',')
            .map_or((args.trim(), None), |(arg, rest)| (arg.trim(), Some(rest)));

        if let Some(rest) = rest {
            tracing::warn!(
                "{pos}: property 'color' accepts at most 1 argument, got {}",
                rest.split(',').count() + 1
            );
        }

        let default = if arg == "_" {
            None
        } else {
            let Some(val) = parse_color(arg) else {
                bail!("{pos}: default value of property 'color' must be 'nil' or a hexadecimal integer, got '{arg}'");
            };

            Some(val)
        };

        Ok(Some(Self {
            pos,
            is_compat,
            var,
            name,
            default,
            rest,
        }))
    }

    pub(in crate::build::exedit) fn can_bind(&self) -> bool {
        !self.is_compat
    }

    pub(in crate::build::exedit) fn name(&self) -> &'a str {
        self.name
    }

    pub(in crate::build::exedit) fn label(&self) -> &'a str {
        self.name.rsplit_once("::").map_or(self.name, |(_, label)| label)
    }

    pub(in crate::build::exedit) fn bind(&mut self, var: &'a str, val: Option<&str>) -> anyhow::Result<()> {
        let pos = &self.pos;

        if self.var != "_" && self.var != var {
            bail!("{pos}: variable name '{}' conflicts with '{var}'", self.var);
        }

        if let Some(val) = val {
            let Some(color) = parse_lua_color(val) else {
                bail!("{pos}: initializer of variable '{var}' must be 'nil' or an integer, got '{val}'");
            };

            match self.default {
                Some(default) if default != color => {
                    bail!("{pos}: default value '{default}' conflicts with '{val}'");
                }
                None => self.default = Some(color),
                Some(_) => {}
            }
        } else if self.default.is_none() {
            bail!("{pos}: variable '{var}' requires an initializer");
        }

        self.var = var;

        Ok(())
    }

    pub(in crate::build::exedit) fn validate(
        &self,
        vars: &mut HashSet<&'a str>,
        names: &mut HashSet<Cow<'a, str>>,
    ) -> anyhow::Result<()> {
        let pos = &self.pos;

        if self.is_compat {
            tracing::warn!("{pos}: property 'color' uses legacy syntax");
        } else if !lua::is_ident(self.var).map_err(|err| anyhow::anyhow!("{pos}: {err}"))? {
            bail!(
                "{pos}: variable name '{}' of property 'color' must be a Lua identifier",
                self.var
            );
        }

        if !vars.insert(self.var) {
            bail!("{pos}: duplicate variable name '{}'", self.var);
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

        match self.default {
            None => bail!("{pos}: default value of property 'color' must be 'nil' or a hexadecimal integer, got '_'"),
            Some(Color::Int(val)) if !(0..=0xff_ffff).contains(&val) => {
                bail!("{pos}: default value of property 'color' must be between 0x000000 and 0xffffff, got {val}");
            }
            _ => {}
        }

        Ok(())
    }
}

impl fmt::Display for Prop<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let default = self.default.unwrap_or(Color::Int(0));

        if self.is_compat {
            write!(f, "--color:{default}")?;
        } else {
            write!(f, "--color@{}:{},{}", self.var, self.name, default)?;
        }

        if let Some(rest) = self.rest {
            write!(f, ",{rest}")?;
        }

        Ok(())
    }
}

fn parse_color(src: &str) -> Option<Color> {
    if src == "nil" {
        return Some(Color::Nil);
    }

    let invalid = match src.as_bytes() {
        [b'0', b'x', ..] => b'A'..=b'F',
        [b'0', b'X', ..] => b'a'..=b'f',
        _ => return None,
    };
    let digits = &src[2..];

    if digits
        .bytes()
        .any(|byte| !byte.is_ascii_hexdigit() || invalid.contains(&byte))
    {
        return None;
    }

    i64::from_str_radix(digits, 16).ok().map(Color::Int)
}

fn parse_lua_color(src: &str) -> Option<Color> {
    match src {
        "nil" => Some(Color::Nil),
        _ => lua::parse_integer(src).map(Color::Int),
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
    fn parses_color_prop() {
        let prop = Prop::parse(pos(), "color", Some("col"), "  Color  , 0xffffff ")
            .unwrap()
            .unwrap();
        assert_eq!(prop.var, "col");
        assert_eq!(prop.name(), "Color");
        assert_eq!(prop.label(), "Color");
        assert_eq!(prop.to_string(), "--color@col:Color,0xffffff");

        let scoped = Prop::parse(pos(), "color", Some("col"), "Group::Sub::Color,nil")
            .unwrap()
            .unwrap();
        assert_eq!(scoped.name(), "Group::Sub::Color");
        assert_eq!(scoped.label(), "Color");
        assert_eq!(scoped.to_string(), "--color@col:Group::Sub::Color,nil");

        let legacy = Prop::parse(pos(), "color", None, "0xff0000").unwrap().unwrap();
        assert_eq!(legacy.var, "color");
        assert_eq!(legacy.name(), "色");
        assert_eq!(legacy.label(), "色");
        assert_eq!(legacy.to_string(), "--color:0xff0000");

        let legacy_nil = Prop::parse(pos(), "color", None, "nil").unwrap().unwrap();
        assert_eq!(legacy_nil.to_string(), "--color:nil");

        let upper = Prop::parse(pos(), "color", Some("col"), "Color,0XFF00AA")
            .unwrap()
            .unwrap();
        assert_eq!(upper.to_string(), "--color@col:Color,0xff00aa");

        let extra = Prop::parse(pos(), "color", Some("col"), "Color,0x0,extra , more")
            .unwrap()
            .unwrap();
        assert_eq!(extra.to_string(), "--color@col:Color,0x000000,extra , more");

        for (kind, var, body) in [("file", Some("path"), "File"), ("track", Some("value"), "Label,0,10,0")] {
            assert!(Prop::parse(pos(), kind, var, body).unwrap().is_none(), "{kind}: {body}");
        }

        for (var, body) in [
            (Some("col"), "Color"),
            (Some("col"), "Color,"),
            (Some("col"), "Color,abc"),
            (Some("col"), "Color,0"),
            (Some("col"), "Color,255"),
            (Some("col"), "Color,+0xffffff"),
            (Some("col"), "Color,0xFF0000"),
            (Some("col"), "Color,0Xff0000"),
            (Some("col"), "Color,0xff_00_00"),
            (None, ""),
            (None, "abc"),
            (None, "0"),
            (None, "16777215"),
            (None, "0xFF0000"),
            (None, "0Xff0000"),
        ] {
            let err = Prop::parse(pos(), "color", var, body).unwrap_err();
            assert!(err.to_string().starts_with("input.anm2:2:3:"), "{var:?}: {body}: {err}");
        }
    }

    #[test]
    fn binds_var_and_default() {
        let mut prop = Prop::parse(pos(), "color", Some("_"), "Color, _").unwrap().unwrap();
        prop.bind("col", Some("0xff00ff")).unwrap();
        assert_eq!(prop.to_string(), "--color@col:Color,0xff00ff");

        let mut dec_bind = Prop::parse(pos(), "color", Some("_"), "Color,_").unwrap().unwrap();
        dec_bind.bind("col", Some("255")).unwrap();
        assert_eq!(dec_bind.to_string(), "--color@col:Color,0x0000ff");

        let mut upper_bind = Prop::parse(pos(), "color", Some("_"), "Color,_").unwrap().unwrap();
        upper_bind.bind("col", Some("0xFF_00_AA")).unwrap();
        assert_eq!(upper_bind.to_string(), "--color@col:Color,0xff00aa");

        let mut nil_bind = Prop::parse(pos(), "color", Some("_"), "Color,_").unwrap().unwrap();
        nil_bind.bind("col", Some("nil")).unwrap();
        assert_eq!(nil_bind.to_string(), "--color@col:Color,nil");

        let mut matching = Prop::parse(pos(), "color", Some("col"), "Color,0xff").unwrap().unwrap();
        assert!(matching.bind("col", Some("255")).is_ok());

        let mut compat_bind = Prop::parse(pos(), "color", None, "_").unwrap().unwrap();
        compat_bind.bind("color", Some("0xff00ff")).unwrap();
        assert_eq!(compat_bind.to_string(), "--color:0xff00ff");

        let mut no_init = Prop::parse(pos(), "color", Some("_"), "Color,nil").unwrap().unwrap();
        no_init.bind("col", None).unwrap();
        assert_eq!(no_init.to_string(), "--color@col:Color,nil");

        for (target, body, var, val) in [
            (None, "0x0", "col", Some("0")),
            (None, "0x0", "color", Some("1")),
            (Some("other"), "Color,_", "col", Some("0")),
            (Some("col"), "Color,_", "col", None),
            (Some("col"), "Color,_", "col", Some("abc")),
            (Some("col"), "Color,0x0", "col", Some("1")),
            (Some("col"), "Color,nil", "col", Some("0")),
        ] {
            let mut invalid = Prop::parse(pos(), "color", target, body).unwrap().unwrap();
            assert!(invalid.bind(var, val).is_err(), "{body}: {var}={val:?}");
        }
    }

    #[test]
    fn validates_color_prop() {
        for (var, body) in [
            (Some("col"), "Color,0x0"),
            (Some("col"), "1color,0x0"),
            (Some("col"), "Color,0xffffff"),
            (Some("col"), "Color,0XFFFFFF"),
            (Some("col"), "Color,nil"),
            (None, "0x0"),
            (None, "0XFFFFFF"),
            (None, "nil"),
        ] {
            let prop = Prop::parse(pos(), "color", var, body).unwrap().unwrap();
            let mut vars = HashSet::new();
            let mut names = HashSet::new();
            prop.validate(&mut vars, &mut names).unwrap();
        }

        for (var, body) in [
            (Some("1invalid"), "Color,0x0"),
            (Some("col"), "effect.foo,0x0"),
            (Some("col"), "123,0x0"),
            (Some("col"), "Color,_"),
            (Some("col"), "Color,0x1000000"),
            (None, "_"),
            (None, "0x1000000"),
        ] {
            let mut vars = HashSet::new();
            let mut names = HashSet::new();
            let err = Prop::parse(pos(), "color", var, body)
                .unwrap()
                .unwrap()
                .validate(&mut vars, &mut names)
                .unwrap_err();
            assert!(err.to_string().starts_with("input.anm2:2:3:"), "{var:?}: {body}: {err}");
        }

        let prop = Prop::parse(pos(), "color", Some("col"), "Color,0x0").unwrap().unwrap();
        let mut vars = HashSet::from(["col"]);
        let mut names = HashSet::new();
        assert!(prop.validate(&mut vars, &mut names).is_err());

        let mut vars = HashSet::new();
        let mut names = HashSet::from([Cow::Borrowed("Color")]);
        assert!(prop.validate(&mut vars, &mut names).is_err());

        let mut neg = Prop::parse(pos(), "color", Some("_"), "Color,_").unwrap().unwrap();
        neg.bind("col", Some("-1")).unwrap();
        assert!(neg.validate(&mut HashSet::new(), &mut HashSet::new()).is_err());

        let mut over = Prop::parse(pos(), "color", Some("_"), "Color,_").unwrap().unwrap();
        over.bind("col", Some("16777216")).unwrap();
        assert!(over.validate(&mut HashSet::new(), &mut HashSet::new()).is_err());
    }
}
