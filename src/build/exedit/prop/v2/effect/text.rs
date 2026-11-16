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
                            bail!("{pos}: trailing characters after quoted default value of property 'text'");
                        }

                        return Ok(Self::Quoted(unescape_quoted(&rest[..index])));
                    }
                    _ => {}
                }
            }
        }

        let val = if arg.contains("\\n") {
            Cow::Owned(arg.replace("\\n", "\n"))
        } else {
            Cow::Borrowed(arg)
        };

        Ok(Self::Plain(val))
    }

    fn as_str(&self) -> &str {
        match self {
            Self::Plain(val) | Self::Quoted(val) => val,
        }
    }
}

impl fmt::Display for Text<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (mut rest, targets, quote) = match self {
            Self::Plain(val) => (val.as_ref(), ['\n'].as_slice(), ""),
            Self::Quoted(val) => (val.as_ref(), ['\n', '\t', '"', '\\'].as_slice(), "\""),
        };

        f.write_str(quote)?;

        while let Some(at) = rest.find(targets) {
            let esc = match rest.as_bytes()[at] {
                b'\n' => "\\n",
                b'\t' => "\\t",
                b'"' => "\\\"",
                _ => "\\\\",
            };
            write!(f, "{}{esc}", &rest[..at])?;
            rest = &rest[at + 1..];
        }

        write!(f, "{rest}{quote}")
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
        if kind != "text" {
            return Ok(None);
        }

        if body.chars().any(char::is_control) {
            bail!("{pos}: property 'text' cannot contain control characters");
        }

        let Some((name, arg)) = body.split_once(',') else {
            bail!("{pos}: property 'text' requires at least 1 argument, got 0");
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

            if text.chars().any(|ch| ch.is_control() && !matches!(ch, '\n' | '\t')) {
                bail!(
                    "{pos}: initializer of variable '{var}' cannot contain control characters other than '\\n' and '\\t'"
                );
            }

            match &self.default {
                default if default.as_str().trim() == "_" => {
                    self.default = if matches!(default, Text::Quoted(_))
                        || text.trim() == "_"
                        || text.ends_with(|ch: char| ch.is_whitespace() && ch != '\n')
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
                "{pos}: variable name '{}' of property 'text' must be a Lua identifier",
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
        write!(f, "--text@{}:{},{}", self.var, self.name, self.default)
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
    fn parses_text_prop() {
        let prop = Prop::parse(pos(), "text", "txt", "  Text ,  a,  b  ").unwrap().unwrap();
        assert_eq!(prop.var, "txt");
        assert_eq!(prop.name(), "Text");
        assert_eq!(prop.label(), "Text");
        assert_eq!(prop.to_string(), "--text@txt:Text,  a,  b  ");

        let empty = Prop::parse(pos(), "text", "txt", "Group::Sub::Text,").unwrap().unwrap();
        assert_eq!(empty.name(), "Group::Sub::Text");
        assert_eq!(empty.label(), "Text");
        assert_eq!(empty.to_string(), "--text@txt:Group::Sub::Text,");

        let spaced_underscore = Prop::parse(pos(), "text", "txt", "Text, _ ").unwrap().unwrap();
        assert_eq!(spaced_underscore.to_string(), "--text@txt:Text, _ ");

        let placeholder = Prop::parse(pos(), "text", "txt", "Text,_").unwrap().unwrap();
        assert_eq!(placeholder.to_string(), "--text@txt:Text,_");

        let multiline = Prop::parse(pos(), "text", "txt", r"Text,a\nb").unwrap().unwrap();
        assert_eq!(multiline.default.as_str(), "a\nb");
        assert_eq!(multiline.to_string(), r"--text@txt:Text,a\nb");

        let quoted = Prop::parse(pos(), "text", "txt", r#"Text,"a\nb\tc\"d\\e""#)
            .unwrap()
            .unwrap();
        assert_eq!(quoted.default.as_str(), "a\nb\tc\"d\\e");
        assert_eq!(quoted.to_string(), r#"--text@txt:Text,"a\nb\tc\"d\\e""#);

        assert!(Prop::parse(pos(), "string", "txt", "Text,foo").unwrap().is_none());
        assert!(Prop::parse(pos(), "text", "txt", "Text").is_err());
        assert!(Prop::parse(pos(), "text", "txt", "Text,a\nb").is_err());
        assert!(Prop::parse(pos(), "text", "txt", r#"Text,"abc" trailing"#).is_err());
    }

    #[test]
    fn binds_text_prop() {
        let mut prop = Prop::parse(pos(), "text", "_", "Text,_").unwrap().unwrap();
        prop.bind("txt", Some(r#""a\nb""#)).unwrap();
        assert_eq!(prop.var, "txt");
        assert_eq!(prop.to_string(), r"--text@txt:Text,a\nb");

        let mut tabbed = Prop::parse(pos(), "text", "_", "Text,_").unwrap().unwrap();
        tabbed.bind("txt", Some(r#""a\n\tb""#)).unwrap();
        assert_eq!(tabbed.to_string(), r#"--text@txt:Text,"a\n\tb""#);

        let mut matching = Prop::parse(pos(), "text", "txt", r"Text,a\nb").unwrap().unwrap();
        matching.bind("txt", Some(r#""a\nb""#)).unwrap();

        let mut strict = Prop::parse(pos(), "text", "_", r#"Text,"_""#).unwrap().unwrap();
        strict.bind("txt", Some(r#""a\nb""#)).unwrap();
        assert_eq!(strict.to_string(), r#"--text@txt:Text,"a\nb""#);

        let mut explicit = Prop::parse(pos(), "text", "_", "Text,hello").unwrap().unwrap();
        explicit.bind("txt", None).unwrap();
        assert_eq!(explicit.to_string(), "--text@txt:Text,hello");

        let mut unbound = Prop::parse(pos(), "text", "_", "Text,_").unwrap().unwrap();
        assert!(unbound.bind("txt", None).is_err());
        assert!(unbound.bind("txt", Some(r#""a\rb""#)).is_err());
        assert!(unbound.bind("txt", Some(r#""a\r\nb""#)).is_err());
        assert!(unbound.bind("txt", Some(r#""a\0b""#)).is_err());
        assert!(unbound.bind("txt", Some(r#""\xff""#)).is_err());
        assert!(matching.bind("other", Some(r#""a\nb""#)).is_err());
        assert!(matching.bind("txt", Some(r#""other""#)).is_err());
        assert!(matching.bind("txt", Some("123")).is_err());
    }

    #[test]
    fn validates_text_prop() {
        for body in ["Text,hello", "1txt,hello", "Text,_"] {
            let prop = Prop::parse(pos(), "text", "txt", body).unwrap().unwrap();
            let mut vars = HashSet::new();
            let mut names = HashSet::new();
            prop.validate(&mut vars, &mut names).unwrap();
        }

        for (var, body) in [
            ("", "Text,hello"),
            ("local", "Text,hello"),
            ("1bad", "Text,hello"),
            ("txt", "effect.txt,hello"),
            ("txt", "123,hello"),
        ] {
            let prop = Prop::parse(pos(), "text", var, body).unwrap().unwrap();
            let mut vars = HashSet::new();
            let mut names = HashSet::new();
            assert!(prop.validate(&mut vars, &mut names).is_err(), "{var}: {body}");
        }

        let prop = Prop::parse(pos(), "text", "txt", "Text,hello").unwrap().unwrap();
        let mut vars = HashSet::from(["txt"]);
        let mut names = HashSet::new();
        assert!(prop.validate(&mut vars, &mut names).is_err());

        let mut vars = HashSet::new();
        let mut names = HashSet::from([Cow::Borrowed("Text")]);
        assert!(prop.validate(&mut vars, &mut names).is_err());
    }
}
