use crate::build::lang::lua;
use crate::build::source::Pos;
use anyhow::bail;
use std::borrow::Cow;
use std::collections::HashSet;
use std::fmt;

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub(in crate::build::exedit) struct Prop<'a> {
    pos: Pos,
    var: &'a str,
    name: &'a str,
    default: Text<'a>,
}

#[derive(Debug, Clone)]
enum Text<'a> {
    Plain(Cow<'a, str>),
    Quoted(Cow<'a, str>),
}

impl<'a> Text<'a> {
    fn parse(pos: &Pos, arg: &'a str) -> anyhow::Result<Self> {
        if let Some(rest) = arg.strip_prefix('"') {
            let mut bytes = rest.bytes().enumerate();

            while let Some((index, byte)) = bytes.next() {
                match byte {
                    b'\\' => _ = bytes.next(),
                    b'"' => {
                        if !rest[index + 1..].trim().is_empty() {
                            bail!("{pos}: trailing characters after quoted default value of property 'string'");
                        }

                        let val = unescape_quoted(&rest[..index]);

                        if val.contains('\n') {
                            bail!("{pos}: property 'string' cannot contain '\\n'");
                        }

                        return Ok(Self::Quoted(val));
                    }
                    _ => {}
                }
            }
        }

        if arg.contains("\\n") {
            bail!("{pos}: property 'string' cannot contain '\\n'");
        }

        Ok(Self::Plain(Cow::Borrowed(arg)))
    }

    fn as_str(&self) -> &str {
        match self {
            Self::Plain(val) | Self::Quoted(val) => val,
        }
    }
}

impl fmt::Display for Text<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut rest = match self {
            Self::Plain(val) => return f.write_str(val),
            Self::Quoted(val) => val.as_ref(),
        };

        f.write_str("\"")?;

        while let Some(at) = rest.find(['\t', '"', '\\']) {
            let esc = match rest.as_bytes()[at] {
                b'\t' => "\\t",
                b'"' => "\\\"",
                _ => "\\\\",
            };
            write!(f, "{}{esc}", &rest[..at])?;
            rest = &rest[at + 1..];
        }

        write!(f, "{rest}\"")
    }
}

#[allow(dead_code)]
impl<'a> Prop<'a> {
    pub(in crate::build::exedit) fn parse(
        pos: Pos,
        kind: &str,
        var: &'a str,
        body: &'a str,
    ) -> anyhow::Result<Option<Self>> {
        if kind != "string" {
            return Ok(None);
        }

        if body.chars().any(char::is_control) {
            bail!("{pos}: property 'string' cannot contain control characters");
        }

        let Some((name, arg)) = body.split_once(',') else {
            bail!("{pos}: property 'string' requires at least 1 argument, got 0");
        };

        let default = Text::parse(&pos, arg)?;

        Ok(Some(Self {
            pos,
            var,
            name: name.trim(),
            default,
        }))
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
            let Some(text) = lua::parse_string(val).map_err(|err| anyhow::anyhow!("{pos}: {err}"))? else {
                bail!("{pos}: initializer of variable '{var}' must be a string, got '{val}'");
            };

            if text.chars().any(|ch| ch.is_control() && ch != '\t') {
                bail!("{pos}: initializer of variable '{var}' cannot contain control characters other than '\\t'");
            }

            match &self.default {
                default if default.as_str().trim() == "_" => {
                    self.default = if matches!(default, Text::Quoted(_))
                        || text.trim() == "_"
                        || text.ends_with(char::is_whitespace)
                        || text.contains(['\t', '"', '\\'])
                    {
                        Text::Quoted(Cow::Owned(text.into_owned()))
                    } else {
                        Text::Plain(Cow::Owned(text.into_owned()))
                    };
                }
                default if default.as_str() != text.as_ref() => {
                    bail!("{pos}: default value '{default}' conflicts with '{val}'");
                }
                _ => {}
            }
        } else if self.default.as_str().trim() == "_" {
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
                "{pos}: variable name '{}' of property 'string' must be a Lua identifier",
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

        Ok(())
    }
}

impl fmt::Display for Prop<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "--string@{}:{},{}", self.var, self.name, self.default)
    }
}

