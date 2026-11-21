use crate::build::lang::lua;
use crate::build::source::Pos;
use anyhow::bail;
use std::borrow::Cow;
use std::collections::HashSet;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Color {
    Nil,
    Int(i64),
}

#[derive(Debug, Clone, Copy)]
#[repr(u8)]
enum Flag {
    Undefined = 1 << 0,
    Position = 1 << 1,
    Fixed = 1 << 2,
    Alpha = 1 << 3,
}

#[derive(Debug, Default, Clone)]
struct Options<'a> {
    flags: u8,
    items: Option<Vec<&'a str>>,
}

impl<'a> Options<'a> {
    fn parse(pos: &Pos, arg: Option<&'a str>) -> Self {
        let mut flags = 0;
        let items = arg.map(|arg| {
            arg.split('|')
                .map(|opt| {
                    let opt = opt.trim();
                    match opt {
                        "undefined" => flags |= Flag::Undefined as u8,
                        "position" => flags |= Flag::Position as u8,
                        "fixed" => flags |= Flag::Fixed as u8,
                        "alpha" => flags |= Flag::Alpha as u8,
                        _ => tracing::warn!("{pos}: unknown option '{opt}' in property 'colorlist'"),
                    }
                    opt
                })
                .collect()
        });
        Self { flags, items }
    }

    fn has(&self, flag: Flag) -> bool {
        self.flags & (flag as u8) != 0
    }
}

#[derive(Debug, Clone)]
pub(in crate::build::exedit) struct Prop<'a> {
    pos: Pos,
    var: &'a str,
    names: Vec<&'a str>,
    default: Option<Vec<Color>>,
    max: i64,
    options: Options<'a>,
    rest: Vec<&'a str>,
}

impl<'a> Prop<'a> {
    pub(in crate::build::exedit) fn parse(
        pos: Pos,
        kind: &str,
        var: &'a str,
        body: &'a str,
    ) -> anyhow::Result<Option<Self>> {
        if kind != "colorlist" {
            return Ok(None);
        }

        if body.chars().any(char::is_control) {
            bail!("{pos}: property 'colorlist' cannot contain control characters");
        }

        let mut parts = body.split(',');
        let names: Vec<_> = parts.next().unwrap().split('|').map(str::trim).collect();
        let count = parts.clone().count();

        if count < 2 {
            bail!("{pos}: property 'colorlist' requires at least 2 arguments, got {count}");
        }

        if count > 3 {
            tracing::warn!("{pos}: property 'colorlist' accepts at most 3 arguments, got {count}");
        }

        let default = match parts.next().unwrap().trim() {
            "_" => None,
            arg => {
                let mut colors = Vec::new();
                for part in arg.split('|').map(str::trim) {
                    let Some(color) = parse_color(part) else {
                        bail!(
                            "{pos}: default value of property 'colorlist' must be 'nil' \
                            or a hexadecimal integer, got '{part}'"
                        );
                    };

                    colors.push(color);
                }

                Some(colors)
            }
        };

        let arg = parts.next().unwrap().trim();
        let Ok(max) = arg.parse::<i64>() else {
            bail!("{pos}: maximum count of property 'colorlist' must be an integer, got '{arg}'");
        };

        let options = Options::parse(&pos, parts.next());
        let rest: Vec<_> = parts.collect();

        Ok(Some(Self {
            pos,
            var,
            names,
            default,
            max,
            options,
            rest,
        }))
    }

