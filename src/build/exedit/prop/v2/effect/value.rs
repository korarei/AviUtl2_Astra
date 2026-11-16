use crate::build::lang::lua::{self, TokenKind};
use crate::build::source::Pos;
use anyhow::bail;
use std::borrow::Cow;
use std::collections::HashSet;
use std::fmt::{self, Write};

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub(in crate::build::exedit) struct Prop<'a> {
    pos: Pos,
    var: &'a str,
    name: &'a str,
    default: Option<Cow<'a, str>>,
    rest: Option<&'a str>,
}

#[allow(dead_code)]
impl<'a> Prop<'a> {
    pub(in crate::build::exedit) fn parse(
        pos: Pos,
        kind: &str,
        var: &'a str,
        body: &'a str,
    ) -> anyhow::Result<Option<Self>> {
        if kind != "value" {
            return Ok(None);
        }

        if body.chars().any(char::is_control) {
            bail!("{pos}: property 'value' cannot contain control characters");
        }

        let Some((name, arg)) = body.split_once(',') else {
            bail!("{pos}: property 'value' requires at least 1 argument, got 0");
        };

        let arg = arg.trim();
        if arg.is_empty() {
            bail!("{pos}: property 'value' requires at least 1 argument, got 0");
        }

        let tokens: Vec<_> = lua::Lexer::new(arg)
            .collect::<Result<_, _>>()
            .map_err(|err| anyhow::anyhow!("{pos}: {err}"))?;
        let mut parser = lua::Parser::new(arg, &tokens);
        let mut span = parser.parse_expr_at(0).map_err(|err| anyhow::anyhow!("{pos}: {err}"))?;
        let mut count = 1usize;
        let mut rest = None;
        let mut tail = tokens.iter().filter(|token| !token.kind.is_trivia());

        while let Some(next) = tail.find(|token| token.span.st >= span.ed) {
            if next.kind != TokenKind::Comma {
                bail!(
                    "{pos}: unexpected token '{}' in default value of property 'value'",
                    &arg[next.span.st..next.span.ed]
                );
            }

            count += 1;
            rest.get_or_insert(next.span.st);
            span = parser
                .parse_expr_at(next.span.ed)
                .map_err(|err| anyhow::anyhow!("{pos}: {err}"))?;
        }

        if count > 1 {
            tracing::warn!("{pos}: property 'value' accepts at most 1 argument, got {count}");
        }

        let (arg, rest) = rest.map_or((arg, None), |at| (arg[..at].trim_end(), Some(&arg[at + 1..])));
        let default = (arg != "_").then_some(Cow::Borrowed(arg));

        Ok(Some(Self {
            pos,
            var,
            name: name.trim(),
            default,
            rest,
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
            let expr = normalize_expr(pos, var, val)?;

            match &self.default {
                None => self.default = Some(Cow::Owned(expr)),
                Some(default) => {
                    let mut lhs = lua::Lexer::new(default)
                        .filter(|token| token.as_ref().map_or(true, |token| !token.kind.is_trivia()));
                    let mut rhs = lua::Lexer::new(&expr)
                        .filter(|token| token.as_ref().map_or(true, |token| !token.kind.is_trivia()));

                    loop {
                        match (
                            lhs.next().transpose().map_err(|err| anyhow::anyhow!("{pos}: {err}"))?,
                            rhs.next().transpose().map_err(|err| anyhow::anyhow!("{pos}: {err}"))?,
                        ) {
                            (None, None) => break,
                            (Some(lhs), Some(rhs))
                                if match (&lhs.kind, &rhs.kind) {
                                    (TokenKind::String(lhs), TokenKind::String(rhs)) => lhs.val == rhs.val,
                                    (lhs, rhs) => lhs == rhs,
                                } => {}
                            _ => bail!("{pos}: default value '{default}' conflicts with '{val}'"),
                        }
                    }
                }
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
                "{pos}: variable name '{}' of property 'value' must be a Lua identifier",
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
        let default = self.default.as_deref().unwrap_or("_");
        write!(f, "--value@{}:{},{default}", self.var, self.name)?;

        if let Some(rest) = self.rest {
            write!(f, ",{rest}")?;
        }

        Ok(())
    }
}

fn normalize_expr(pos: &Pos, var: &str, val: &str) -> anyhow::Result<String> {
    let tokens: Vec<_> = lua::Lexer::new(val)
        .collect::<Result<_, _>>()
        .map_err(|err| anyhow::anyhow!("{pos}: {err}"))?;
    let span = lua::Parser::new(val, &tokens)
        .parse_expr_at(0)
        .map_err(|err| anyhow::anyhow!("{pos}: {err}"))?;

    if let Some(next) = tokens
        .iter()
        .find(|token| token.span.st >= span.ed && !token.kind.is_trivia())
    {
        bail!(
            "{pos}: unexpected token '{}' in initializer of variable '{var}'",
            &val[next.span.st..next.span.ed]
        );
    }

    let mut dst = String::with_capacity(val.len());
    let mut has_trivia = false;

    for token in tokens {
        if token.span.st < span.st || token.span.ed > span.ed {
            continue;
        }

        if token.kind.is_trivia() {
            has_trivia = !dst.is_empty();
            continue;
        }

        if has_trivia {
            dst.push(' ');
            has_trivia = false;
        }

        let raw = &val[token.span.st..token.span.ed];
        if !raw.chars().any(char::is_control) {
            dst.push_str(raw);
            continue;
        }

        let TokenKind::String(literal) = token.kind else {
            bail!("{pos}: initializer of variable '{var}' cannot contain control characters");
        };

        write_string(&mut dst, &literal.val);
    }

    Ok(dst)
}

fn write_string(dst: &mut String, src: &[u8]) {
    dst.push('"');

    for chunk in src.utf8_chunks() {
        for ch in chunk.valid().chars() {
            match ch {
                '\\' => dst.push_str("\\\\"),
                '"' => dst.push_str("\\\""),
                '\x07' => dst.push_str("\\a"),
                '\x08' => dst.push_str("\\b"),
                '\t' => dst.push_str("\\t"),
                '\n' => dst.push_str("\\n"),
                '\x0b' => dst.push_str("\\v"),
                '\x0c' => dst.push_str("\\f"),
                '\r' => dst.push_str("\\r"),
                _ if !ch.is_control() => dst.push(ch),
                _ => {
                    let mut buf = [0; 4];

                    for byte in ch.encode_utf8(&mut buf).bytes() {
                        write!(dst, "\\{byte:03}").unwrap();
                    }
                }
            }
        }

        for byte in chunk.invalid() {
            write!(dst, "\\{byte:03}").unwrap();
        }
    }

    dst.push('"');
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
    fn parses_value_prop() {
        let prop = Prop::parse(pos(), "value", "val", "  Value ,  { first = 1, second = 2 }  ")
            .unwrap()
            .unwrap();
        assert_eq!(prop.var, "val");
        assert_eq!(prop.name(), "Value");
        assert_eq!(prop.label(), "Value");
        assert_eq!(prop.to_string(), "--value@val:Value,{ first = 1, second = 2 }");

        let scoped = Prop::parse(pos(), "value", "val", "Group::Sub::Value,_")
            .unwrap()
            .unwrap();
        assert_eq!(scoped.name(), "Group::Sub::Value");
        assert_eq!(scoped.label(), "Value");
        assert_eq!(scoped.to_string(), "--value@val:Group::Sub::Value,_");

        let quoted = Prop::parse(pos(), "value", "val", "Value, 'a,  b'").unwrap().unwrap();
        assert_eq!(quoted.to_string(), "--value@val:Value,'a,  b'");

        let call = Prop::parse(pos(), "value", "val", "Value, f(1, 2)").unwrap().unwrap();
        assert_eq!(call.to_string(), "--value@val:Value,f(1, 2)");

        assert!(Prop::parse(pos(), "string", "val", "Value,0").unwrap().is_none());

        for body in [
            "Value",
            "Value,",
            "Value,  ",
            "Value,{1, 2",
            "Value,1 +",
            "Value,true false",
            "Value,local a = 1",
            "Value,\"unterminated",
            "Value,a\tb",
        ] {
            assert!(Prop::parse(pos(), "value", "val", body).is_err(), "{body}");
        }
    }

    #[test]
    fn preserves_extra_arguments() {
        for (body, default, rest) in [
            ("Value,true,false", "true", "false"),
            ("Value,1,2", "1", "2"),
            ("Value,{1,2,3}, {2,3,4}, f(5,6)", "{1,2,3}", " {2,3,4}, f(5,6)"),
            ("Value,'a,b', 'c,d'", "'a,b'", " 'c,d'"),
        ] {
            let mut prop = Prop::parse(pos(), "value", "val", body).unwrap().unwrap();
            assert_eq!(prop.default.as_deref(), Some(default));
            assert_eq!(prop.rest, Some(rest));
            assert_eq!(prop.to_string(), format!("--value@val:{body}"));
            prop.bind("val", Some(default)).unwrap();
            prop.validate(&mut HashSet::new(), &mut HashSet::new()).unwrap();
        }

        let mut prop = Prop::parse(pos(), "value", "_", "Value,_, 2").unwrap().unwrap();
        prop.bind("val", Some("1")).unwrap();
        assert_eq!(prop.to_string(), "--value@val:Value,1, 2");

        for body in ["Value,1,", "Value,1,true false", "Value,1,{2", "Value,1,'unfinished"] {
            assert!(Prop::parse(pos(), "value", "val", body).is_err(), "{body}");
        }
    }

    #[test]
    fn binds_value_prop() {
        let mut prop = Prop::parse(pos(), "value", "_", "Value,_").unwrap().unwrap();
        prop.bind("val", Some("{\n  first = 1, -- comment\n  second = 2\n}"))
            .unwrap();
        assert_eq!(prop.var, "val");
        assert_eq!(prop.to_string(), "--value@val:Value,{ first = 1, second = 2 }");

        let mut multiline_str = Prop::parse(pos(), "value", "_", "Value,_").unwrap().unwrap();
        multiline_str.bind("val", Some("[[a\nb\tc]]")).unwrap();
        assert_eq!(multiline_str.to_string(), r#"--value@val:Value,"a\nb\tc""#);

        let mut raw_tab_str = Prop::parse(pos(), "value", "_", "Value,_").unwrap().unwrap();
        raw_tab_str.bind("val", Some("\"a\tb\"")).unwrap();
        assert_eq!(raw_tab_str.to_string(), r#"--value@val:Value,"a\tb""#);

        let mut matching_table = Prop::parse(pos(), "value", "val", "Value,{ first = 1, second = 2 }")
            .unwrap()
            .unwrap();
        matching_table
            .bind("val", Some("{\n  first = 1,\n  second = 2\n}"))
            .unwrap();

        let mut matching_str = Prop::parse(pos(), "value", "val", "Value,'a,  b'").unwrap().unwrap();
        matching_str.bind("val", Some(r#""a,  b""#)).unwrap();

        let mut explicit = Prop::parse(pos(), "value", "_", "Value,0").unwrap().unwrap();
        explicit.bind("val", None).unwrap();
        assert_eq!(explicit.to_string(), "--value@val:Value,0");

        let mut raw_ctrl_str = Prop::parse(pos(), "value", "_", "Value,_").unwrap().unwrap();
        raw_ctrl_str.bind("val", Some("[[a\r\nb\0c]]")).unwrap();
        assert_eq!(raw_ctrl_str.to_string(), r#"--value@val:Value,"a\r\nb\000c""#);

        let mut raw_byte_str = Prop::parse(pos(), "value", "_", "Value,_").unwrap().unwrap();
        raw_byte_str.bind("val", Some("\"a\\xff\tb\"")).unwrap();
        assert_eq!(raw_byte_str.to_string(), r#"--value@val:Value,"a\255\tb""#);

        for (input, expected) in [
            (r#""a\rb""#, r#""a\rb""#),
            (r#""a\0b""#, r#""a\0b""#),
            (r#""\xff""#, r#""\xff""#),
        ] {
            let mut prop = Prop::parse(pos(), "value", "_", "Value,_").unwrap().unwrap();
            prop.bind("val", Some(input)).unwrap();
            assert_eq!(prop.to_string(), format!("--value@val:Value,{expected}"));
        }

        let mut unbound = Prop::parse(pos(), "value", "_", "Value,_").unwrap().unwrap();
        assert!(unbound.bind("val", None).is_err());
        assert!(unbound.bind("val", Some("")).is_err());
        assert!(unbound.bind("val", Some("-- comment")).is_err());
        assert!(prop.bind("other", Some("{ first = 1, second = 2 }")).is_err());
        assert!(prop.bind("val", Some("{ first = 1, second = 3 }")).is_err());
    }

    #[test]
    fn normalizes_expressions() {
        for (src, expected) in [
            (" -- comment\n value \t + -- comment\n 1 ", "value + 1"),
            (
                "[[\"\\\x07\x08\t\n\x0b\x0c\r\u{85}\u{e9}]]",
                "\"\\\"\\\\\\a\\b\\t\\n\\v\\f\\r\\194\\133\u{e9}\"",
            ),
            ("\"\\xc3\\xa9\\xff\t\"", "\"\u{e9}\\255\\t\""),
        ] {
            assert_eq!(normalize_expr(&pos(), "val", src).unwrap(), expected, "{src:?}");
        }

        for src in ["", "-- comment", "1,2", "true false", "1 +", "'unterminated"] {
            assert!(normalize_expr(&pos(), "val", src).is_err(), "{src:?}");
        }
    }

    #[test]
    fn validates_value_prop() {
        for body in ["Value,0", "1val,0", "Value,{}"] {
            let prop = Prop::parse(pos(), "value", "val", body).unwrap().unwrap();
            let mut vars = HashSet::new();
            let mut names = HashSet::new();
            prop.validate(&mut vars, &mut names).unwrap();
        }

        let unbound = Prop::parse(pos(), "value", "val", "Value,_").unwrap().unwrap();
        let mut vars = HashSet::new();
        let mut names = HashSet::new();
        unbound.validate(&mut vars, &mut names).unwrap();
        assert_eq!(unbound.to_string(), "--value@val:Value,_");

        for (var, body) in [
            ("", "Value,0"),
            ("local", "Value,0"),
            ("1bad", "Value,0"),
            ("val", "effect.val,0"),
            ("val", "123,0"),
        ] {
            let prop = Prop::parse(pos(), "value", var, body).unwrap().unwrap();
            let mut vars = HashSet::new();
            let mut names = HashSet::new();
            assert!(prop.validate(&mut vars, &mut names).is_err(), "{var}: {body}");
        }

        let prop = Prop::parse(pos(), "value", "val", "Value,0").unwrap().unwrap();
        let mut vars = HashSet::from(["val"]);
        let mut names = HashSet::new();
        assert!(prop.validate(&mut vars, &mut names).is_err());

        let mut vars = HashSet::new();
        let mut names = HashSet::from([Cow::Borrowed("Value")]);
        assert!(prop.validate(&mut vars, &mut names).is_err());
    }
}