fn unescape_quoted(mut src: &str) -> Cow<'_, str> {
    if !src.contains('\\') {
        return Cow::Borrowed(src);
    }

    let mut dst = String::with_capacity(src.len());

    while let Some(at) = src.find('\\') {
        dst.push_str(&src[..at]);
        let mut chars = src[at + 1..].chars();

        match chars.next() {
            Some('n') => dst.push('\n'),
            Some('t') => dst.push('\t'),
            Some('"') => dst.push('"'),
            Some('\\') | None => dst.push('\\'),
            Some(ch) => {
                dst.push('\\');
                dst.push(ch);
            }
        }

        src = chars.as_str();
    }

    dst.push_str(src);
    Cow::Owned(dst)
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
    fn parses_string_prop() {
        let prop = Prop::parse(pos(), "string", "s", "  String ,  a,  b  ")
            .unwrap()
            .unwrap();
        assert_eq!(prop.var, "s");
        assert_eq!(prop.name(), "String");
        assert_eq!(prop.label(), "String");
        assert_eq!(prop.to_string(), "--string@s:String,  a,  b  ");

        let empty = Prop::parse(pos(), "string", "s", "Group::Sub::String,")
            .unwrap()
            .unwrap();
        assert_eq!(empty.name(), "Group::Sub::String");
        assert_eq!(empty.label(), "String");
        assert_eq!(empty.to_string(), "--string@s:Group::Sub::String,");

        let spaced_underscore = Prop::parse(pos(), "string", "s", "String, _ ").unwrap().unwrap();
        assert_eq!(spaced_underscore.to_string(), "--string@s:String, _ ");

        let placeholder = Prop::parse(pos(), "string", "s", "String,_").unwrap().unwrap();
        assert_eq!(placeholder.to_string(), "--string@s:String,_");

        let quoted = Prop::parse(pos(), "string", "s", r#"String,"a\tb\"c\\d\xe"  "#)
            .unwrap()
            .unwrap();
        assert_eq!(quoted.default.as_str(), "a\tb\"c\\d\\xe");
        assert_eq!(quoted.to_string(), r#"--string@s:String,"a\tb\"c\\d\\xe""#);

        let plain_tab = Prop::parse(pos(), "string", "s", r"String,a\tb").unwrap().unwrap();
        assert_eq!(plain_tab.default.as_str(), r"a\tb");
        assert_eq!(plain_tab.to_string(), r"--string@s:String,a\tb");

        let unclosed = Prop::parse(pos(), "string", "s", r#"String,"unclosed"#)
            .unwrap()
            .unwrap();
        assert_eq!(unclosed.default.as_str(), r#""unclosed"#);
        assert_eq!(unclosed.to_string(), r#"--string@s:String,"unclosed"#);

        let leading_space = Prop::parse(pos(), "string", "s", r#"String, "abc""#).unwrap().unwrap();
        assert_eq!(leading_space.default.as_str(), r#" "abc""#);

        assert!(Prop::parse(pos(), "text", "s", "String,foo").unwrap().is_none());
        assert!(Prop::parse(pos(), "string", "s", "String").is_err());
        assert!(Prop::parse(pos(), "string", "s", "String,a\tb").is_err());
        assert!(Prop::parse(pos(), "string", "s", r"String,a\nb").is_err());
        assert!(Prop::parse(pos(), "string", "s", r#"String,"a\nb""#).is_err());
        assert!(Prop::parse(pos(), "string", "s", r#"String,"abc" trailing"#).is_err());
    }

    #[test]
    fn binds_string_prop() {
        let mut prop = Prop::parse(pos(), "string", "_", "String,_").unwrap().unwrap();
        prop.bind("s", Some(r#""a\tb""#)).unwrap();
        assert_eq!(prop.var, "s");
        assert_eq!(prop.to_string(), r#"--string@s:String,"a\tb""#);

        let mut leading_space = Prop::parse(pos(), "string", "_", "String,_").unwrap().unwrap();
        leading_space.bind("s", Some(r#"" abc""#)).unwrap();
        assert_eq!(leading_space.to_string(), "--string@s:String, abc");

        let mut trailing_space = Prop::parse(pos(), "string", "_", "String,_").unwrap().unwrap();
        trailing_space.bind("s", Some(r#""abc ""#)).unwrap();
        assert_eq!(trailing_space.to_string(), r#"--string@s:String,"abc ""#);

        let mut leading_quote = Prop::parse(pos(), "string", "_", "String,_").unwrap().unwrap();
        leading_quote.bind("s", Some(r#""\"abc""#)).unwrap();
        assert_eq!(leading_quote.to_string(), r#"--string@s:String,"\"abc""#);

        let mut underscore_val = Prop::parse(pos(), "string", "_", "String,_").unwrap().unwrap();
        underscore_val.bind("s", Some(r#""_""#)).unwrap();
        assert_eq!(underscore_val.to_string(), r#"--string@s:String,"_""#);

        let mut strict = Prop::parse(pos(), "string", "_", r#"String," _ ""#).unwrap().unwrap();
        strict.bind("s", Some(r#""hello""#)).unwrap();
        assert_eq!(strict.to_string(), r#"--string@s:String,"hello""#);

        let mut matching = Prop::parse(pos(), "string", "s", r#"String,"a\tb""#).unwrap().unwrap();
        matching.bind("s", Some(r#""a\tb""#)).unwrap();
        assert_eq!(matching.to_string(), r#"--string@s:String,"a\tb""#);

        let mut plain_matching = Prop::parse(pos(), "string", "s", r"String,a\tb").unwrap().unwrap();
        plain_matching.bind("s", Some(r#""a\\tb""#)).unwrap();
        assert!(plain_matching.bind("s", Some(r#""a\tb""#)).is_err());

        let mut spaced_plain = Prop::parse(pos(), "string", "s", "String, foo").unwrap().unwrap();
        assert!(spaced_plain.bind("s", Some(r#""foo""#)).is_err());

        let mut explicit = Prop::parse(pos(), "string", "_", "String,foo").unwrap().unwrap();
        explicit.bind("s", None).unwrap();
        assert_eq!(explicit.to_string(), "--string@s:String,foo");

        let mut unbound = Prop::parse(pos(), "string", "_", "String,_").unwrap().unwrap();
        assert!(unbound.bind("s", None).is_err());
        assert!(unbound.bind("s", Some(r#""a\nb""#)).is_err());
        assert!(unbound.bind("s", Some(r#""a\rb""#)).is_err());
        assert!(unbound.bind("s", Some(r#""a\0b""#)).is_err());
        assert!(unbound.bind("s", Some(r#""\xff""#)).is_err());
        assert!(prop.bind("other", Some(r#""a\tb""#)).is_err());
        assert!(prop.bind("s", Some(r#""other""#)).is_err());
        assert!(prop.bind("s", Some("123")).is_err());

        let mut strict_unbound = Prop::parse(pos(), "string", "_", r#"String,"_""#).unwrap().unwrap();
        assert!(strict_unbound.bind("s", None).is_err());
    }

    #[test]
    fn validates_string_prop() {
        for body in ["String,foo", "1str,foo", "String,_"] {
            let prop = Prop::parse(pos(), "string", "s", body).unwrap().unwrap();
            let mut vars = HashSet::new();
            let mut names = HashSet::new();
            prop.validate(&mut vars, &mut names).unwrap();
        }

        for (var, body) in [
            ("", "String,foo"),
            ("local", "String,foo"),
            ("1bad", "String,foo"),
            ("s", "effect.str,foo"),
            ("s", "123,foo"),
        ] {
            let prop = Prop::parse(pos(), "string", var, body).unwrap().unwrap();
            let mut vars = HashSet::new();
            let mut names = HashSet::new();
            assert!(prop.validate(&mut vars, &mut names).is_err(), "{var}: {body}");
        }

        let prop = Prop::parse(pos(), "string", "s", "String,foo").unwrap().unwrap();
        let mut vars = HashSet::from(["s"]);
        let mut names = HashSet::new();
        assert!(prop.validate(&mut vars, &mut names).is_err());

        let mut vars = HashSet::new();
        let mut names = HashSet::from([Cow::Borrowed("String")]);
        assert!(prop.validate(&mut vars, &mut names).is_err());
    }
}