    pub(in crate::build::exedit) fn name(&self) -> &'a str {
        self.names[0]
    }

    pub(in crate::build::exedit) fn labels(&self) -> Vec<&'a str> {
        let primary = self.names[0]
            .rsplit_once("::")
            .map_or(self.names[0], |(_, label)| label);
        std::iter::once(primary)
            .chain(self.names[1..].iter().copied())
            .collect()
    }

    pub(in crate::build::exedit) fn bind(&mut self, var: &'a str, val: Option<&str>) -> anyhow::Result<()> {
        let pos = &self.pos;

        if self.var != "_" && self.var != var {
            bail!("{pos}: variable name '{}' conflicts with '{var}'", self.var);
        }

        if let Some(val) = val {
            if let Some(colors) =
                resolve_lua_colors(var, val, self.default.as_deref()).map_err(|err| anyhow::anyhow!("{pos}: {err}"))?
            {
                self.default = Some(colors);
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

        if !lua::is_ident(self.var).map_err(|err| anyhow::anyhow!("{pos}: {err}"))? {
            bail!(
                "{pos}: variable name '{}' of property 'colorlist' must be a Lua identifier",
                self.var
            );
        }

        if !vars.insert(self.var) {
            bail!("{pos}: duplicate variable name '{}'", self.var);
        }

        let primary = self.names[0];

        if primary.starts_with("effect.") {
            bail!("{pos}: property name must not start with 'effect.', got '{primary}'");
        }

        if lua::parse_number(primary).is_some() {
            bail!("{pos}: property name must not be a number, got '{primary}'");
        }

        if !names.insert(Cow::Borrowed(primary)) {
            bail!("{pos}: duplicate property name '{primary}'");
        }

        if self.max < 1 {
            bail!(
                "{pos}: maximum count of property 'colorlist' must be at least 1, got {}",
                self.max
            );
        }

        let Some(colors) = &self.default else {
            bail!("{pos}: default value of property 'colorlist' must be 'nil' or a hexadecimal integer, got '_'");
        };

        if colors.is_empty() {
            bail!("{pos}: property 'colorlist' requires at least 1 default value, got 0");
        }

        if !i64::try_from(colors.len()).is_ok_and(|len| len <= self.max) {
            bail!(
                "{pos}: default value count {} of property 'colorlist' exceeds maximum count {}",
                colors.len(),
                self.max
            );
        }

        let (max_color, limit) = if self.options.has(Flag::Alpha) {
            (0xffff_ffff, "0x00000000 and 0xffffffff")
        } else {
            (0xff_ffff, "0x000000 and 0xffffff")
        };

        for &color in colors {
            match color {
                Color::Nil if !self.options.has(Flag::Undefined) => {
                    bail!("{pos}: 'nil' default value of property 'colorlist' requires 'undefined' option");
                }
                Color::Int(val) if !(0..=max_color).contains(&val) => {
                    bail!("{pos}: default value of property 'colorlist' must be between {limit}, got {val}");
                }
                _ => {}
            }
        }

        Ok(())
    }
}

impl fmt::Display for Prop<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "--colorlist@{}:", self.var)?;

        for (i, name) in self.names.iter().enumerate() {
            if i > 0 {
                f.write_str("|")?;
            }
            f.write_str(name)?;
        }

        f.write_str(",")?;

        for (i, &color) in self.default.as_deref().unwrap_or(&[Color::Int(0)]).iter().enumerate() {
            if i > 0 {
                f.write_str("|")?;
            }
            match color {
                Color::Nil => f.write_str("nil")?,
                Color::Int(val) if self.options.has(Flag::Alpha) => write!(f, "0x{val:08x}")?,
                Color::Int(val) => write!(f, "0x{val:06x}")?,
            }
        }

        write!(f, ",{}", self.max)?;

        if let Some(items) = &self.options.items {
            f.write_str(",")?;
            for (i, opt) in items.iter().enumerate() {
                if i > 0 {
                    f.write_str("|")?;
                }
                f.write_str(opt)?;
            }
        }

        for arg in &self.rest {
            write!(f, ",{arg}")?;
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

fn resolve_lua_colors(var: &str, val: &str, default: Option<&[Color]>) -> anyhow::Result<Option<Vec<Color>>> {
    let Some(fields) = lua::parse_table(val)? else {
        bail!("initializer of variable '{var}' must be a table, got '{val}'");
    };

    if fields.iter().any(|field| field.key.is_some()) {
        bail!("initializer of variable '{var}' cannot contain keyed fields, got '{val}'");
    }

    if default.is_some() {
        return Ok(None);
    }

    let mut colors = Vec::with_capacity(fields.len());

    for field in fields {
        let Some(entry) = lua::parse_table(field.val)? else {
            bail!("element of variable '{var}' must be a table, got '{}'", field.val);
        };

        let mut col = None;
        for field in entry {
            let is_col = match field.key {
                Some(lua::Key::Name(key)) => key == "col",
                Some(lua::Key::Expr(key)) => lua::parse_string(key)?.as_deref() == Some("col"),
                None => false,
            };

            if is_col {
                if col.is_some() {
                    bail!("duplicate 'col' field in element of variable '{var}'");
                }

                let Some(parsed) = parse_lua_color(field.val) else {
                    bail!(
                        "color of element in variable '{var}' must be 'nil' or an integer, got '{}'",
                        field.val
                    );
                };
                col = Some(parsed);
            }
        }

        colors.push(col.ok_or_else(|| anyhow::anyhow!("missing 'col' field in element of variable '{var}'"))?);
    }

    Ok(Some(colors))
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
    fn parses_colorlist_prop() {
        let prop = Prop::parse(
            pos(),
            "colorlist",
            "list",
            " 基準色 | 中間色 , 0xffffff | 0x888888 , 4 , undefined | position ",
        )
        .unwrap()
        .unwrap();
        assert_eq!(prop.var, "list");
        assert_eq!(prop.name(), "基準色");
        assert_eq!(prop.labels(), vec!["基準色", "中間色"]);
        assert_eq!(
            prop.to_string(),
            "--colorlist@list:基準色|中間色,0xffffff|0x888888,4,undefined|position"
        );

        let alpha = Prop::parse(pos(), "colorlist", "list2", "図形色,0xffffffff,1,alpha|fixed")
            .unwrap()
            .unwrap();
        assert_eq!(alpha.name(), "図形色");
        assert_eq!(alpha.labels(), vec!["図形色"]);
        assert_eq!(alpha.to_string(), "--colorlist@list2:図形色,0xffffffff,1,alpha|fixed");

        let no_opts = Prop::parse(pos(), "colorlist", "list", "Group::基準色|Group::中間色,0XFF00AA,2")
            .unwrap()
            .unwrap();
        assert_eq!(no_opts.name(), "Group::基準色");
        assert_eq!(no_opts.labels(), vec!["基準色", "Group::中間色"]);
        assert_eq!(
            no_opts.to_string(),
            "--colorlist@list:Group::基準色|Group::中間色,0xff00aa,2"
        );

        let nil_and_extra = Prop::parse(
            pos(),
            "colorlist",
            "list",
            "基準色,nil|0x1,2,undefined|custom,extra , more",
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            nil_and_extra.to_string(),
            "--colorlist@list:基準色,nil|0x000001,2,undefined|custom,extra , more"
        );

        assert!(
            Prop::parse(pos(), "color", "list", "基準色,0xffffff,1")
                .unwrap()
                .is_none()
        );

        for body in [
            "基準色",
            "基準色,0xffffff",
            "基準色,0xffffff,abc",
            "基準色,0xffffff,1.5",
            "基準色,0xffffff,0x2",
            "基準色,,1",
            "基準色,0xffffff|,1",
            "基準色,255,1",
            "基準色,0xFF0000,1",
            "基準色,0Xff0000,1",
            "基準色,0xffffff\n,1",
        ] {
            let err = Prop::parse(pos(), "colorlist", "list", body).unwrap_err();
            assert!(err.to_string().starts_with("input.anm2:2:3:"), "{body}: {err}");
        }
    }

    #[test]
    fn binds_var_and_default() {
        let mut unbound = Prop::parse(pos(), "colorlist", "_", "基準色|中間色,_,4,undefined|position")
            .unwrap()
            .unwrap();
        unbound
            .bind(
                "list",
                Some("{ { pos = 0, col = 0xffffff }, { pos = 1, col = 0x888888, extra = { 1, 2 } } }"),
            )
            .unwrap();
        assert_eq!(
            unbound.to_string(),
            "--colorlist@list:基準色|中間色,0xffffff|0x888888,4,undefined|position"
        );

        let mut nil_and_alpha = Prop::parse(pos(), "colorlist", "_", "図形色,_,4,undefined|alpha")
            .unwrap()
            .unwrap();
        nil_and_alpha
            .bind("list", Some("{ { col = nil }, { col = 0XFFFFFFFF } }"))
            .unwrap();
        assert_eq!(
            nil_and_alpha.to_string(),
            "--colorlist@list:図形色,nil|0xffffffff,4,undefined|alpha"
        );

        let mut with_defaults_no_init = Prop::parse(pos(), "colorlist", "list", "基準色,0xffffff,4")
            .unwrap()
            .unwrap();
        with_defaults_no_init.bind("list", None).unwrap();
        assert_eq!(with_defaults_no_init.to_string(), "--colorlist@list:基準色,0xffffff,4");

        let mut with_defaults_empty_array = Prop::parse(pos(), "colorlist", "list", "基準色,0xffffff,4")
            .unwrap()
            .unwrap();
        with_defaults_empty_array.bind("list", Some("{}")).unwrap();
        assert_eq!(
            with_defaults_empty_array.to_string(),
            "--colorlist@list:基準色,0xffffff,4"
        );

        let mut with_defaults_any_array = Prop::parse(pos(), "colorlist", "list", "基準色,0xffffff,4")
            .unwrap()
            .unwrap();
        with_defaults_any_array
            .bind("list", Some("{ { col = 0x000000 }, get_col() }"))
            .unwrap();
        assert_eq!(
            with_defaults_any_array.to_string(),
            "--colorlist@list:基準色,0xffffff,4"
        );

        let mut bracketed_key = Prop::parse(pos(), "colorlist", "_", "図形色,_,3").unwrap().unwrap();
        bracketed_key
            .bind(
                "list",
                Some("{ { [\"col\"] = 0x112233 }, { ['col'] = 0x445566 }, { [ [[col]] ] = 0x778899 } }"),
            )
            .unwrap();
        assert_eq!(
            bracketed_key.to_string(),
            "--colorlist@list:図形色,0x112233|0x445566|0x778899,3"
        );

        for (target, body, var, val) in [
            ("other", "基準色,_,1", "list", Some("{ { col = 0xffffff } }")),
            ("list", "基準色,_,1", "list", None),
            ("list", "基準色,0xffffff,1", "list", Some("0xffffff")),
            ("list", "基準色,0xffffff,1", "list", Some("{ col = 0xffffff }")),
            (
                "list",
                "基準色,0xffffff,1",
                "list",
                Some("{ [1] = { col = 0xffffff } }"),
            ),
            ("list", "基準色,0xffffff,1", "list", Some("{} + {}")),
            ("list", "基準色,_,1", "list", Some("{ 0xffffff }")),
            ("list", "基準色,_,1", "list", Some("{ { pos = 0 } }")),
            ("list", "基準色,_,1", "list", Some("{ { [col] = 0xffffff } }")),
            (
                "list",
                "基準色,_,1",
                "list",
                Some("{ { col = 0xffffff, [\"col\"] = 0x000000 } }"),
            ),
            (
                "list",
                "基準色,_,1",
                "list",
                Some("{ { col = 0xffffff, col = 0x000000 } }"),
            ),
            ("list", "基準色,_,1", "list", Some("{ { col = other } }")),
            ("list", "基準色,_,1", "list", Some("{ { col = 0xffffff } + {} }")),
        ] {
            let mut invalid = Prop::parse(pos(), "colorlist", target, body).unwrap().unwrap();
            assert!(invalid.bind(var, val).is_err(), "{body}: {var}={val:?}");
        }
    }

    #[test]
    fn validates_colorlist_prop() {
        for (var, body) in [
            ("list", "基準色,0x0,1"),
            ("list", "1基準色,0xffffff,1"),
            ("list", "基準色|基準色|他の色,0xffffff|0x000000,2"),
            ("list", "基準色,0xffffff,2,fixed"),
            ("list", "基準色,0xffffffff,1,alpha|fixed"),
            ("list", "基準色,nil|0xffffff,2,undefined"),
            ("list", "基準色,nil|0xffffffff,2,undefined|alpha"),
        ] {
            let prop = Prop::parse(pos(), "colorlist", var, body).unwrap().unwrap();
            let mut vars = HashSet::new();
            let mut names = HashSet::new();
            prop.validate(&mut vars, &mut names).unwrap();
        }

        for (var, body) in [
            ("1invalid", "基準色,0xffffff,1"),
            ("list", "effect.foo,0xffffff,1"),
            ("list", "123,0xffffff,1"),
            ("list", "基準色,0xffffff,0"),
            ("list", "基準色,0xffffff,-1"),
            ("list", "基準色,_,1"),
            ("list", "基準色,0xffffff|0x000000,1"),
            ("list", "基準色,nil,1"),
            ("list", "基準色,0x1000000,1"),
            ("list", "基準色,0x100000000,1,alpha"),
        ] {
            let mut vars = HashSet::new();
            let mut names = HashSet::new();
            let err = Prop::parse(pos(), "colorlist", var, body)
                .unwrap()
                .unwrap()
                .validate(&mut vars, &mut names)
                .unwrap_err();
            assert!(err.to_string().starts_with("input.anm2:2:3:"), "{var}: {body}: {err}");
        }

        let prop = Prop::parse(pos(), "colorlist", "list", "Group::基準色|中間色,0x0,1")
            .unwrap()
            .unwrap();
        let mut vars = HashSet::from(["list"]);
        assert!(prop.validate(&mut vars, &mut HashSet::new()).is_err());

        let mut names = HashSet::from([Cow::Borrowed("Group::基準色")]);
        assert!(prop.validate(&mut HashSet::new(), &mut names).is_err());

        let mut names = HashSet::from([Cow::Borrowed("中間色")]);
        prop.validate(&mut HashSet::new(), &mut names).unwrap();

        let mut empty_bind = Prop::parse(pos(), "colorlist", "_", "基準色,_,1").unwrap().unwrap();
        empty_bind.bind("list", Some("{}")).unwrap();
        assert!(empty_bind.validate(&mut HashSet::new(), &mut HashSet::new()).is_err());

        let mut nil_without_opt = Prop::parse(pos(), "colorlist", "_", "基準色,_,1").unwrap().unwrap();
        nil_without_opt.bind("list", Some("{ { col = nil } }")).unwrap();
        assert!(
            nil_without_opt
                .validate(&mut HashSet::new(), &mut HashSet::new())
                .is_err()
        );

        let mut alpha_without_opt = Prop::parse(pos(), "colorlist", "_", "基準色,_,1").unwrap().unwrap();
        alpha_without_opt
            .bind("list", Some("{ { col = 0xffffffff } }"))
            .unwrap();
        assert!(
            alpha_without_opt
                .validate(&mut HashSet::new(), &mut HashSet::new())
                .is_err()
        );

        let mut exceeds_max = Prop::parse(pos(), "colorlist", "_", "基準色,_,1").unwrap().unwrap();
        exceeds_max
            .bind("list", Some("{ { col = 0xffffff }, { col = 0x000000 } }"))
            .unwrap();
        assert!(exceeds_max.validate(&mut HashSet::new(), &mut HashSet::new()).is_err());
    }
}
